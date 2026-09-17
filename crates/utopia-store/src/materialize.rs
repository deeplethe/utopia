//! 绑上的签名下的开放陈述算成类型化事实（0044 决定 3 的第三片，见 0067）。
//!
//! 类型化图谱是视图：一条类型化行 = 一条开放陈述 × 它签名的绑定。谓词是绑定给的属性，
//! 主宾按绑定的方向（reverse 就是陈述的宾语当主语），字面值、世界轴时间、来源时间、
//! 置信度照抄，证据与限定各复制一份，`from_statement_id` 指回那条陈述。带 mood 限定的
//! 陈述不算。
//!
//! 重算是集合运算，跑多少遍结果一样：先作废「源头不成立」的类型化行（陈述作废了、签名
//! 不再绑着、绑到了别的属性或反了方向），再给「该有而没有」的陈述补一行。绑定不变的行
//! 不动——它们的 id、证据和记录时间都留着。没有模型调用。

use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

/// 陈述与绑定对得上的条件：短语归一后相等，两端的类相同（空也相同），宾语是不是字面值相同。
/// 两处 SQL 共用；`s` 是开放陈述（facts），`se`/`oe` 是它两端的实体，`b` 是 phrase_bindings
const MATCH: &str = "b.kb_id = s.kb_id
       AND b.phrase = lower(btrim(regexp_replace(s.phrase, '\\s+', ' ', 'g')))
       AND b.subject_type_id IS NOT DISTINCT FROM se.type_id
       AND b.object_is_value = (s.object_id IS NULL)
       AND (s.object_id IS NULL OR b.object_type_id IS NOT DISTINCT FROM oe.type_id)";

/// 一轮重算写了什么。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Outcome {
    /// 作废的类型化行
    pub retired: u64,
    /// 新算出来的类型化行
    pub added: u64,
}

/// 对一个库重算一遍。
pub async fn materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<Outcome> {
    let mut tx = pool.begin().await?;

    // 1. 作废源头不成立的：陈述死了、签名没绑着、属性或方向变了、陈述带了 mood
    let retired = sqlx::query(&format!(
        "UPDATE facts t
            SET invalidated_at = now()
          WHERE t.kb_id = $1 AND t.layer = 'typed' AND t.invalidated_at IS NULL
            AND t.from_statement_id IS NOT NULL
            AND NOT EXISTS (
                SELECT 1
                  FROM facts s
                  JOIN entities se ON se.id = s.subject_id
             LEFT JOIN entities oe ON oe.id = s.object_id
                  JOIN phrase_bindings b ON {MATCH}
                 WHERE s.id = t.from_statement_id
                   AND s.invalidated_at IS NULL
                   AND b.status = 'bound'
                   AND b.relation_type_id = t.predicate_id
                   AND ((b.direction = 'forward' AND t.subject_id = s.subject_id)
                     OR (b.direction = 'reverse' AND t.subject_id = s.object_id))
                   AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q
                                    WHERE q.fact_id = s.id AND q.role = 'mood'))"
    ))
    .bind(kb_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();

    // 2. 补该有而没有的：先选出（陈述, 绑定）对，再逐条落行——id 是 v7，库里没有生成
    //    它的函数。reverse 只对两样东西之间的关系有意义（字面值当不了主语）
    let due: Vec<(Uuid, Uuid, String)> = sqlx::query_as(&format!(
        "SELECT s.id, b.relation_type_id, b.direction
           FROM facts s
           JOIN entities se ON se.id = s.subject_id
      LEFT JOIN entities oe ON oe.id = s.object_id
           JOIN phrase_bindings b ON {MATCH}
          WHERE s.kb_id = $1 AND s.layer = 'open' AND s.invalidated_at IS NULL
            AND b.status = 'bound'
            AND (b.direction = 'forward' OR s.object_id IS NOT NULL)
            AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q
                             WHERE q.fact_id = s.id AND q.role = 'mood')
            AND NOT EXISTS (SELECT 1 FROM facts t
                             WHERE t.from_statement_id = s.id AND t.invalidated_at IS NULL
                               AND t.predicate_id = b.relation_type_id)
          ORDER BY s.id"
    ))
    .bind(kb_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut added: Vec<(Uuid, Uuid)> = Vec::with_capacity(due.len());
    for (statement, property, direction) in &due {
        let id = Uuid::now_v7();
        let reverse = direction == "reverse";
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                                valid_from, valid_from_precision, valid_to, valid_to_precision,
                                attested_from, attested_to, confidence, layer, from_statement_id)
             SELECT $1, s.kb_id,
                    CASE WHEN $4 THEN s.object_id ELSE s.subject_id END,
                    $3,
                    CASE WHEN $4 THEN s.subject_id ELSE s.object_id END,
                    CASE WHEN $4 THEN NULL ELSE s.object_value END,
                    s.valid_from, s.valid_from_precision, s.valid_to, s.valid_to_precision,
                    s.attested_from, s.attested_to, s.confidence, 'typed', s.id
               FROM facts s WHERE s.id = $2",
        )
        .bind(id)
        .bind(statement)
        .bind(property)
        .bind(reverse)
        .execute(&mut *tx)
        .await?;
        added.push((id, *statement));
    }

    // 3. 新行抄证据与限定：证据是同一段原文的同一处引文；限定照角色词原样带过去
    if !added.is_empty() {
        let ids: Vec<Uuid> = added.iter().map(|(id, _)| *id).collect();
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version,
                                        proposed_predicate, quote_start, quote_end)
             SELECT t.id, e.chunk_id, e.quote, e.document_id, e.doc_version,
                    e.proposed_predicate, e.quote_start, e.quote_end
               FROM facts t
               JOIN fact_evidence e ON e.fact_id = t.from_statement_id
              WHERE t.id = ANY($1)
             ON CONFLICT DO NOTHING",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO statement_qualifiers (fact_id, role, value, entity_id)
             SELECT t.id, q.role, q.value, q.entity_id
               FROM facts t
               JOIN statement_qualifiers q ON q.fact_id = t.from_statement_id
              WHERE t.id = ANY($1)
             ON CONFLICT DO NOTHING",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Outcome {
        retired,
        added: added.len() as u64,
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
