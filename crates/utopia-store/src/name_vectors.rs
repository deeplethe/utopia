//! 名字向量：召回的第二条通道（0041 决定 3，第 2 刀）。
//!
//! 通道 1 是字面相等：mention 的名字与某条名字事实 `normalize_name` 之后一样。它找不到
//! 简称（海探1 ↔ 海洋探测器1号）、找不到另一种文字写的同一个名字（#709），而这两种
//! 恰恰是「多名」被错拆成两个实体的主因。这里给每条名字事实存名字字符串本身的向量，
//! 查询时在同一个库里取最近的几条；**只提议，不决定**——最近的名字是不是同一个东西，
//! 由消解那头按既有规矩（画像、裁决器，往后是第 3 刀的证据）去判。
//!
//! 向量随嵌入模型走，不定维（与 `chunks.embedding` 同）。查询照 0035 的两条规矩写：
//! `<=>` 两侧 cast 到字面维度、谓词里带 `vector_dims(...) = N`，HNSW 建好了就走索引，
//! 没建走精确路径，结果一样。

use pgvector::Vector;
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

use crate::vector_index::{self, Target};

/// 一次最多提议几条。名字向量的近邻里真正相关的很少超过前几个；再多只是给裁决器
/// 添噪音。数值待 `identity.mjs` 定，先取一个不会刷爆队列的
pub const TOP_K: usize = 8;

/// 余弦下限。名字字符串的向量比整段文本的向量「紧」——两个不相干的名字也能有
/// 0.4 上下的余弦——所以这条线比画像的 `SIM_ATTACH` 高。同样是待测量的临时值
pub const SIM_FLOOR: f32 = 0.60;

/// 还没有向量的名字事实：现行的、`known_as` 上的、`name_vectors` 里没有它的。
/// 按写入先后取（`facts.id` 是 uuid v7，按它排即按写入排；这张表没有 created_at——
/// 端到端跑出来的：按一个不存在的列排，补向量每篇都静默失败），一次取一批
pub async fn pending(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
) -> AppResult<Vec<(Uuid, Uuid, String)>> {
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT f.id, f.subject_id, f.object_value->>'value'
           FROM facts f
           JOIN relation_types nr ON nr.id = f.predicate_id
           LEFT JOIN name_vectors v ON v.fact_id = f.id
          WHERE f.kb_id = $1 AND nr.kb_id = $1 AND nr.builtin AND nr.key = $2
            AND f.invalidated_at IS NULL AND f.object_value IS NOT NULL
            AND v.fact_id IS NULL
          ORDER BY f.id
          LIMIT $3",
    )
    .bind(kb_id)
    .bind(crate::names::KNOWN_AS)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 写一批名字向量：`(fact_id, entity_id, embedding)`。同一条事实重写就覆盖（换了模型
/// 重算）。第一次写下这个维度的向量，索引就该排上了（0035）
pub async fn set(pool: &PgPool, kb_id: Uuid, items: &[(Uuid, Uuid, Vec<f32>)]) -> AppResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (fact_id, entity_id, emb) in items {
        sqlx::query(
            "INSERT INTO name_vectors (fact_id, kb_id, entity_id, embedding)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (fact_id) DO UPDATE SET embedding = EXCLUDED.embedding",
        )
        .bind(fact_id)
        .bind(kb_id)
        .bind(entity_id)
        .bind(Vector::from(emb.clone()))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    if let Some((_, _, first)) = items.first() {
        vector_index::request(pool, Target::NameVectors, first.len()).await?;
    }
    Ok(())
}

/// 一条被召回的名字：它挂在哪个实体上、那个实体是什么类、名字本身、与查询的余弦
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Near {
    pub entity_id: Uuid,
    pub fact_id: Uuid,
    pub name: String,
    pub type_label: Option<String>,
    pub similarity: f32,
}

/// 同库里与查询向量最近的 `k` 条名字，按相似度降序。只看现行的名字事实、没被合并掉
/// 的实体。不按 `description` 过滤：被描述的东西没有名字事实（0044），JOIN facts 已经
/// 把它排除了；万一将来一个有名字的东西也带上描述，它的名字照样该被召回（#877 评审）。
///
/// 里层按索引能接住的形状取 `k * 4` 条最近的，外层再用事实与实体的状态过滤——过滤
/// 放在里层会让索引用不上（0035）；取四倍是给过滤留余量，名字事实作废和实体合并
/// 都不常见，通常一条都不会被滤掉
pub async fn nearest(pool: &PgPool, kb_id: Uuid, query: &[f32], k: usize) -> AppResult<Vec<Near>> {
    let dims = query.len();
    if dims == 0 || k == 0 {
        return Ok(Vec::new());
    }
    let distance = vector_index::distance("v.embedding", 2, dims);
    let same_dims = vector_index::same_dims("v.embedding", dims);
    let sql = format!(
        // 外层次序照 `vector_index::RESORT` 的规矩（`distance + 0, id`），列名限定到 CTE：
        // 外层还联着 facts / entities / entity_types，裸写 `id` 会二义
        "WITH near AS MATERIALIZED (
             SELECT v.fact_id AS id, v.entity_id, ({distance}) AS distance
               FROM name_vectors v
              WHERE v.kb_id = $1 AND {same_dims}
              ORDER BY {distance}
              LIMIT $3
         )
         SELECT n.entity_id, n.id AS fact_id,
                f.object_value->>'value' AS name,
                et.label AS type_label,
                (1 - n.distance)::real AS similarity
           FROM near n
           JOIN facts f ON f.id = n.id AND f.invalidated_at IS NULL
           JOIN entities e ON e.id = n.entity_id AND e.merged_into IS NULL
           LEFT JOIN entity_types et ON et.id = e.type_id
          ORDER BY n.distance + 0, n.id
          LIMIT $4",
    );
    let rows: Vec<Near> = sqlx::query_as(&sql)
        .bind(kb_id)
        .bind(Vector::from(query.to_vec()))
        .bind((k * 4) as i64)
        .bind(k as i64)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}
