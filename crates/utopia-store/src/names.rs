//! 名字是关于实体的一条事实（0041 决定 1）。
//!
//! 一个实体的每个名字——本名、简称、曾用名、另一种文字的写法——都是内建属性
//! `known_as` 上的一条值事实：`object_value = {"value": 名字}`，`object_id` 为空。
//! 它有出处（证据引文）、两个时钟、可以有有效期（更名），能被撤回，合并时跟着
//! 其它事实一起搬、撤回合并时一起搬回去。**它是值，不是节点**：画布只画带
//! `object_id` 的边。
//!
//! 读的时候要分清两件事，这个模块给两种 SQL 片段：
//! - 召回：`has_name_in` —— 这个实体有没有叫这些名字之一的（含曾用名：世界轴上
//!   结束了的名字仍然认得出旧文档里的它，所以只看记录轴 `invalidated_at`）；
//! - 列事实：`not_a_name` —— 实体面板、审阅卡、度数、规则这些地方，名字不算
//!   一条「事实」，不能冒充证据，也不能把计数撑大。

use crate::graph::{add_evidence, insert_value_fact, Validity};
use crate::resolution::normalize_name;
use sqlx::PgPool;
use std::collections::HashMap;
use utopia_core::models::NameView;
use utopia_core::AppResult;
use uuid::Uuid;

/// 内建属性的 key。与 `is_a` 同一个做法：`builtin = TRUE`，建库时不铺，第一次要时建
pub const KNOWN_AS: &str = "known_as";

/// 取（没有就建）这个库的 `known_as` 属性。
pub async fn ensure_known_as(pool: &PgPool, kb_id: Uuid) -> AppResult<Uuid> {
    if let Some((id,)) =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM relation_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(KNOWN_AS)
            .fetch_optional(pool)
            .await?
    {
        return Ok(id);
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, temporal, builtin, description)
         VALUES ($1, $2, $3, 'known as', 'attribute', 'text', 'state', TRUE, $4)
         ON CONFLICT (kb_id, key) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(KNOWN_AS)
    .bind(
        "A name a text uses for this entity. Each name is a fact with its source and both clocks; \
         a name is a value and never becomes a node.",
    )
    .execute(pool)
    .await?;
    // ON CONFLICT 命中说明并发建过了，取回那一条
    let (id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM relation_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(KNOWN_AS)
            .fetch_one(pool)
            .await?;
    Ok(id)
}

/// 一个名字的出处：哪一块、引文。没有出处的名字（人在界面上写的、迁移补的）传 None
#[derive(Debug, Clone, Copy)]
pub struct NameSource<'a> {
    pub chunk_id: Uuid,
    pub quote: &'a str,
}

/// 记下「这段文字管这个实体叫这个名字」。返回名字事实的 id；名字是空的返回 None。
///
/// 同一个实体同一个名字只有一行（`insert_value_fact` 按主语、谓词、值去重），
/// 再被提到只是多一条证据、证据日期往早挪。
pub async fn record(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    name: &str,
    source: Option<NameSource<'_>>,
    attested_at: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Option<Uuid>> {
    let name = normalize_name(name);
    if name.is_empty() {
        return Ok(None);
    }
    let attr = ensure_known_as(pool, kb_id).await?;
    let (fact_id, _) = insert_value_fact(
        pool,
        kb_id,
        entity_id,
        Some(attr),
        &serde_json::json!({ "value": name }),
        Validity {
            attested_at,
            ..Default::default()
        },
        1.0,
    )
    .await?;
    if let Some(s) = source {
        add_evidence(pool, fact_id, s.chunk_id, Some(s.quote), Some(KNOWN_AS)).await?;
    }
    Ok(Some(fact_id))
}

/// 召回用：实体 `{entity}` 有一条现行的名字事实，库 id 在第 `${kb}` 个参数，
/// 小写值在第 `${names}` 个参数（text[]）里。
///
/// 只看记录轴：世界轴上结束了的名字（曾用名）照样召回——更名之前的文档还在用它。
///
/// **写成不相关子查询，按库过滤，并带上索引的谓词**（`facts_value_text_idx` 是
/// `(kb_id, lower(value)) WHERE invalidated_at IS NULL AND object_value IS NOT NULL`）。
/// 相关子查询 `nf.subject_id = e.id` 不带库，规划器用不上这个索引，每次召回都要
/// 扫全表的 facts——所有库的
pub fn has_name_in(entity: &str, kb: usize, names: usize) -> String {
    format!(
        "{entity}.id IN (SELECT nf.subject_id FROM facts nf
                   JOIN relation_types nr ON nr.id = nf.predicate_id
                  WHERE nf.kb_id = ${kb} AND nr.kb_id = ${kb} AND nr.builtin AND nr.key = '{KNOWN_AS}'
                    AND nf.invalidated_at IS NULL AND nf.object_value IS NOT NULL
                    AND lower(nf.object_value->>'value') = ANY(${names}))"
    )
}

