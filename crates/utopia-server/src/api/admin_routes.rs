//! 系统管理 API（仅系统管理员）：部署配置 + 代开账号。

use axum::extract::State;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::{Role, User};
use utopia_core::AppError;

use crate::auth::{self, AuthUser};
use crate::error::ApiResult;
use crate::state::AppState;

fn require_admin(user: &User) -> Result<(), AppError> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

pub async fn get_deployment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let open = utopia_store::access::open_registration(&state.pool).await?;
    let workers = utopia_store::access::worker_concurrency(&state.pool).await?;
    Ok(Json(json!({ "open_registration": open, "worker_concurrency": workers })))
}

#[derive(Deserialize)]
pub struct DeploymentReq {
    pub open_registration: bool,
    /// 任务 worker 并发数（1-32）；缺省不改
    #[serde(default)]
    pub worker_concurrency: Option<i32>,
}

pub async fn put_deployment(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<DeploymentReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    utopia_store::access::set_open_registration(&state.pool, req.open_registration).await?;
    if let Some(n) = req.worker_concurrency {
        utopia_store::access::set_worker_concurrency(&state.pool, n).await?;
        // 热生效：调度循环每轮读这个值,无需重启
        state
            .worker_concurrency
            .store(n as usize, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct CreateUserReq {
    pub email: String,
    pub display_name: String,
    pub password: String,
    /// 部署角色：admin | editor | viewer
    #[serde(default)]
    pub role: Option<String>,
}

/// 管理员代开账号（注册关闭后的唯一入口）。
pub async fn create_user(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<CreateUserReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    if !req.email.contains('@') || req.email.len() > 254 {
        return Err(AppError::Validation("Invalid email address".into()).into());
    }
    if req.password.chars().count() < 8 {
        return Err(
            AppError::Validation("Password must be at least 8 characters".into()).into(),
        );
    }
    if req.display_name.trim().is_empty() || req.display_name.chars().count() > 64 {
        return Err(
            AppError::Validation("Display name must be 1-64 characters".into()).into(),
        );
    }
    let role = match req.role.as_deref().unwrap_or("editor") {
        "admin" => Role::Admin,
        "editor" => Role::Editor,
        "viewer" => Role::Viewer,
        _ => return Err(AppError::Validation("role must be admin, editor or viewer".into()).into()),
    };
    let hash = auth::hash_password(&req.password)?;
    let created = utopia_store::accounts::admin_create_user(
        &state.pool,
        req.email.trim(),
        &hash,
        req.display_name.trim(),
        role,
    )
    .await?;
    Ok(Json(json!({ "user": created })))
}
