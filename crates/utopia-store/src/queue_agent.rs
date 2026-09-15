//! 审核台其余几档交给 agent（0043）时要取的数：等着的项、它们的证据、原文现在怎么写、
//! 人在这一档上做过的决定。只取数，不做决定——决定在 `utopia_server::queue_agent`。
//!
//! 取的都是**还没被 agent 看过**的项：agent 看过、说了什么的，要么已经动手（那一项多半
//! 已经离开了队列），要么留了建议等人。行被时间线重算改写过、换了 id 的，顺着 supersedes
//! 往上找，前身被看过就算看过——不然每重算一次，同一条事实就再问一遍模型。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

/// agent 在这一行或它的前身上已经有过一笔决定（任何状态）。别名固定用 `f`
const LOOKED_AT: &str = "EXISTS (
    WITH RECURSIVE lineage(id) AS (
        SELECT f.id
        UNION SELECT p.supersedes FROM facts p JOIN lineage l ON p.id = l.id
         WHERE p.supersedes IS NOT NULL)
    SELECT 1 FROM agent_decisions d
     WHERE d.target_kind = 'fact' AND d.target_id IN (SELECT id FROM lineage))";

/// 一条事实，给 agent 看的样子
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FactRow {
    pub id: Uuid,
    pub subject: String,
    pub predicate: Option<String>,
    /// 实体宾语的名字，或字面值的原样
    pub object: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_from_precision: Option<String>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_to_precision: Option<String>,
    pub confidence: f32,
    /// 证据全在旧版本的分块上（「待确认」那一档）
    pub stale: bool,
}

const FACT_COLUMNS: &str = "f.id, s.canonical_name AS subject,
       COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate,
       COALESCE(o.canonical_name, f.object_value ->> 'summary', f.object_value ->> 'value',
                f.object_value #>> '{}') AS object,
       f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision, f.confidence";

/// 低置信与证据过期两档里还没被 agent 看过的事实，先来先看。派生事实不在其中（它们不是
/// 抽出来的，没有原文可核）
pub async fn fact_queue(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<FactRow>> {
    let stale = crate::review::UNCONFIRMED_FACT;
    let rows = sqlx::query_as(&format!(
        "SELECT {FACT_COLUMNS}, ({stale}) AS stale
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.derived_by_rule IS NULL
           AND (f.confidence < $2 OR ({stale}))
           AND NOT {LOOKED_AT}
         ORDER BY f.recorded_at, f.id
         LIMIT $3"
    ))
    .bind(kb_id)
    .bind(crate::review::LOW_CONFIDENCE_BELOW)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 一条事实，按 id 取（不论死活）
pub async fn fact(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<Option<FactRow>> {
    let stale = crate::review::UNCONFIRMED_FACT;
    Ok(sqlx::query_as(&format!(
        "SELECT {FACT_COLUMNS}, ({stale}) AS stale
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.id = $2"
    ))
    .bind(kb_id)
    .bind(fact_id)
    .fetch_optional(pool)
    .await?)
}

/// 同一持有者、同一谓词上现存的其他行：冲突两边在时间线上的邻居
pub async fn neighbours(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<Vec<FactRow>> {
    let stale = crate::review::UNCONFIRMED_FACT;
    Ok(sqlx::query_as(&format!(
        "SELECT {FACT_COLUMNS}, ({stale}) AS stale
         FROM facts f
         JOIN facts me ON me.id = $2
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.id <> me.id
           AND f.subject_id = me.subject_id AND f.predicate_id = me.predicate_id
         ORDER BY f.valid_from NULLS LAST, f.recorded_at
         LIMIT 12"
    ))
    .bind(kb_id)
    .bind(fact_id)
    .fetch_all(pool)
    .await?)
}

/// 一条证据：哪份文档、那份文档自己的日期、引文
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Evidence {
    pub chunk_id: Uuid,
    pub document: String,
    /// 文档自己的日期（正文或来源给的），上传时刻不算
    pub dated: Option<DateTime<Utc>>,
    pub quote: Option<String>,
}

pub async fn evidence(pool: &PgPool, fact_id: Uuid) -> AppResult<Vec<Evidence>> {
    Ok(sqlx::query_as(
        "SELECT fe.chunk_id, d.filename AS document,
                CASE WHEN d.doc_time_source IN ('content', 'source') THEN d.doc_time END AS dated,
                fe.quote
         FROM fact_evidence fe
         JOIN documents d ON d.id = fe.document_id
         WHERE fe.fact_id = $1 AND d.deleted_at IS NULL
         ORDER BY d.doc_time NULLS LAST, fe.chunk_id
         LIMIT 4",
    )
    .bind(fact_id)
    .fetch_all(pool)
    .await?)
}

/// 证据过期的事实：它的文档**现在**的分块里提到主语的那几块。旧版本说过的话，新版本
/// 还说不说，得看新版本
pub async fn current_text(
    pool: &PgPool,
    fact_id: Uuid,
    subject: &str,
) -> AppResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT c.id, left(c.text, 1500)
         FROM chunks c
         WHERE c.superseded_at IS NULL
           AND c.document_id IN (SELECT fe.document_id FROM fact_evidence fe WHERE fe.fact_id = $1)
           AND strpos(lower(c.text), lower($2)) > 0
         ORDER BY c.seq
         LIMIT 2",
    )
    .bind(fact_id)
    .bind(subject)
    .fetch_all(pool)
    .await?)
}

/// 一条等着的冲突
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConflictRow {
    pub id: Uuid,
    pub reason: String,
    pub old_fact_id: Uuid,
    pub new_fact_id: Uuid,
}

/// 开着、还没被 agent 看过的冲突，先来先看
pub async fn conflict_queue(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<ConflictRow>> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.reason, c.old_fact_id, c.new_fact_id
         FROM fact_conflicts c
         WHERE c.kb_id = $1 AND c.status = 'open'
           AND NOT EXISTS (SELECT 1 FROM agent_decisions d
                            WHERE d.target_kind = 'conflict' AND d.target_id = c.id)
         ORDER BY c.created_at, c.id
         LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// 这个库里的人在这几种决定上最近怎么做的：每条一行，给模型当先例（0025 的约定：只认
/// 人写的行，机器自己的决定不是先例）
pub async fn precedents(
    pool: &PgPool,
    kb_id: Uuid,
    actions: &[&str],
    limit: i64,
) -> AppResult<Vec<String>> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT action, detail FROM audit_events
         WHERE kb_id = $1 AND actor_id IS NOT NULL AND action = ANY($2)
         ORDER BY created_at DESC
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(actions)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(action, d)| render_precedent(&action, &d))
        .collect())
}

