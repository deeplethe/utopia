use axum::extract::State;
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar};
use utopia_core::models::{User, Workspace};
use utopia_core::AppError;
use serde::Deserialize;
use serde_json::json;

use crate::auth::{self, AuthUser};
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegisterReq {
    pub email: String,
    pub password: String,
    pub display_name: String,
    pub org_name: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub email: String,
    pub password: String,
}

fn validate_register(req: &RegisterReq) -> Result<(), AppError> {
    if !req.email.contains('@') || req.email.len() > 254 {
        return Err(AppError::Validation("Invalid email address".into()));
    }
    if req.password.chars().count() < 8 {
        return Err(AppError::Validation(
            "Password must be at least 8 characters".into(),
        ));
    }
    if req.display_name.trim().is_empty() || req.display_name.chars().count() > 64 {
        return Err(AppError::Validation(
            "Display name must be 1-64 characters".into(),
        ));
    }
    Ok(())
}

pub async fn register(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterReq>,
) -> ApiResult<(CookieJar, Json<serde_json::Value>)> {
    validate_register(&req)?;
    let hash = auth::hash_password(&req.password)?;
    let org_name = req
        .org_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    // 注册开关：库内配置（/admin 可切）优先；建库前（首用户引导）总是放行
    let open = utopia_store::access::open_registration(&state.pool)
        .await
        .unwrap_or(state.open_registration);
    let (user, workspace): (User, Workspace) = utopia_store::accounts::register(
        &state.pool,
        req.email.trim(),
        &hash,
        req.display_name.trim(),
        org_name,
        open,
    )
    .await?;

    let token = auth::issue_token(&state, user.id)?;
    let jar = jar.add(auth::auth_cookie(token.clone()));
    Ok((
        jar,
        Json(json!({ "user": user, "workspace": workspace, "token": token })),
    ))
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginReq>,
) -> ApiResult<(CookieJar, Json<serde_json::Value>)> {
    let user = utopia_store::accounts::find_user_by_email(&state.pool, req.email.trim())
        .await?
        .ok_or(AppError::Unauthorized)?;
    if !auth::verify_password(&req.password, &user.password_hash) {
        return Err(AppError::Unauthorized.into());
    }
    let token = auth::issue_token(&state, user.id)?;
    let jar = jar.add(auth::auth_cookie(token.clone()));
    Ok((jar, Json(json!({ "user": user, "token": token }))))
}

pub async fn logout(jar: CookieJar) -> (CookieJar, Json<serde_json::Value>) {
    let jar = jar.remove(Cookie::from(auth::COOKIE_NAME));
    (jar, Json(json!({ "ok": true })))
}

pub async fn me(AuthUser(user): AuthUser) -> Json<User> {
    Json(user)
}

#[derive(Deserialize)]
pub struct UpdateMeReq {
    pub display_name: String,
}

/// 个人资料：改显示名。
pub async fn update_me(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<UpdateMeReq>,
) -> ApiResult<Json<User>> {
    let name = req.display_name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(AppError::Validation("Display name must be 1-64 characters".into()).into());
    }
    let updated = utopia_store::accounts::update_display_name(&state.pool, user.id, name).await?;
    Ok(Json(updated))
}

#[derive(Deserialize)]
pub struct ChangePasswordReq {
    pub current_password: String,
    pub new_password: String,
}

/// 改密码：验旧密 → 哈希新密。旧密错误与"未登录"区分开，前端能给出准确提示。
pub async fn change_password(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<ChangePasswordReq>,
) -> ApiResult<Json<serde_json::Value>> {
    if !auth::verify_password(&req.current_password, &user.password_hash) {
        return Err(AppError::Validation("Current password is incorrect".into()).into());
    }
    if req.new_password.chars().count() < 8 {
        return Err(AppError::Validation("Password must be at least 8 characters".into()).into());
    }
    let hash = auth::hash_password(&req.new_password)?;
    utopia_store::accounts::update_password(&state.pool, user.id, &hash).await?;
    Ok(Json(json!({ "ok": true })))
}
