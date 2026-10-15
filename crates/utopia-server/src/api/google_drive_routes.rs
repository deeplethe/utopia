//! Google Drive 来源的连接与选文件夹（0068）。
//!
//! **连接由来源的编辑者本人完成。** 他在来源设置里点「连接」，我们记下一次流程
//! （state、PKCE verifier、是谁、哪个来源），把他送到 Google 的同意页；回调时这个
//! 浏览器里的会话必须还是他，而且他对那个库还得是 Editor。不核对的话，有人可以
//! 自己发起一次流程，再诱使别人点完同意——别人的 Drive 就接进了他的来源。
//!
//! 刷新令牌只进不出：存进来源配置的 `refresh_token`（封印），文件夹浏览器的请求
//! 在服务器上拿它换访问令牌，浏览器从头到尾拿不到任何一枚 Google 令牌。

use axum::extract::{Path, Query, State};
use axum::response::Html;
use axum::Json;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use utopia_core::models::{Role, Source, SourceKind};
use utopia_core::AppError;
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::{self, AuthUser};
use crate::error::{ApiErr, ApiResult};
use crate::google_drive::{self, Drive, FolderEntry, OAuthConfig};
use crate::state::AppState;

/// 一次连接流程能活多久：同意页上停十分钟还没点完，就当放弃了
const FLOW_TTL_SECS: u64 = 600;
/// 同时在途的连接流程上限，理由同 SSO：每一次 `connect` 都落一行
const MAX_PENDING_FLOWS: i64 = 1000;
/// 一个来源最多选多少个文件夹。选得更多的，选它们共同的上级
const MAX_FOLDERS: usize = 50;

fn configured() -> Result<OAuthConfig, AppError> {
    google_drive::oauth_config().ok_or_else(|| {
        AppError::invalid(
            "google_drive_not_configured",
            "Google Drive is not configured on this server",
        )
    })
}

/// 取一条 Google Drive 来源，确认它属于路径上的这个库
async fn drive_source(state: &AppState, kb_id: Uuid, source_id: Uuid) -> Result<Source, AppError> {
    let source = utopia_store::sources::get(&state.pool, source_id).await?;
    if source.kb_id != kb_id || SourceKind::parse(&source.kind) != Some(SourceKind::GoogleDrive) {
        return Err(AppError::NotFound);
    }
    Ok(source)
}

/// 界面据此决定要不要提供这种来源的连接按钮
pub async fn status(AuthUser(_): AuthUser) -> Json<Value> {
    Json(json!({ "enabled": google_drive::oauth_config().is_some() }))
}

/// 开始一次连接：回给界面一个 Google 同意页的地址，界面在弹窗里打开它
pub async fn connect(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    drive_source(&state, kb_id, source_id).await?;
    let cfg = configured()?;

    sqlx::query("DELETE FROM google_drive_flows WHERE expires_at < now()")
        .execute(&state.pool)
        .await?;
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM google_drive_flows")
        .fetch_one(&state.pool)
        .await?;
    if pending >= MAX_PENDING_FLOWS {
        return Err(AppError::invalid(
            "google_drive_busy",
            "Too many Google Drive connections are in progress; try again in a few minutes",
        )
        .into());
    }

    let flow_state = auth::generate_jwt_secret();
    let verifier = auth::generate_jwt_secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    sqlx::query(
        "INSERT INTO google_drive_flows (state, verifier, user_id, source_id, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(&flow_state)
    .bind(&verifier)
    .bind(user.id)
    .bind(source_id)
    .bind(FLOW_TTL_SECS as f64)
    .execute(&state.pool)
    .await?;

    let mut url =
        reqwest::Url::parse(google_drive::AUTHORIZE_URL).map_err(|e| AppError::Other(e.into()))?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", cfg.client_id.as_str()),
        ("redirect_uri", cfg.redirect.as_str()),
        ("scope", google_drive::SCOPES),
        ("state", flow_state.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        // 刷新令牌只在 offline + 每次都请同意时才保证发：重连一个撤过授权的来源也一样
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("include_granted_scopes", "false"),
    ]);
    Ok(Json(json!({ "url": url.as_str() })))
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    /// 人在同意页点了取消
    error: Option<String>,
}

