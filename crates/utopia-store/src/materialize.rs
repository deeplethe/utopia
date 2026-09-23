//! 绑上的签名下的开放陈述算成类型化事实（0044 决定 3 的第三、四片，见 0067、0068）。
//!
//! 类型化图谱是视图：一条类型化行 = 一个三元组，来自一条或几条开放陈述与它们签名的绑定。
//! 谓词是绑定给的属性，主宾按绑定的方向（reverse 就是陈述的宾语当主语），字面值、世界轴
//! 时间、来源时间、置信度从陈述来；证据与限定各复制一份；来源记在 `typed_fact_sources`，
//! `from_statement_id` 是第一条。带 mood 限定的陈述不算。
//!
//! 写行走类型化图谱本来的门（[`crate::graph::insert_fact`] / [`crate::graph::insert_value_fact`]）：同断言
//! 同起点复用那一行，裸行被带时间的观察取代并链上，「结束了」关上开着的行——两份文档说
//! 同一件事，时间线上是一条边。
//!
//! 重算是集合运算，跑多少遍结果一样：先删「不再成立」的来源（陈述作废了、签名不再绑着、
//! 绑到了别的属性或反了方向、行本身作废了），再作废来源全空的类型化行，最后给「该有而
//! 没有」的（陈述, 绑定）对补上——有同断言的行就并进去，没有才新建。没有模型调用。

use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

use crate::graph::{insert_fact_on, FactObject, Validity};

/// 陈述与绑定对得上的条件：短语归一后相等，两端的类相同（空也相同），宾语是不是字面值相同。
/// `s` 是开放陈述（facts），`se`/`oe` 是它两端的实体，`b` 是 phrase_bindings
const MATCH: &str = "b.kb_id = s.kb_id
       AND b.phrase = lower(btrim(regexp_replace(s.phrase, '\\s+', ' ', 'g')))
       AND b.subject_type_id IS NOT DISTINCT FROM se.type_id
       AND b.object_is_value = (s.object_id IS NULL)
       AND (s.object_id IS NULL OR b.object_type_id IS NOT DISTINCT FROM oe.type_id)";

/// 一轮重算写了什么。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// 作废的类型化行（来源全空了）
    pub retired: u64,
    /// 新建的类型化行
    pub added: u64,
    /// 并进已有行的陈述数
    pub merged: u64,
}

/// 一条该物化的（陈述, 绑定）对，连陈述上要抄的东西。
#[derive(sqlx::FromRow)]
struct Due {
    statement: Uuid,
    property: Uuid,
    direction: String,
    subject_id: Uuid,
    object_id: Option<Uuid>,
    object_value: Option<serde_json::Value>,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_from_precision: Option<String>,
    valid_from_grade: Option<String>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_to_precision: Option<String>,
    attested_from: Option<chrono::DateTime<chrono::Utc>>,
    confidence: f32,
}

/// 对一个库重算一遍。
///
/// Worker 走这一条：等多久都行，因为它没人在屏幕前面等。人的判定不走这条：
/// 见 `materialize_human`，那是另一条带预算的入口
pub async fn materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<Outcome> {
    // A worker and a human review can recompute the same base concurrently.
    // Serialize before reading due statements, and use this connection for the
    // whole recompute so waiting runs cannot exhaust the pool with lock holders.
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *tx)
        .await?;
    let outcome = materialize_in_tx(&mut tx, kb_id).await?;
    tx.commit().await?;
    Ok(outcome)
}

/// 任务里的入口（0051）：**试锁，不等**。拿到 `typed_materialize` 就在这条连接上
/// 跑完整的重算并提交；拿不到就回滚、返回 `None`，由调用方挂成 `Deferred` 稍后再来。
///
/// 为什么不像 `materialize` 那样等锁：等锁的是一条池里的连接，几个决定连着点下来，
/// 每个 job 都抱着一条连接排队，池就空了（0051 §Alternatives）。也为什么不像旧的
/// 人工入口那样给等待设 2 秒预算：job 没人在屏幕前面等，超时只是把同一次重算推到
/// 下一次重试，不如一开始就不等。人的那一次点击只提交决定和这个 job（同一事务，
/// `phrase_bindings::decide_with_delivery`），屏幕上等的是事件，不是锁。
pub async fn try_materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<Option<Outcome>> {
    let mut tx = pool.begin().await?;
    let acquired: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))",
    )
    .bind(kb_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if !acquired {
        tx.rollback().await?;
        return Ok(None);
    }
    let outcome = materialize_in_tx(&mut tx, kb_id).await?;
    tx.commit().await?;
    Ok(Some(outcome))
}

