//! 来源管理 API + 文本推送摄入（ingest）。

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::{Role, SOURCE_SECRET_KEYS};
use uuid::Uuid;

use super::graph_routes::require_kb;
use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

/// 生成 api 来源的推送密钥。
fn new_ingest_token() -> String {
    format!("utp_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// 取一条来源，并确认它属于路径上的这个库。
///
/// `require_kb` 只查人对库的权限；来源 id 是另一个维度——不比对的话，A 库的
/// Editor 拿着 B 库来源的 id 就能同步、清理、删除它。不属于就当不存在（404），
/// 与 `get_token` 一直以来的做法一致
async fn source_in_kb(
    state: &AppState,
    kb_id: Uuid,
    source_id: Uuid,
) -> ApiResult<utopia_core::models::Source> {
    let source = utopia_store::sources::get(&state.pool, source_id).await?;
    if source.kb_id != kb_id {
        return Err(utopia_core::AppError::NotFound.into());
    }
    Ok(source)
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let sources = utopia_store::sources::list(&state.pool, kb_id).await?;
    Ok(Json(json!({ "sources": sources })))
}

#[derive(Deserialize)]
pub struct CreateBody {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub sync_interval_minutes: Option<i32>,
    /// 标准 5 段 cron（与 interval 互斥，二者传其一）
    #[serde(default)]
    pub sync_cron: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(body): Json<CreateBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = utopia_store::sources::create(
        &state.pool,
        kb_id,
        &body.kind,
        &body.name,
        &body.config,
        body.icon.as_deref(),
        body.sync_interval_minutes,
        body.sync_cron.as_deref(),
    )
    .await?;
    // 拉取型来源建好立即同步一次（有配置即产出，无需等下一个调度周期）
    if matches!(source.kind.as_str(), "url" | "rss" | "custom") {
        let _ = utopia_store::sources::mark_queued(&state.pool, source.id).await;
        utopia_store::jobs::enqueue(
            &state.pool,
            "sync_source",
            json!({ "source_id": source.id }),
        )
        .await?;
    }
    // api 来源：生成专属推送密钥（此后可随时经 get_token 查看）
    let mut ingest_token: Option<String> = None;
    if source.kind == "api" {
        let token = new_ingest_token();
        utopia_store::sources::set_ingest_token(&state.pool, source.id, &token).await?;
        ingest_token = Some(token);
    }
    state.emit_source(kb_id);
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "source.created",
        "source",
        Some(source.id),
        json!({ "kind": source.kind, "name": source.name }),
    )
    .await;
    Ok(Json(
        json!({ "source": mask_secrets(source), "ingest_token": ingest_token }),
    ))
}

/// 查看 api 来源的推送密钥（Editor；列表响应从不携带，查看走这里）。
pub async fn get_token(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = source_in_kb(&state, kb_id, source_id).await?;
    if source.kind != "api" {
        return Err(utopia_core::AppError::NotFound.into());
    }
    Ok(Json(json!({ "ingest_token": source.ingest_token })))
}

/// 轮换 api 来源的推送密钥：旧密钥立即失效。
pub async fn rotate_token(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = source_in_kb(&state, kb_id, source_id).await?;
    if source.kind != "api" {
        return Err(utopia_core::AppError::NotFound.into());
    }
    let token = new_ingest_token();
    utopia_store::sources::set_ingest_token(&state.pool, source_id, &token).await?;
    Ok(Json(json!({ "ingest_token": token })))
}

/// 响应前剔除凭据（只进不出；键见 `SOURCE_SECRET_KEYS`）。
fn mask_secrets(source: utopia_core::models::Source) -> utopia_core::models::Source {
    source.without_secrets()
}

/// 更新时凭据的合并规则，每个 `SOURCE_SECRET_KEYS` 里的键一样：新配置里**没有**这个键
/// 或值是空串 → 保留库里的原值（表单留空就是「别动」）；显式 `null` → 删掉；
/// 其余照新值。响应从不回显，所以客户端没有办法把旧值原样送回来，规则只能长在这里
fn keep_secrets(next: &mut serde_json::Value, existing: &serde_json::Value) {
    let Some(obj) = next.as_object_mut() else {
        return;
    };
    for key in SOURCE_SECRET_KEYS {
        let keep = match obj.get(*key) {
            None => true,
            Some(serde_json::Value::Null) => {
                obj.remove(*key);
                false
            }
            Some(v) => v.as_str().is_some_and(|s| s.trim().is_empty()),
        };
        if keep {
            obj.remove(*key);
            if let Some(prev) = existing.get(*key) {
                obj.insert((*key).to_string(), prev.clone());
            }
        }
    }
}

#[derive(Deserialize)]
pub struct UpdateBody {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
    pub icon: Option<String>,
    /// 出现 schedule 字段即整体覆盖调度（interval 与 cron 互斥；两者皆 null = 关闭定时）
    #[serde(default)]
    pub schedule: Option<ScheduleBody>,
}

#[derive(Deserialize)]
pub struct ScheduleBody {
    #[serde(default)]
    pub sync_interval_minutes: Option<i32>,
    #[serde(default)]
    pub sync_cron: Option<String>,
}

pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let existing = source_in_kb(&state, kb_id, source_id).await?;
    // 凭据只进不出：响应从不回显，表单留空 / 没传 = 保留库里原值
    let mut config = body.config;
    if let Some(cfg) = config.as_mut() {
        keep_secrets(cfg, &existing.config);
    }
    let source = utopia_store::sources::update(
        &state.pool,
        source_id,
        body.name.as_deref(),
        config.as_ref(),
        body.icon.as_deref(),
        body.schedule
            .map(|s| (s.sync_interval_minutes, s.sync_cron)),
    )
    .await?;
    state.emit_source(kb_id);
    // 审计不落凭据：config 只记「改没改」
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "source.updated",
        "source",
        Some(source_id),
        json!({ "name": body.name, "config_changed": config.is_some() }),
    )
    .await;
    Ok(Json(json!({ "source": mask_secrets(source) })))
}

