use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::retrieval;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SearchReq {
    pub q: String,
    #[serde(default)]
    pub top_k: Option<usize>,
    /// 记录轴（0019）：按**那一刻库里有的东西**检索。YYYY-MM-DD 或 RFC3339。
    /// 命中的是当时活着的块、当时还没被删的文档
    #[serde(default)]
    pub as_of: Option<String>,
}

pub async fn search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<SearchReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let q = req.q.trim();
    if q.is_empty() {
        return Err(AppError::invalid("empty_query", "Search query cannot be empty").into());
    }
    let kb = utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;

    let top_k = req.top_k.unwrap_or(10).min(50);
    let as_of = super::graph_routes::parse_instant("as_of", req.as_of.as_deref())?;
    let chunks = retrieval::hybrid(&state, kb_id, kb.workspace_id, q, top_k, as_of).await?;
    Ok(Json(json!({ "results": chunks })))
}