/// 台账上一条人的决定写成一行
pub fn render_precedent(action: &str, d: &serde_json::Value) -> String {
    let text = |k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let body = if action.starts_with("conflict.") {
        format!(
            "{} {} {} vs {} {} {}",
            text("old_subject"),
            text("predicate"),
            text("old_object"),
            text("new_subject"),
            text("predicate"),
            text("new_object")
        )
    } else {
        format!(
            "{} {} {}",
            text("subject"),
            text("predicate"),
            text("object")
        )
    };
    let why = d
        .get("why")
        .and_then(|v| v.as_str())
        .map(|w| format!(" (because: {w})"))
        .unwrap_or_default();
    format!(
        "{action}: {}{why}",
        body.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// 还有没被 agent 看过的事实或冲突（定时扫描用）
pub async fn has_backlog(pool: &PgPool, kb_id: Uuid) -> AppResult<bool> {
    Ok(!fact_queue(pool, kb_id, 1).await?.is_empty()
        || !conflict_queue(pool, kb_id, 1).await?.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_precedent_reads_as_one_line_with_its_reason() {
        let fact = serde_json::json!({
            "subject": "HQ Lease", "predicate": "option deadline", "object": "2020-04-14",
            "why": "the sixth amendment's table states it"
        });
        assert_eq!(
            render_precedent("fact.confirm", &fact),
            "fact.confirm: HQ Lease option deadline 2020-04-14 (because: the sixth amendment's table states it)"
        );
        let conflict = serde_json::json!({
            "predicate": "landlord", "old_subject": "HQ Lease", "old_object": "HPBB1",
            "new_subject": "HQ Lease", "new_object": "BBHQ1"
        });
        assert_eq!(
            render_precedent("conflict.close_old", &conflict),
            "conflict.close_old: HQ Lease landlord HPBB1 vs HQ Lease landlord BBHQ1"
        );
    }
}
