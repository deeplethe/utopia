//! 系统管理 API（仅系统管理员）：部署配置 + 代开账号。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::{Role, User};
use utopia_core::AppError;

use crate::auth::{self, AuthUser};
use crate::error::ApiResult;
use crate::state::AppState;
use uuid::Uuid;

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
    let onto_lang = utopia_store::access::default_ontology_lang(&state.pool).await?;
    let (limits, dflt) = utopia_store::model_limits::list(&state.pool).await?;
    let in_use = utopia_store::model_limits::models_in_use(&state.pool).await?;
    Ok(Json(json!({
        "open_registration": open,
        "worker_concurrency": workers,
        "default_ontology_lang": onto_lang,
        "model_limits": limits,
        "default_model_concurrency": dflt,
        "models_in_use": in_use.into_iter().map(|(b, m, k)| json!({"base_url": b, "model": m, "kind": k})).collect::<Vec<_>>(),
    })))
}

#[derive(Deserialize)]
pub struct DeploymentReq {
    pub open_registration: bool,
    /// 任务 worker 并发（1-256）：**外层兜底**，防任务无限堆积。
    /// 真正的节流是按模型的限额，这个值应当明显大于各模型限额之和
    #[serde(default)]
    pub worker_concurrency: Option<i32>,
    /// 未单独配置的模型走的并发缺省
    #[serde(default)]
    pub default_model_concurrency: Option<i32>,
    /// 单个模型的并发；`max_concurrent` 为 null 表示删掉专属配置、回落到缺省
    #[serde(default)]
    pub model_limit: Option<ModelLimitReq>,
    /// 新建知识库时本体用哪种语言播种。**不是界面语言**——那个在客户端
    #[serde(default)]
    pub default_ontology_lang: Option<String>,
}

#[derive(Deserialize)]
pub struct ModelLimitReq {
    pub base_url: String,
    pub model: String,
    pub max_concurrent: Option<i32>,
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
    // 按模型的限额即时生效：闸门每次调用前读库，发现限额变了就换一把新信号量
    if let Some(l) = &req.default_ontology_lang {
        utopia_store::access::set_default_ontology_lang(&state.pool, l).await?;
    }
    if let Some(n) = req.default_model_concurrency {
        utopia_store::model_limits::set_default(&state.pool, n).await?;
    }
    if let Some(m) = req.model_limit {
        utopia_store::model_limits::set(&state.pool, &m.base_url, &m.model, m.max_concurrent)
            .await?;
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
        return Err(AppError::invalid("bad_email", "Invalid email address").into());
    }
    if req.password.chars().count() < 8 {
        return Err(AppError::invalid(
            "password_too_short",
            "Password must be at least 8 characters",
        )
        .into());
    }
    if req.display_name.trim().is_empty() || req.display_name.chars().count() > 64 {
        return Err(
            AppError::invalid("bad_display_name", "Display name must be 1-64 characters").into(),
        );
    }
    let role = match req.role.as_deref().unwrap_or("editor") {
        "admin" => Role::Admin,
        "editor" => Role::Editor,
        "viewer" => Role::Viewer,
        _ => {
            return Err(AppError::Validation("role must be admin, editor or viewer".into()).into())
        }
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

/// 停用一个账号（软删除，见迁移 0056）。
///
/// **不是 DELETE。** 审计事件、合并日志、改类账本、口径确认的 `actor_id` 都指着
/// 这个人，那些是审计材料——人走了仍然要能回答「当时是谁做的」。停用只断访问：
/// 登录查不到人，已签发的 token 下一次请求也查不到（会话校验走同一个函数），
/// 成员列表里不再出现，而所有归因照旧。
///
/// 两条护栏在 store 层而不是这里：不能停用自己、不能停用最后一个管理员。
/// 放在下面是因为界面挡得住误点，挡不住直接调接口。
pub async fn deactivate_user(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(target): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    utopia_store::accounts::deactivate_user(&state.pool, target, user.id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "user.deactivated",
        "user",
        Some(target),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 把停用的账号放回来。
///
/// **可能失败**：停用期间有人用同一个 email 建了新账号，那个部分唯一索引
/// （`users_email_active_idx`）会挡住恢复。让管理员看见冲突，好过悄悄让两个
/// 在职账号共用一个 email。
pub async fn reactivate_user(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(target): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    utopia_store::accounts::reactivate_user(&state.pool, target).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "user.reactivated",
        "user",
        Some(target),
        serde_json::json!({}),
    )
    .await;
    Ok(Json(serde_json::json!({ "ok": true })))
}