/// 查找用：实体 `{entity}` 有一条现行的名字事实 ILIKE 第 `${param}` 个参数。
pub fn has_name_like(entity: &str, param: usize) -> String {
    format!(
        "EXISTS (SELECT 1 FROM facts nf
                   JOIN relation_types nr ON nr.id = nf.predicate_id
                  WHERE nf.subject_id = {entity}.id AND nf.kb_id = {entity}.kb_id
                    AND nr.builtin AND nr.key = '{KNOWN_AS}'
                    AND nf.invalidated_at IS NULL
                    AND nf.object_value->>'value' ILIKE ${param})"
    )
}

/// 列事实用：事实 `{fact}` 不是名字事实。
pub fn not_a_name(fact: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM relation_types nr
                      WHERE nr.id = {fact}.predicate_id AND nr.builtin AND nr.key = '{KNOWN_AS}')"
    )
}

/// 一个实体的名字栏：本名在前，其余按首次记下的先后。`as_of` 回放到当时库里记着的名字。
pub async fn for_entity(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<NameView>> {
    Ok(sqlx::query_as(&format!(
        "SELECT f.id AS fact_id, f.object_value->>'value' AS name,
                lower(f.object_value->>'value') = lower(e.canonical_name) AS canonical,
                f.recorded_at, f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                ARRAY(SELECT DISTINCT fe.document_id FROM fact_evidence fe
                       WHERE fe.fact_id = f.id AND fe.document_id IS NOT NULL
                       ORDER BY fe.document_id) AS document_ids,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count
           FROM facts f
           JOIN relation_types r ON r.id = f.predicate_id
           JOIN entities e ON e.id = $2
          WHERE f.kb_id = $1 AND r.builtin AND r.key = '{KNOWN_AS}'
            AND {owner} = $2 AND {held}
          ORDER BY canonical DESC, f.recorded_at",
        owner = crate::record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false),
        held = crate::record_axis::facts_held_at("f", as_of.map(|_| 3)),
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(as_of)
    .fetch_all(pool)
    .await?)
}

/// 一个刚从原文里读到的别名，别的实体已经在用：把这一对送去裁决（0041 决定 5 的第一步）。
///
/// **只排队，不合并。** 两个实体叫同一个名字，可能是一个东西被拆开了（「海探1」先到、
/// 写着「简称海探1」的那篇后到），也可能就是两个东西。判断归裁决器与治理闸门。
/// 不排的话，后到的那篇把名字记在新实体上，两个实体都叫「海探1」，却没有任何东西
/// 把它们配成一对——入库顺序又一次决定了结果。
///
/// 类型一方为空或两边相同才配：声明了不同类型的同名，是重名那条路的事。
///
/// **已经判过「不是一个」的对不再排。** 同一个简称每出现在一块里就会走到这里一次；
/// 人（或裁决器）分开过的一对要是每次都重新进队列，分开这个决定就等于没有记住。
/// 返回排进去的对数
pub async fn pair_shared_name(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    name: &str,
) -> AppResult<usize> {
    let name = normalize_name(name);
    let others: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT e.id
           FROM entities e
           JOIN entities me ON me.id = $2
           JOIN facts nf ON nf.subject_id = e.id AND nf.kb_id = $1
                        AND nf.invalidated_at IS NULL AND nf.object_value IS NOT NULL
           JOIN relation_types nr ON nr.id = nf.predicate_id AND nr.builtin AND nr.key = $4
          WHERE e.kb_id = $1 AND e.merged_into IS NULL AND e.id <> $2
            AND lower(nf.object_value->>'value') = lower($3)
            AND (e.type_id IS NULL OR me.type_id IS NULL OR e.type_id = me.type_id)
            AND NOT EXISTS (
                SELECT 1 FROM resolution_reviews rr
                 WHERE rr.kb_id = $1 AND rr.status = 'kept'
                   AND least(rr.left_id, rr.right_id) = least(e.id, $2)
                   AND greatest(rr.left_id, rr.right_id) = greatest(e.id, $2))
          LIMIT 4",
    )
    .bind(kb_id)
    .bind(entity_id)
    .bind(&name)
    .bind(KNOWN_AS)
    .fetch_all(pool)
    .await?;
    for (other,) in &others {
        crate::resolution::create_review(
            pool,
            kb_id,
            entity_id,
            *other,
            0.0,
            &format!("shared_name|{name}"),
            crate::resolution::ReviewStage::Adjudicating,
        )
        .await?;
    }
    Ok(others.len())
}

/// 两个名字都带数字、而数字不一样：召回不把它们配成一对。
///
/// 名字相近、只差一个数的东西几乎从来不是同一个：`migration 394` 与 `migration 395`、
/// `#440` 与 `#441`、`海洋探测器1号` 与 `2号`。名字向量分不开它们——只差一个数的两个
/// 名字在任何嵌入模型下余弦都在 0.9 以上——包含关系也分不开（`migration 39` 是
/// `migration 395` 的子串）。一个工程文档库上，这类候选裁决了 1,663 对、只合并 3 对，
/// 却占了队列的四成（#1104）。
///
/// **只在两边都有数字时才拦。** 一边没有数字的照常提议：`沧海` 与 `沧海分布式推理平台 2.0`
/// 是简称对全称，正是包含关系要接住的那一种。
///
/// 比的是排过序的数字串，前导零不算（`0106` 就是 `106`），全角数字当半角。中文数字
/// 不认：`一号` 的「一」和 `一个` 的「一」这里分不出来，宁可照常提议。
pub fn numbers_differ(a: &str, b: &str) -> bool {
    let (a, b) = (digit_runs(a), digit_runs(b));
    !a.is_empty() && !b.is_empty() && a != b
}

fn digit_runs(name: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut open = false;
    for c in name.chars() {
        let digit = match c {
            '0'..='9' => Some(c),
            '\u{FF10}'..='\u{FF19}' => char::from_u32(c as u32 - 0xFEE0),
            _ => None,
        };
        match digit {
            Some(d) if open => runs.last_mut().expect("an open run").push(d),
            Some(d) => {
                runs.push(d.to_string());
                open = true;
            }
            None => open = false,
        }
    }
    for run in &mut runs {
        let trimmed = run.trim_start_matches('0');
        *run = if trimmed.is_empty() {
            "0".into()
        } else {
            trimmed.into()
        };
    }
    runs.sort();
    runs
}

/// 本名以外的现行名字，按首次记下的先后。给裁决器与审阅卡的「又名」那一行
pub async fn other_names(pool: &PgPool, entity_id: Uuid) -> AppResult<Vec<String>> {
    Ok(other_names_many(pool, &[entity_id])
        .await?
        .remove(&entity_id)
        .unwrap_or_default())
}

/// Keep deduplication and first-seen ordering within each entity. The KB join
/// lets the existing (kb_id, subject_id) indexes serve even a single-ID lookup.
pub(crate) async fn other_names_many(
    pool: &PgPool,
    entity_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<String>>> {
    if entity_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT entity_id, v FROM (
           SELECT DISTINCT ON (f.subject_id, lower(f.object_value->>'value'))
                  f.subject_id AS entity_id, f.object_value->>'value' AS v, f.recorded_at
             FROM entities e
             JOIN facts f ON f.subject_id = e.id AND f.kb_id = e.kb_id
             JOIN relation_types r ON r.id = f.predicate_id
            WHERE e.id = ANY($1) AND r.builtin AND r.key = $2
              AND f.invalidated_at IS NULL
              AND lower(f.object_value->>'value') <> lower(e.canonical_name)
            ORDER BY f.subject_id, lower(f.object_value->>'value'), f.recorded_at
         ) n ORDER BY n.entity_id, n.recorded_at",
    )
    .bind(entity_ids)
    .bind(KNOWN_AS)
    .fetch_all(pool)
    .await?;
    let mut names: HashMap<Uuid, Vec<String>> = HashMap::new();
    for (id, name) in rows {
        names.entry(id).or_default().push(name);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::numbers_differ;

    #[test]
    fn names_with_different_numbers_are_not_a_pair() {
        for (a, b) in [
            ("migration 394", "migration 395"),
            ("#440", "#441"),
            ("海洋探测器1号", "海洋探测器2号"),
            ("api/main.py:84-142", "api/main.py:145-169"),
            ("migration 39", "migration 395"),
            ("v1.2", "v1.3"),
        ] {
            assert!(numbers_differ(a, b), "{a} / {b}");
        }
    }

    #[test]
    fn the_same_numbers_or_none_on_one_side_still_pair() {
        for (a, b) in [
            ("海洋探测器1号", "海探1"),
            ("沧海", "沧海分布式推理平台 2.0"),
            ("migration 0106", "migration 106"),
            ("Ｑ３ ２０２４", "2024 Q3"),
            ("sección 01010101", "Sección 01010101 (Distrito Nacional)"),
            ("Sherlock Holmes", "Mr. Holmes"),
        ] {
            assert!(!numbers_differ(a, b), "{a} / {b}");
        }
    }
}
