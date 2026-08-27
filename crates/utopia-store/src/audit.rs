//! 审计日志：谁在何时对什么做了什么。纯审计——只记录与展示，不承载回滚等衍生功能。
//! 记录失败绝不影响业务操作（调用方一律 `let _ =`）。

use sqlx::PgPool;
use utopia_core::models::AuditEventView;
use utopia_core::AppResult;
use uuid::Uuid;

pub async fn record(
    pool: &PgPool,
    kb_id: Option<Uuid>,
    actor_id: Uuid,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    detail: serde_json::Value,
) -> AppResult<()> {
    record_opt(pool, kb_id, Some(actor_id), action, target_kind, target_id, detail).await
}

/// 无人类操作者的系统事件（如 AI 裁决自动合并）走这里：actor 为 NULL。
pub async fn record_opt(
    pool: &PgPool,
    kb_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    action: &str,
    target_kind: &str,
    target_id: Option<Uuid>,
    detail: serde_json::Value,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO audit_events (id, kb_id, actor_id, action, target_kind, target_id, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(actor_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(detail)
    .execute(pool)
    .await?;
    Ok(())
}

/// 审核决策台账：只取 review 域的动作（review./fact./conflict./merge.），服务端分页。
pub async fn review_history(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<AuditEventView>, i64)> {
    const COND: &str = "e.kb_id = $1 AND (e.action LIKE 'review.%' OR e.action LIKE 'fact.%'
                        OR e.action LIKE 'conflict.%' OR e.action LIKE 'merge.%')";
    let rows: Vec<AuditEventView> = sqlx::query_as(&format!(
        "SELECT e.id, e.action, e.target_kind, e.target_id, e.detail,
                u.display_name AS actor_name, e.created_at
         FROM audit_events e LEFT JOIN users u ON u.id = e.actor_id
         WHERE {COND} ORDER BY e.created_at DESC LIMIT $2 OFFSET $3"
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) =
        sqlx::query_as(&format!("SELECT count(*) FROM audit_events e WHERE {COND}"))
            .bind(kb_id)
            .fetch_one(pool)
            .await?;
    Ok((rows, total))
}

pub async fn list_for_kb(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
) -> AppResult<Vec<AuditEventView>> {
    let rows: Vec<AuditEventView> = sqlx::query_as(
        "SELECT e.id, e.action, e.target_kind, e.target_id, e.detail,
                u.display_name AS actor_name, e.created_at
         FROM audit_events e LEFT JOIN users u ON u.id = e.actor_id
         WHERE e.kb_id = $1 ORDER BY e.created_at DESC LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}