/// 回调落在弹窗里：成功与失败都回一张不带脚本的小页面。界面那头在轮询来源，
/// 看见 `account_email` 出现就关掉弹窗
pub async fn callback(
    State(state): State<AppState>,
    user: Result<AuthUser, ApiErr>,
    Query(q): Query<Callback>,
) -> Html<String> {
    match finish(&state, user.ok().map(|AuthUser(u)| u), q).await {
        Ok(()) => page(
            "Google Drive connected",
            "You can close this window and choose the folders to sync.",
        ),
        Err((code, reason)) => {
            tracing::warn!(code, reason = %reason, "Google Drive 连接被拒");
            page(
                "Google Drive was not connected",
                match code {
                    "cancelled" => "The consent screen was cancelled. Close this window and try again.",
                    "session" => "This window is not signed in to Utopia as the person who started the connection. Close it and start again from the source settings.",
                    "expired" => "This connection attempt has expired. Close this window and start again.",
                    "scope" => "Google did not grant read access to Drive. Close this window, connect again and allow it.",
                    _ => "Google could not be reached or refused the request. Close this window and try again.",
                },
            )
        }
    }
}

async fn finish(
    state: &AppState,
    user: Option<utopia_core::models::User>,
    q: Callback,
) -> Result<(), (&'static str, String)> {
    let db = |e: sqlx::Error| ("unavailable", format!("database: {e}"));
    let (Some(code), Some(flow_state)) = (q.code, q.state) else {
        return Err(match q.error {
            Some(e) => ("cancelled", format!("google returned {e}")),
            None => ("invalid", "callback without code or state".into()),
        });
    };
    // 一次性：先删后用，同一个 state 第二次到就什么都取不出来
    let flow: Option<(String, Uuid, Uuid)> = sqlx::query_as(
        "DELETE FROM google_drive_flows WHERE state = $1 AND expires_at > now()
         RETURNING verifier, user_id, source_id",
    )
    .bind(&flow_state)
    .fetch_optional(&state.pool)
    .await
    .map_err(db)?;
    let (verifier, flow_user, source_id) =
        flow.ok_or(("expired", "unknown or expired state".to_string()))?;
    if let Some(e) = q.error {
        return Err(("cancelled", format!("google returned {e}")));
    }
    let user = user.filter(|u| u.id == flow_user).ok_or((
        "session",
        "callback session is not the user who started it".to_string(),
    ))?;
    let source = utopia_store::sources::get(&state.pool, source_id)
        .await
        .map_err(|e| ("invalid", format!("source: {e}")))?;
    // 发起之后的这几分钟里权限可能被收回了
    require_kb(state, &user, source.kb_id, Role::Editor)
        .await
        .map_err(|_| {
            (
                "session",
                "user is no longer an editor of the knowledge base".to_string(),
            )
        })?;

    let cfg = google_drive::oauth_config().ok_or(("unavailable", "not configured".to_string()))?;
    let grant = google_drive::exchange_code(&cfg, &code, &verifier)
        .await
        .map_err(|e| {
            let msg = format!("{e:#}");
            (
                if msg.contains("read access to Drive") {
                    "scope"
                } else {
                    "unavailable"
                },
                msg,
            )
        })?;

    let mut config = source.config.clone();
    if !config.is_object() {
        config = json!({});
    }
    config["refresh_token"] = json!(grant.refresh_token);
    config["account_email"] = json!(grant.account_email);
    // 列表里看不见令牌，界面凭这一项知道「连上了」：没验证过的邮箱不显示，不能拿它当标志
    config["connected_at"] = json!(chrono::Utc::now().to_rfc3339());
    utopia_store::sources::update(&state.pool, source_id, None, Some(&config), None, None)
        .await
        .map_err(|e| ("unavailable", format!("saving the grant: {e}")))?;
    state.emit_source(source.kb_id);
    // 审计不落令牌，只记连上的是哪个 Google 账号
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(source.kb_id),
        user.id,
        "source.google_drive_connected",
        "source",
        Some(source_id),
        json!({ "account_email": grant.account_email }),
    )
    .await;
    Ok(())
}

