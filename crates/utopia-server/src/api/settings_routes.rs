use axum::extract::{Path, State};
use axum::Json;
use utopia_core::models::Role;
use utopia_llm::ChatMessage;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::llm_util;
use crate::state::AppState;

/// GET：脱敏视图（密钥只回传是否已配置）。
pub async fn get(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let s = utopia_store::settings::get(&state.pool, workspace_id).await?;
    Ok(Json(match s {
        None => json!({}),
        Some(s) => json!({
            "chat_base_url": s.chat_base_url,
            "chat_model": s.chat_model,
            "has_chat_key": s.chat_api_key.as_deref().is_some_and(|k| !k.is_empty()),
            "embed_base_url": s.embed_base_url,
            "embed_model": s.embed_model,
            "embed_dim": s.embed_dim,
            "has_embed_key": s.embed_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        }),
    }))
}

#[derive(Deserialize)]
pub struct PutSettingsReq {
    pub chat_base_url: Option<String>,
    /// None 或空串 = 保留旧密钥
    pub chat_api_key: Option<String>,
    pub chat_model: Option<String>,
    pub embed_base_url: Option<String>,
    pub embed_api_key: Option<String>,
    pub embed_model: Option<String>,
    pub embed_dim: Option<i32>,
}

pub async fn put(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PutSettingsReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let nonempty = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    utopia_store::settings::upsert(
        &state.pool,
        workspace_id,
        nonempty(&req.chat_base_url).as_deref(),
        nonempty(&req.chat_api_key).as_deref(),
        nonempty(&req.chat_model).as_deref(),
        nonempty(&req.embed_base_url).as_deref(),
        nonempty(&req.embed_api_key).as_deref(),
        nonempty(&req.embed_model).as_deref(),
        req.embed_dim,
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// 连通性测试：对话发一条最小消息；embedding 试算一条并返回维度。
pub async fn test(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let Some(s) = utopia_store::settings::get(&state.pool, workspace_id).await? else {
        return Ok(Json(json!({ "chat": { "ok": false, "error": "Not configured" },
                               "embed": { "ok": false, "error": "Not configured" } })));
    };

    let chat_result = match llm_util::chat_client(&s) {
        None => json!({ "ok": false, "error": "Not configured" }),
        Some(client) => {
            let msg = [ChatMessage {
                role: "user".into(),
                content: "Reply with exactly one word: OK".into(),
            }];
            match client.chat(&msg).await {
                Ok(reply) => {
                    json!({ "ok": true, "reply": reply.chars().take(50).collect::<String>() })
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            }
        }
    };

    let embed_result = match llm_util::embed_client(&s) {
        None => json!({ "ok": false, "error": "Not configured" }),
        Some(client) => match client.embed(&["connectivity test".to_string()]).await {
            Ok(v) if !v.is_empty() => json!({ "ok": true, "dim": v[0].len() }),
            Ok(_) => json!({ "ok": false, "error": "Empty response" }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        },
    };

    Ok(Json(json!({ "chat": chat_result, "embed": embed_result })))
}
