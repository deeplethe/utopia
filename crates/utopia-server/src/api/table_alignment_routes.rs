use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;
use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use utopia_core::{models::Role, AppError};
use uuid::Uuid;

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let proposals = utopia_store::table_alignments::list(&state.pool, kb_id).await?;
    let last_run = utopia_store::exploration_runs::recent(&state.pool, kb_id, 1)
        .await?
        .into_iter()
        .next();
    Ok(Json(json!({"items":proposals,"last_run":last_run})))
}

#[derive(Deserialize)]
pub struct Decision {
    pub key: String,
    pub version: Uuid,
    pub status: String,
}

pub async fn decide(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<Decision>,
) -> ApiResult<Json<Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if !matches!(req.status.as_str(), "adopted" | "rejected") {
        return Err(AppError::invalid("bad_status", "status must be adopted or rejected").into());
    }
    crate::table_exploration::decide(
        &state,
        kb_id,
        &req.key,
        req.version,
        req.status == "adopted",
        user.id,
    )
    .await?;
    Ok(Json(json!({"ok":true})))
}
