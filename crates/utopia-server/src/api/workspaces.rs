use axum::extract::{Path, State};
use axum::Json;
use utopia_core::models::{Role, Workspace};
use utopia_core::AppError;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct NameReq {
    pub name: String,
}

fn validate_name(name: &str) -> Result<&str, AppError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(AppError::Validation("Name must be 1-64 characters".into()));
    }
    Ok(name)
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Vec<Workspace>>> {
    let list = utopia_store::workspaces::list_for_user(&state.pool, user.id).await?;
    Ok(Json(list))
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<NameReq>,
) -> ApiResult<Json<Workspace>> {
    let name = validate_name(&req.name)?;
    let ws = utopia_store::workspaces::create(&state.pool, user.org_id, user.id, name).await?;
    Ok(Json(ws))
}

pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let role = utopia_store::workspaces::require_role(&state.pool, user.id, id, Role::Viewer).await?;
    let ws = utopia_store::workspaces::get(&state.pool, id).await?;
    Ok(Json(json!({ "workspace": ws, "role": role })))
}

pub async fn rename(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<NameReq>,
) -> ApiResult<Json<Workspace>> {
    let name = validate_name(&req.name)?;
    utopia_store::workspaces::require_role(&state.pool, user.id, id, Role::Admin).await?;
    let ws = utopia_store::workspaces::rename(&state.pool, id, name).await?;
    Ok(Json(ws))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, id, Role::Owner).await?;
    utopia_store::workspaces::delete(&state.pool, id).await?;
    Ok(Json(json!({ "ok": true })))
}