/// 断开：删掉刷新令牌，并请 Google 把它作废。作废失败不拦着——库里已经没有它了
pub async fn disconnect(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = drive_source(&state, kb_id, source_id).await?;
    let mut config = source.config.clone();
    let token = config["refresh_token"].as_str().map(String::from);
    if let Some(obj) = config.as_object_mut() {
        obj.remove("refresh_token");
        obj.remove("account_email");
        obj.remove("connected_at");
    }
    utopia_store::sources::update(&state.pool, source_id, None, Some(&config), None, None).await?;
    if let Some(token) = token {
        let revoked = reqwest::Client::new()
            .post("https://oauth2.googleapis.com/revoke")
            .form(&[("token", token.as_str())])
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await;
        if let Err(e) = revoked {
            tracing::warn!(error = %e, "Google 刷新令牌作废失败，库里已删除");
        }
    }
    state.emit_source(kb_id);
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "source.google_drive_disconnected",
        "source",
        Some(source_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct FoldersQuery {
    /// 不给 = 第一层（我的云端硬盘、与我共享、各个共享云端硬盘）
    parent: Option<String>,
}

/// 文件夹浏览器：以连上的那个人的身份列子文件夹
pub async fn folders(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<FoldersQuery>,
) -> ApiResult<Json<Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = drive_source(&state, kb_id, source_id).await?;
    let cfg = configured()?;
    let refresh = source.config["refresh_token"]
        .as_str()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            AppError::invalid(
                "google_drive_not_connected",
                "Connect a Google account first",
            )
        })?;
    let unavailable = |e: anyhow::Error| {
        AppError::invalid_detail(
            "google_drive_unavailable",
            "Google Drive could not be read",
            format!("{e:#}"),
        )
    };
    let drive = Drive::connect(&cfg, refresh).await.map_err(unavailable)?;
    let folders: Vec<FolderEntry> = match q.parent.as_deref().filter(|p| !p.is_empty()) {
        None => drive.roots().await,
        Some(parent) => drive.child_folders(parent).await,
    }
    .map_err(unavailable)?;
    Ok(Json(json!({ "folders": folders })))
}

#[derive(Deserialize)]
pub struct SelectBody {
    folders: Vec<FolderEntry>,
}

/// 存下选中的文件夹，并立即同步一次。换了文件夹，上一批里不在新范围内的文档
/// 会在这一轮被标成「不在来源中」，删不删由人决定，与别的来源一样
pub async fn select(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SelectBody>,
) -> ApiResult<Json<Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = drive_source(&state, kb_id, source_id).await?;
    if body.folders.len() > MAX_FOLDERS {
        return Err(AppError::invalid(
            "google_drive_too_many_folders",
            format!("Choose at most {MAX_FOLDERS} folders"),
        )
        .into());
    }
    if body.folders.iter().any(|f| !google_drive::valid_id(&f.id)) {
        return Err(AppError::invalid("google_drive_invalid_folder", "Invalid folder id").into());
    }
    let mut config = source.config.clone();
    if !config.is_object() {
        config = json!({});
    }
    config["folders"] = json!(body.folders);
    let updated =
        utopia_store::sources::update(&state.pool, source_id, None, Some(&config), None, None)
            .await?;
    let connected = updated.config["refresh_token"]
        .as_str()
        .is_some_and(|t| !t.is_empty());
    if connected && !body.folders.is_empty() {
        let _ = utopia_store::sources::mark_queued(&state.pool, source_id).await;
        utopia_store::jobs::enqueue(
            &state.pool,
            "sync_source",
            json!({ "source_id": source_id }),
        )
        .await?;
    }
    state.emit_source(kb_id);
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "source.updated",
        "source",
        Some(source_id),
        json!({ "google_drive_folders": body.folders.len() }),
    )
    .await;
    Ok(Json(json!({ "source": updated.without_secrets() })))
}

/// 一张小页面，不带脚本。字符串都是这里写死的，不拼任何请求里来的东西
fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"></head>\
         <body style=\"font-family:system-ui,sans-serif;max-width:32rem;margin:4rem auto;padding:0 1rem\">\
         <h1 style=\"font-size:1.25rem\">{title}</h1><p>{body}</p></body></html>"
    ))
}