async fn materialize_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    kb_id: Uuid,
) -> AppResult<Outcome> {
    // 1. 删不再成立的来源：陈述死了、行死了、签名没绑着、属性或方向变了、陈述带了 mood
    sqlx::query(&format!(
        "DELETE FROM typed_fact_sources src
          USING facts t
          WHERE src.fact_id = t.id AND t.kb_id = $1
            AND NOT EXISTS (
                SELECT 1
                  FROM facts s
                  JOIN entities se ON se.id = s.subject_id
             LEFT JOIN entities oe ON oe.id = s.object_id
                  JOIN phrase_bindings b ON {MATCH}
                 WHERE s.id = src.statement_id
                   AND s.invalidated_at IS NULL
                   AND t.invalidated_at IS NULL
                   AND b.status = 'bound'
                   AND b.relation_type_id = t.predicate_id
                   AND ((b.direction = 'forward' AND t.subject_id = s.subject_id)
                     OR (b.direction = 'reverse' AND t.subject_id = s.object_id))
                   AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q
                                    WHERE q.fact_id = s.id AND q.role = 'mood'))"
    ))
    .bind(kb_id)
    .execute(&mut **tx)
    .await?;

    // 2. 作废来源全空的类型化行：只动算出来的行（带 from_statement_id 的），人写的不碰
    let retired = sqlx::query(
        "UPDATE facts t
            SET invalidated_at = now()
          WHERE t.kb_id = $1 AND t.layer = 'typed' AND t.invalidated_at IS NULL
            AND t.from_statement_id IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM typed_fact_sources src WHERE src.fact_id = t.id)",
    )
    .bind(kb_id)
    .execute(&mut **tx)
    .await?
    .rows_affected();

    // 3. 该有而没有的（陈述, 绑定）对：陈述活着、签名绑着、没 mood、还没有活着的行以它为来源
    //    且谓词相同。reverse 只对两样东西之间的关系有意义（字面值当不了主语）
    let due: Vec<Due> = sqlx::query_as(&format!(
        "SELECT s.id AS statement, b.relation_type_id AS property, b.direction,
                s.subject_id, s.object_id, s.object_value,
                s.valid_from, s.valid_from_precision, s.valid_from_grade,
                s.valid_to, s.valid_to_precision,
                s.attested_from, s.confidence
           FROM facts s
           JOIN entities se ON se.id = s.subject_id
      LEFT JOIN entities oe ON oe.id = s.object_id
           JOIN phrase_bindings b ON {MATCH}
          WHERE s.kb_id = $1 AND s.layer = 'open' AND s.invalidated_at IS NULL
            AND b.status = 'bound'
            AND (b.direction = 'forward' OR s.object_id IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q
                             WHERE q.fact_id = s.id AND q.role = 'mood')
            AND NOT EXISTS (SELECT 1 FROM typed_fact_sources src JOIN facts t ON t.id = src.fact_id
                             WHERE src.statement_id = s.id AND t.invalidated_at IS NULL
                               AND t.predicate_id = b.relation_type_id)
          ORDER BY s.id"
    ))
    .bind(kb_id)
    .fetch_all(&mut **tx)
    .await?;

    let (mut added, mut merged) = (0u64, 0u64);
    for d in &due {
        let reverse = d.direction == "reverse";
        let validity = Validity {
            from: d.valid_from,
            from_precision: d.valid_from_precision.as_deref(),
            from_grade: d.valid_from_grade.as_deref(),
            to: d.valid_to,
            to_precision: d.valid_to_precision.as_deref(),
            attested_at: d.attested_from,
        };
        let (fact, new) = match (reverse, d.object_id, &d.object_value) {
            (true, Some(object), _) => {
                insert_fact_on(
                    tx,
                    kb_id,
                    object,
                    Some(d.property),
                    FactObject::Entity(d.subject_id),
                    validity,
                    d.confidence,
                )
                .await?
            }
            (false, Some(object), _) => {
                insert_fact_on(
                    tx,
                    kb_id,
                    d.subject_id,
                    Some(d.property),
                    FactObject::Entity(object),
                    validity,
                    d.confidence,
                )
                .await?
            }
            (false, None, Some(value)) => {
                insert_fact_on(
                    tx,
                    kb_id,
                    d.subject_id,
                    Some(d.property),
                    FactObject::Value(value),
                    validity,
                    d.confidence,
                )
                .await?
            }
            _ => continue,
        };
        if new {
            added += 1;
            sqlx::query("UPDATE facts SET from_statement_id = $2 WHERE id = $1 AND from_statement_id IS NULL")
                .bind(fact)
                .bind(d.statement)
                .execute(&mut **tx)
                .await?;
            // 新行取代了一条裸行（时间精化，supersedes 链上）：被取代那行的来源跟着搬过来，
            // 这一轮就收敛，不等下一轮把旧来源当「不成立」删掉再补
            sqlx::query(
                "INSERT INTO typed_fact_sources (fact_id, statement_id)
                 SELECT $1, src.statement_id
                   FROM facts n JOIN typed_fact_sources src ON src.fact_id = n.supersedes
                  WHERE n.id = $1
                 ON CONFLICT DO NOTHING",
            )
            .bind(fact)
            .execute(&mut **tx)
            .await?;
            sqlx::query(
                "DELETE FROM typed_fact_sources src
                  USING facts n
                  WHERE n.id = $1 AND src.fact_id = n.supersedes",
            )
            .bind(fact)
            .execute(&mut **tx)
            .await?;
        } else {
            merged += 1;
        }
        sqlx::query(
            "INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.statement)
        .execute(&mut **tx)
        .await?;
        // 证据与限定各抄一份：证据是同一段原文的同一处引文；限定照角色词原样带过去
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version,
                                        proposed_predicate, quote_start, quote_end)
             SELECT $1, chunk_id, quote, document_id, doc_version, proposed_predicate,
                    quote_start, quote_end
               FROM fact_evidence WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.statement)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO statement_qualifiers (fact_id, role, value, entity_id)
             SELECT $1, role, value, entity_id FROM statement_qualifiers WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(fact)
        .bind(d.statement)
        .execute(&mut **tx)
        .await?;
    }
    Ok(Outcome {
        retired,
        added,
        merged,
    })
}

/// 库里活着的、从陈述算出来的类型化行数。
pub async fn count(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM facts
          WHERE kb_id = $1 AND layer = 'typed' AND from_statement_id IS NOT NULL
            AND invalidated_at IS NULL",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?)
}

#[cfg(test)]
#[path = "materialize_delivery_tests.rs"]
mod delivery_tests;
