//! 短语签名的向量（#1097，0110）。
//!
//! 短语对齐给候选多的签名开短名单，要把签名的文本（短语、一条例句、一段引文）嵌入。
//! 签名的文本在两轮之间几乎不变，从前每轮全部重嵌：库里一万七千条候选多的签名，每轮
//! 一万七千次嵌入，判的可能只是新来的几十条。这里按 (库, 嵌入模型, 文本的哈希) 存下
//! 算过的向量，一轮只嵌没见过的文本。
//!
//! 只按键读，不做近邻查询，所以不建 HNSW、不登记 `vector_index`；`embedding` 不定维，
//! 随所选嵌入模型（与 `chunks.embedding` 同一条规矩）。这是缓存不是账本：删了下一轮
//! 重嵌，结果一样。

use pgvector::Vector;
use sqlx::PgPool;
use std::collections::HashMap;
use utopia_core::AppResult;
use uuid::Uuid;

/// 这些哈希里已经存了向量的那些，哈希 → 向量
pub async fn get(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
    hashes: &[Vec<u8>],
) -> AppResult<HashMap<Vec<u8>, Vec<f32>>> {
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Vec<u8>, Vector)> = sqlx::query_as(
        "SELECT text_hash, embedding FROM signature_vectors
          WHERE kb_id = $1 AND model = $2 AND text_hash = ANY($3)",
    )
    .bind(kb_id)
    .bind(model)
    .bind(hashes)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(hash, v)| (hash, v.to_vec()))
        .collect())
}

/// 存一批新嵌的向量。一个事务一次提交：逐条自动提交是一行一次 fsync。撞键（同一段文本
/// 两轮之间被别处写过）不覆盖：同一模型同一段文字，向量是同一个
pub async fn put(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
    items: &[(Vec<u8>, Vec<f32>)],
) -> AppResult<()> {
    if items.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (hash, emb) in items {
        sqlx::query(
            "INSERT INTO signature_vectors (kb_id, model, text_hash, embedding)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT DO NOTHING",
        )
        .bind(kb_id)
        .bind(model)
        .bind(hash)
        .bind(Vector::from(emb.clone()))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 删掉这个库里不再用得上的向量：哈希不在 `keep` 里的，以及别的模型嵌的（换了模型，
/// 旧向量与新属性向量不在一个空间里，留着只占地方）。`keep` 要是这一轮**全部**签名的
/// 哈希，不只是这一轮嵌了的——否则一批嵌入失败，上一轮存好的那批跟着被删。返回删了几行
pub async fn prune(pool: &PgPool, kb_id: Uuid, model: &str, keep: &[Vec<u8>]) -> AppResult<u64> {
    Ok(sqlx::query(
        "DELETE FROM signature_vectors
          WHERE kb_id = $1 AND (model <> $2 OR NOT (text_hash = ANY($3)))",
    )
    .bind(kb_id)
    .bind(model)
    .bind(keep)
    .execute(pool)
    .await?
    .rows_affected())
}
