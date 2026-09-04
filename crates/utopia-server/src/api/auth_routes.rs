use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::User;
use utopia_core::AppError;

use super::kbs::{install_packs, DEFAULT_PACK};
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
        return Err(AppError::invalid("bad_email", "Invalid email address"));
    }
    if req.password.chars().count() < 8 {
        return Err(AppError::invalid(
            "password_too_short",
            "Password must be at least 8 characters",
        ));
    }
    if req.display_name.trim().is_empty() || req.display_name.chars().count() > 64 {
        return Err(AppError::invalid(
            "bad_display_name",
            "Display name must be 1-64 characters",
        ));
    }
    Ok(())
}

pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
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
    let utopia_store::accounts::Registered {
        user,
        workspace,
        general_kb,
    } = utopia_store::accounts::register(
        &state.pool,
        req.email.trim(),
        &hash,
        req.display_name.trim(),
        org_name,
        open,
    )
    .await?;

    /* 首个用户那个 General 库也要有词汇表（#322）。
    建库对话框里 schema.org 是预勾选的默认（0008、0009），而这条路径绕过了
    对话框——于是每个部署的第一个库、也就是新用户落地的那个，本体是空的：
    没有 domain/range，抽取的方向就定不住，事实退化成 related_to，
    而这正是 0008 装包要解决的事。

    **装不上不能挡住注册。** 事务已经提交，账号已经存在；这一步失败只是
    回到今天的行为（库空着，日后手动装包再重跑类型消解即可，见 0009），
    让人注册不进来则是把小事变成大事 */
    if let Some(kb_id) = general_kb {
        if install_packs(&state, kb_id, user.id, &[DEFAULT_PACK.to_string()])
            .await
            .is_err()
        {
            tracing::warn!(kb_id = %kb_id, pack = DEFAULT_PACK, "默认库的冷启动本体包没装上");
        }
    }

    let token = auth::issue_token(&state, user.id)?;
    let secure = auth::behind_tls(&headers, state.cookie_secure);
    let jar = jar.add(auth::auth_cookie(token.clone(), secure));
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "auth.register",
        "user",
        Some(user.id),
        json!({ "email": user.email, "is_admin": user.is_admin }),
    )
    .await;
    Ok((
        jar,
        Json(json!({ "user": user, "workspace": workspace, "token": token })),
    ))
}

/// 登录失败留痕：没有账号可归属，actor 记 NULL，尝试的邮箱进 detail。
/// 区分「邮箱不存在」与「密码不对」——同一 IP 上大量前者是邮箱枚举，
/// 集中在一个账号上的后者是撞库，两种攻击的形状不一样。台账仅管理员可读，
/// 不存在借此探测账号是否注册的问题。
async fn record_login_failure(state: &AppState, email: &str, reason: &str) {
    let _ = utopia_store::audit::record_opt(
        &state.pool,
        None,
        None,
        "auth.login_failed",
        "user",
        None,
        json!({ "email": email, "reason": reason }),
    )
    .await;
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<LoginReq>,
) -> ApiResult<(CookieJar, Json<serde_json::Value>)> {
    let email = req.email.trim();
    let Some(user) = utopia_store::accounts::find_user_by_email(&state.pool, email).await? else {
        record_login_failure(&state, email, "unknown_email").await;
        return Err(AppError::Unauthorized.into());
    };
    if !auth::verify_password(&req.password, &user.password_hash) {
        record_login_failure(&state, email, "bad_password").await;
        return Err(AppError::Unauthorized.into());
    }
    let token = auth::issue_token(&state, user.id)?;
    let secure = auth::behind_tls(&headers, state.cookie_secure);
    let jar = jar.add(auth::auth_cookie(token.clone(), secure));
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "auth.login",
        "user",
        Some(user.id),
        json!({}),
    )
    .await;
    Ok((jar, Json(json!({ "user": user, "token": token }))))
}

/// 登出不要求有效会话——cookie 过期时也必须能清掉它，否则前端就卡在一个
/// 死会话上。所以这里自己解 token 取身份，解不出就只清 cookie 不留痕。
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> (CookieJar, Json<serde_json::Value>) {
    if let Some(user_id) = jar
        .get(auth::COOKIE_NAME)
        .and_then(|c| auth::decode_user_id(&state, c.value()).ok())
    {
        let _ = utopia_store::audit::record(
            &state.pool,
            None,
            user_id,
            "auth.logout",
            "user",
            Some(user_id),
            json!({}),
        )
        .await;
    }
    let secure = auth::behind_tls(&headers, state.cookie_secure);
    let jar = jar.remove(auth::clear_auth_cookie(secure));
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
        return Err(
            AppError::invalid("bad_display_name", "Display name must be 1-64 characters").into(),
        );
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
        return Err(AppError::invalid("wrong_password", "Current password is incorrect").into());
    }
    if req.new_password.chars().count() < 8 {
        return Err(AppError::invalid(
            "password_too_short",
            "Password must be at least 8 characters",
        )
        .into());
    }
    let hash = auth::hash_password(&req.new_password)?;
    utopia_store::accounts::update_password(&state.pool, user.id, &hash).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "auth.password_changed",
        "user",
        Some(user.id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}
