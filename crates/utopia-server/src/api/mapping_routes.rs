//! 数据映射 API：口径的列表、改写与改版历史。
//!
//! 审批（`decide`）留在 `review_routes`——那条端点已经在写审计流水
//! （`mapping.decided`），换个界面调它即可，没有理由为了搬页面而搬端点。

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

/// 一页多少条。口径比审阅队列密（一行就是一个定义），一页给得起 25 条
const MAPPING_PAGE: i64 = 25;

#[derive(Deserialize)]
pub struct ListQuery {
    /// proposed | confirmed | rejected；缺省 = 全部
    status: Option<String>,
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

/// 一页口径 + 三种状态各多少。
///
/// **Viewer 就能看。** 口径是「这个数怎么算」，问数的答案直接由它决定——
/// 看得见答案却看不见口径，等于要人信一个不给看的算法。改（`revise`）
/// 才要 Editor。
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let status = q.status.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = status {
        if !matches!(s, "proposed" | "confirmed" | "rejected") {
            return Err(AppError::invalid(
                "bad_status",
                "status must be proposed, confirmed or rejected",
            )
            .into());
        }
    }
    let needle = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let limit = q.limit.unwrap_or(MAPPING_PAGE).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);

    let (items, total) =
        utopia_store::mappings::page(&state.pool, kb_id, status, needle, limit, offset).await?;
    let (proposed, confirmed, rejected) =
        utopia_store::mappings::status_counts(&state.pool, kb_id).await?;
    Ok(Json(json!({
        "items": items,
        "total": total,
        "counts": { "proposed": proposed, "confirmed": confirmed, "rejected": rejected },
    })))
}

#[derive(Deserialize)]
pub struct ReviseReq {
    table_name: Option<String>,
    expr: Option<String>,
    sql: Option<String>,
    unit: Option<String>,
    summary: Option<String>,
    #[serde(default)]
    derived: bool,
}

/// 改一条口径。
///
/// **在此之前 `mappings::revise` 是零调用的**：函数在、留痕表在、没有路由。
/// 于是口径确认之后就再没人改得动，也没人看得见——问数照着它算，人却够不着。
pub async fn revise(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, mapping_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<ReviseReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    // 空白等于没填：前端清空一个输入框传来的是 ""，落库该是 NULL 而不是空串，
    // 否则「有没有配 expr」这个判断要同时问 IS NULL 和 = ''
    let clean = |s: &Option<String>| -> Option<String> {
        s.as_deref()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    let (table_name, expr, sql, unit, summary) = (
        clean(&req.table_name),
        clean(&req.expr),
        clean(&req.sql),
        clean(&req.unit),
        clean(&req.summary),
    );
    if table_name.is_none() && expr.is_none() && sql.is_none() {
        return Err(AppError::invalid(
            "empty_mapping",
            "A mapping needs at least one of table, expression or SQL",
        )
        .into());
    }
    utopia_store::mappings::revise(
        &state.pool,
        kb_id,
        mapping_id,
        table_name.as_deref(),
        expr.as_deref(),
        sql.as_deref(),
        unit.as_deref(),
        summary.as_deref(),
        req.derived,
        user.id,
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "mapping.revised",
        "concept_mapping",
        Some(mapping_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 一条口径的改版历史。0006 说留痕是为了答得出「上季度这个数是怎么算的」，
/// 这是那句话的兑现处。
pub async fn revisions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, mapping_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let rows = utopia_store::mappings::revisions(&state.pool, kb_id, mapping_id).await?;
    Ok(Json(json!({ "revisions": rows })))
}
