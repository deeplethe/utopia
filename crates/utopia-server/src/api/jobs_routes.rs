//! 失败任务回队列（#216）。
//!
//! 一个失败任务原本只能通过它属的对象再跑：文档能重抽、来源能重同步；`bootstrap_ontology`、
//! `adjudicate_entities` 这些没有对象可点。余额耗尽（#201 让它第一次就 failed）一批文档
//! 全停，充值之后要逐个点。这里给两个入口：库内（Editor）与全局（管理员），范围可按
//! 种类与失败时间收窄——告警上的「再跑一遍」传的就是那次故障的时间窗。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_store::jobs::RequeueScope;
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

// Both API scopes must apply the same retry policies and report the combined total.
async fn requeue_failed(
    pool: &sqlx::PgPool,
    scope: RequeueScope<'_>,
) -> utopia_core::AppResult<u64> {
    let generic = utopia_store::jobs::requeue_failed(pool, scope).await?;
    let rss = utopia_store::rss_full_content::requeue_failed(pool, scope).await?;
    Ok(generic + rss)
}

#[cfg(test)]
#[path = "jobs_routes_tests.rs"]
mod tests;

#[derive(Deserialize, Default)]
pub struct RequeueBody {
    #[serde(default)]
    pub kind: Option<String>,
    /// 只排这个时刻之后失败的
    #[serde(default)]
    pub failed_since: Option<chrono::DateTime<chrono::Utc>>,
}

pub async fn failed_in_kb(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let failed = utopia_store::jobs::failed_count(&state.pool, Some(kb_id)).await?;
    Ok(Json(json!({ "failed": failed })))
}

pub async fn requeue_in_kb(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(body): Json<RequeueBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let requeued = requeue_failed(
        &state.pool,
        RequeueScope {
            kb_id: Some(kb_id),
            kind: body.kind.as_deref(),
            failed_since: body.failed_since,
        },
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "jobs.requeued",
        "kb",
        Some(kb_id),
        json!({ "requeued": requeued, "kind": body.kind, "failed_since": body.failed_since }),
    )
    .await;
    Ok(Json(json!({ "requeued": requeued })))
}

/// 全局重排：系统级告警（没有库的）从这里走。只给管理员——它碰的是所有库的任务
pub async fn requeue_all(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(body): Json<RequeueBody>,
) -> ApiResult<Json<serde_json::Value>> {
    if !user.is_admin {
        return Err(utopia_core::AppError::Forbidden.into());
    }
    let requeued = requeue_failed(
        &state.pool,
        RequeueScope {
            kb_id: None,
            kind: body.kind.as_deref(),
            failed_since: body.failed_since,
        },
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "jobs.requeued",
        "system",
        None,
        json!({ "requeued": requeued, "kind": body.kind, "failed_since": body.failed_since }),
    )
    .await;
    Ok(Json(json!({ "requeued": requeued })))
}