/// 批量删除该来源下所有"不在来源中"的文档（url 对账 / custom 墓碑标出的）。
pub async fn cleanup_missing(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    source_in_kb(&state, kb_id, source_id).await?;
    let ids = utopia_store::documents::list_missing(&state.pool, source_id).await?;
    for id in &ids {
        utopia_store::documents::delete(&state.pool, *id).await?;
        let search = state.search.clone();
        let did = id.to_string();
        tokio::task::spawn_blocking(move || search.delete_document(&did))
            .await
            .map_err(|e| utopia_core::AppError::Other(e.into()))?
            .map_err(utopia_core::AppError::Other)?;
    }
    state.emit_source(kb_id);
    Ok(Json(json!({ "deleted": ids.len() })))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    // Memory 来源常驻：记忆空间不因来源整理而蒸发（记忆文档本身可在 Library 删除）
    let source = source_in_kb(&state, kb_id, source_id).await?;
    if source.kind == utopia_store::memory::MEMORY_SOURCE_KIND {
        return Err(utopia_core::AppError::invalid(
            "memory_source_permanent",
            "The Memory source is permanent.",
        )
        .into());
    }
    utopia_store::sources::delete(&state.pool, source_id).await?;
    state.emit_source(kb_id);
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "source.deleted",
        "source",
        Some(source_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 同步运行历史（渠道审计）。
pub async fn runs(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    source_in_kb(&state, kb_id, source_id).await?;
    let runs = utopia_store::sources::list_runs(&state.pool, source_id, 20).await?;
    Ok(Json(json!({ "runs": runs })))
}

pub async fn sync_now(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    source_in_kb(&state, kb_id, source_id).await?;
    let queued = utopia_store::sources::mark_queued(&state.pool, source_id).await?;
    if queued {
        utopia_store::jobs::enqueue(
            &state.pool,
            "sync_source",
            json!({ "source_id": source_id }),
        )
        .await?;
        state.emit_source(kb_id);
    }
    Ok(Json(json!({ "queued": queued })))
}

#[derive(Deserialize)]
pub struct IngestBody {
    pub filename: String,
    /// 墓碑推送（`deleted: true`）可以不带 content——指南一直这么写，而字段
    /// 从前是必填，缺了就在反序列化那一步被拒。其余情况仍然必填：空串在
    /// 下面的校验里挡
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub doc_time: Option<DateTime<Utc>>,
    /// 调用方的逻辑文档 ID：同 ID 再推 = 更新同一文档（原地替换 + 版本记录）。
    /// 不传则以 filename 为身份。
    #[serde(default)]
    pub external_id: Option<String>,
    /// 墓碑：true = 按身份标记"不在来源中"（不删除；仅 api 来源推送支持）。
    /// 此时 content 可省略；再次正常推送同身份会摘掉标记。
    #[serde(default)]
    pub deleted: bool,
}

fn action_str(action: crate::ingest_sources::IngestAction) -> &'static str {
    match action {
        crate::ingest_sources::IngestAction::Created => "created",
        crate::ingest_sources::IngestAction::Updated => "updated",
        crate::ingest_sources::IngestAction::Moved => "moved",
        crate::ingest_sources::IngestAction::Unchanged => "unchanged",
        crate::ingest_sources::IngestAction::Tombstoned => "marked_missing",
    }
}

/// KB 级文本推送（会话认证）：普通上传语义——落入 Uploads，无身份追踪。
/// 需要"同 ID 再推 = 更新"语义时，建一个 api 来源用它的密钥推送。
pub async fn ingest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(body): Json<IngestBody>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if body.deleted {
        return Err(utopia_core::AppError::Validation(
            "tombstones need identity tracking — push to an api source instead".into(),
        )
        .into());
    }
    if body.filename.trim().is_empty() || body.content.trim().is_empty() {
        return Err(
            utopia_core::AppError::Validation("filename and content are required".into()).into(),
        );
    }
    let action = crate::ingest_sources::ingest_upload(
        &state,
        kb_id,
        body.filename.trim(),
        "text/plain",
        body.content.as_bytes(),
        body.doc_time,
    )
    .await
    .map_err(utopia_core::AppError::Other)?;
    Ok(Json(json!({ "action": action_str(action) })))
}

