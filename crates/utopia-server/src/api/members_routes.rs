//! 工作区成员管理。
//! 规则：查看成员 = viewer+；改角色/移除 = admin+；涉及 owner 角色的授予/剥夺 = 仅 owner；
//! 永远保证工作区至少剩一个 owner。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::{MemberView, OrgUser, Role};
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<Vec<MemberView>>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Viewer)
        .await?;
    Ok(Json(
        utopia_store::members::list(&state.pool, workspace_id).await?,
    ))
}

/// 部署内全部用户（供成员选人器；同一公司内可见）。
pub async fn org_users(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Vec<OrgUser>>> {
    Ok(Json(
        utopia_store::members::org_users(&state.pool, user.org_id).await?,
    ))
}

#[derive(Deserialize)]
pub struct SetRoleReq {
    pub role: String,
}

pub async fn set_role(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((workspace_id, target_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SetRoleReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let new_role = Role::parse(&req.role).ok_or_else(|| {
        AppError::Validation("Role must be one of owner/admin/editor/viewer".into())
    })?;
    let caller_role =
        utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin)
            .await?;

    let target_role =
        utopia_store::members::current_role(&state.pool, workspace_id, target_id).await?;

    // owner 角色的授予或剥夺，只有 owner 能做
    let touches_owner = new_role == Role::Owner || target_role == Some(Role::Owner);
    if touches_owner && caller_role != Role::Owner {
        return Err(AppError::Forbidden.into());
    }
    // 不能把最后一个 owner 降级
    if target_role == Some(Role::Owner)
        && new_role != Role::Owner
        && utopia_store::members::owner_count(&state.pool, workspace_id).await? <= 1
    {
        return Err(AppError::invalid("last_owner_demote", "Cannot demote the last owner").into());
    }

    utopia_store::members::set_role(&state.pool, workspace_id, target_id, new_role).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((workspace_id, target_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    let caller_role =
        utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin)
            .await?;
    let target_role = utopia_store::members::current_role(&state.pool, workspace_id, target_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if target_role == Role::Owner {
        if caller_role != Role::Owner {
            return Err(AppError::Forbidden.into());
        }
        if utopia_store::members::owner_count(&state.pool, workspace_id).await? <= 1 {
            return Err(
                AppError::invalid("last_owner_remove", "Cannot remove the last owner").into(),
            );
        }
    }

    utopia_store::members::remove(&state.pool, workspace_id, target_id).await?;
    Ok(Json(json!({ "ok": true })))
}
