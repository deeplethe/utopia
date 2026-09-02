//! 个人访问令牌的发放与撤销（0014）。
//!
//! **走的是账户级路由,不是知识库级**——令牌属于人,而人可以进好几个库。
//! 哪几个库归令牌自己的 `kb_ids` 管,那是收窄,不是授权。

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct IssueReq {
    pub name: String,
    /// read | write。缺省只读——要让 agent 写进账本,得显式勾
    #[serde(default = "default_scope")]
    pub scope: String,
    /// 缺省 = 这个人能进的全部库
    #[serde(default)]
    pub kb_ids: Option<Vec<Uuid>>,
    /// 多少天后过期。缺省 90 天;显式给 0 表示不过期
    #[serde(default = "default_days")]
    pub expires_in_days: i64,
}
fn default_scope() -> String {
    "read".into()
}
/// 90 天。**不过期是能选的,但不是缺省**——一枚配在别人笔记本上的钥匙,
/// 忘了它存在是常态
fn default_days() -> i64 {
    90
}

/// 发一枚。**明文只在这一次的响应里出现**,之后库里只有哈希。
pub async fn issue(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(req): Json<IssueReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let expires_at = (req.expires_in_days > 0)
        .then(|| chrono::Utc::now() + chrono::Duration::days(req.expires_in_days));
    let (view, plain) = utopia_store::tokens::issue(
        &state.pool,
        user.id,
        &req.name,
        &req.scope,
        req.kb_ids.as_deref(),
        expires_at,
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "token.issued",
        "personal_token",
        Some(view.id),
        json!({ "name": view.name, "scope": view.scope }),
    )
    .await;
    // `token` 这个字段只在这里出现一次。列表接口永远给不出它
    Ok(Json(json!({ "token": plain, "info": view })))
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let tokens = utopia_store::tokens::list(&state.pool, user.id).await?;
    Ok(Json(json!({ "tokens": tokens })))
}

pub async fn revoke(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(token_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::tokens::revoke(&state.pool, user.id, token_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "token.revoked",
        "personal_token",
        Some(token_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}