/// 推送失败的两种性质。**调用方发错了**和**我们这边没接住**得分开：前者记进
/// run 供集成调试，但不算来源同步失败——来源没坏，是那一次请求不合格；
/// 后者才该把来源标成 failed 并进告警中心。从前两者都走 `finish_sync(error)`，
/// 一次格式错误就让铃铛说"来源同步失败，没有新内容进来"
enum PushError {
    /// 4xx：负载不合格（JSON 解析、缺字段）
    Rejected(String),
    /// 摄入本身失败
    Failed(String),
}

impl PushError {
    fn message(&self) -> &str {
        match self {
            PushError::Rejected(m) | PushError::Failed(m) => m,
        }
    }
}

/// 认证之后的推送处理：解析 + 校验 + 摄入/墓碑。错误一律返回文字（记进 run）。
async fn handle_push(
    state: &AppState,
    source: &utopia_core::models::Source,
    bytes: &[u8],
) -> Result<crate::ingest_sources::IngestAction, PushError> {
    let body: IngestBody = serde_json::from_slice(bytes)
        .map_err(|e| PushError::Rejected(format!("Invalid JSON payload: {e}")))?;
    let identity = body
        .external_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| body.filename.trim())
        .to_string();
    if identity.is_empty() {
        return Err(PushError::Rejected(
            "external_id or filename is required".into(),
        ));
    }
    let key = format!("api:{identity}");

    // 墓碑：标记"不在来源中"（与 custom 的 deleted[] 同一条路径），content 可省略
    if body.deleted {
        utopia_store::documents::mark_missing_keys(&state.pool, source.id, &[key])
            .await
            .map_err(|e| PushError::Failed(e.to_string()))?;
        return Ok(crate::ingest_sources::IngestAction::Tombstoned);
    }

    if body.filename.trim().is_empty() || body.content.trim().is_empty() {
        return Err(PushError::Rejected(
            "filename and content are required".into(),
        ));
    }
    let action = crate::ingest_sources::ingest_item(
        state,
        source.kb_id,
        source.id,
        &key,
        body.filename.trim(),
        "text/plain",
        body.content.as_bytes(),
        body.doc_time,
    )
    .await
    .map_err(|e| PushError::Failed(e.to_string()))?;
    // 失而复得：曾被墓碑标记的身份再次正常推送，摘掉 missing 标记
    utopia_store::documents::clear_missing_keys(&state.pool, source.id, &[key])
        .await
        .map_err(|e| PushError::Failed(e.to_string()))?;
    Ok(action)
}

/// api 来源推送（来源专属密钥认证，无会话）：三路身份语义——
/// 新 external_id → 新增；同 ID 同内容 → 无操作；同 ID 新内容 → 原地更新 + 版本记录。
/// 认证通过后每次推送都记一条 run（含格式错误——集成调试全靠它）；
/// 未认证请求不写任何记录（不给匿名流量制造落库路径）。
pub async fn push(
    State(state): State<AppState>,
    Path(source_id): Path<Uuid>,
    headers: HeaderMap,
    bytes: axum::body::Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let source = utopia_store::sources::get(&state.pool, source_id).await?;
    if source.kind != "api" {
        return Err(utopia_core::AppError::NotFound.into());
    }
    // Bearer 密钥校验
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or(utopia_core::AppError::Unauthorized)?;
    if source.ingest_token.as_deref() != Some(token) {
        return Err(utopia_core::AppError::Unauthorized.into());
    }

    let run = utopia_store::sources::start_run(&state.pool, source.id).await?;
    match handle_push(&state, &source, &bytes).await {
        Ok(action) => {
            let (created, updated) = match action {
                crate::ingest_sources::IngestAction::Created => (1, 0),
                crate::ingest_sources::IngestAction::Updated
                | crate::ingest_sources::IngestAction::Moved => (0, 1),
                crate::ingest_sources::IngestAction::Unchanged
                | crate::ingest_sources::IngestAction::Tombstoned => (0, 0),
            };
            utopia_store::sources::finish_run(&state.pool, run, source.id, None, created, updated)
                .await?;
            // 来源行同步反映"上一次推送"：操作条/左栏状态点直接可用
            utopia_store::sources::finish_sync(&state.pool, source.id, None, created).await?;
            state.emit_source(source.kb_id);
            Ok(Json(json!({ "action": action_str(action) })))
        }
        Err(err) => {
            let msg = err.message().to_string();
            utopia_store::sources::finish_run(&state.pool, run, source.id, Some(&msg), 0, 0)
                .await?;
            // 只有我们这边没接住才算来源失败；调用方发错了留在 run 历史里就够
            if let PushError::Failed(_) = err {
                utopia_store::sources::finish_sync(&state.pool, source.id, Some(&msg), 0).await?;
            }
            state.emit_source(source.kb_id);
            Err(utopia_core::AppError::Validation(msg).into())
        }
    }
}

/// 来源级全量重抽（增量语义）：该来源下所有 ready 文档重新过一遍抽取。
/// 走正常管道——实体消解、事实去重、时态冲突照常，既有人工决策全部保留。
pub async fn re_extract(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, source_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let source = utopia_store::sources::get(&state.pool, source_id).await?;
    if source.kb_id != kb_id {
        return Err(utopia_core::AppError::NotFound.into());
    }
    // 任务由 queue_extraction 与状态同事务建好，这里只负责推送
    let ids =
        utopia_store::documents::queue_extraction(&state.pool, kb_id, Some(source_id)).await?;
    for id in &ids {
        state.emit_document(kb_id, *id);
    }
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "source.re_extract",
        "source",
        Some(source_id),
        json!({ "name": source.name, "documents": ids.len() }),
    )
    .await;
    Ok(Json(json!({ "queued": ids.len() })))
}

#[cfg(test)]
mod tests {
    use super::keep_secrets;
    use serde_json::json;

    #[test]
    fn a_blank_or_missing_secret_keeps_the_stored_one() {
        let existing = json!({ "bucket": "old", "secret_access_key": "s", "password": "p" });
        // 没传 → 留；空串 → 留；有值 → 换；null → 删
        let mut next = json!({ "bucket": "new", "password": "  ", "token": null });
        keep_secrets(&mut next, &existing);
        assert_eq!(next["bucket"], "new");
        assert_eq!(
            next["secret_access_key"], "s",
            "missing keeps the stored value"
        );
        assert_eq!(next["password"], "p", "blank keeps the stored value");
        assert!(next.get("token").is_none(), "an explicit null removes it");
        let mut next = json!({ "secret_access_key": "fresh" });
        keep_secrets(&mut next, &existing);
        assert_eq!(next["secret_access_key"], "fresh");
        assert_eq!(next["password"], "p");
    }

    #[test]
    fn no_secret_reaches_a_response() {
        let source = utopia_core::models::Source {
            id: uuid::Uuid::nil(),
            kb_id: uuid::Uuid::nil(),
            kind: "s3".into(),
            name: "s".into(),
            config: json!({ "bucket": "b", "access_key_id": "AKIA", "secret_access_key": "x",
                            "account_key": "y", "service_account_key": "z", "password": "w",
                            "token": "t", "auth_header": "h" }),
            icon: None,
            sync_interval_minutes: None,
            sync_cron: None,
            last_sync_at: None,
            last_sync_status: "never".into(),
            last_sync_error: None,
            last_sync_added: 0,
            ingest_token: Some("utp_x".into()),
            created_at: chrono::Utc::now(),
        };
        let masked = super::mask_secrets(source);
        let obj = masked.config.as_object().unwrap();
        for key in utopia_core::models::SOURCE_SECRET_KEYS {
            assert!(!obj.contains_key(*key), "{key} leaked");
        }
        assert_eq!(obj["bucket"], "b");
        assert_eq!(
            obj["access_key_id"], "AKIA",
            "an identifier is not a secret"
        );
    }
}
