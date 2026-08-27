//! 本体编辑器 API：类型/关系 CRUD + 未匹配统计 + LLM 扩展建议。
//! 查看 = viewer；修改 = editor（本体直接影响后续抽取的白名单）。

use axum::extract::{Path, Query, State};
use axum::Json;
use utopia_llm::ChatMessage;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::llm_util;
use crate::state::AppState;

async fn require_kb(
    state: &AppState,
    user: &utopia_core::models::User,
    kb_id: Uuid,
    min: Role,
) -> Result<utopia_core::models::KnowledgeBase, AppError> {
    utopia_store::access::require_kb(&state.pool, user, kb_id, min).await
}

pub async fn get(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    utopia_store::graph::ensure_default_ontology(&state.pool, kb_id).await?;
    let entity_types = utopia_store::ontology::entity_type_views(&state.pool, kb_id).await?;
    let relation_types = utopia_store::ontology::relation_type_views(&state.pool, kb_id).await?;
    let misses = utopia_store::ontology::list_misses(&state.pool, kb_id).await?;
    Ok(Json(json!({
        "entity_types": entity_types,
        "relation_types": relation_types,
        "misses": misses,
    })))
}

#[derive(Deserialize)]
pub struct InstancesQuery {
    #[serde(default)]
    pub page: i64,
    #[serde(default = "default_per")]
    pub per: i64,
}

fn default_per() -> i64 {
    12
}

/// 某个类下的实体实例列表（详情区右侧，分页）。
pub async fn list_entity_instances(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, type_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<InstancesQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let per = q.per.clamp(1, 100);
    let page = q.page.max(0);
    let (rows, total) =
        utopia_store::ontology::entity_instances(&state.pool, kb_id, type_id, per, page * per)
            .await?;
    Ok(Json(json!({ "entities": rows, "total": total })))
}

#[derive(Deserialize)]
pub struct EntityTypeReq {
    pub key: Option<String>,
    pub label: String,
    #[serde(default)]
    pub color: Option<String>,
    /// circle | square
    #[serde(default)]
    pub shape: Option<String>,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// 语义指引，注入抽取 prompt
    #[serde(default)]
    pub description: Option<String>,
}

pub async fn create_entity_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<EntityTypeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let key = req
        .key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("key is required".into()))?;
    let id = utopia_store::ontology::create_entity_type(
        &state.pool,
        kb_id,
        key,
        req.label.trim(),
        req.color.as_deref().unwrap_or("#8ea5bd"),
        req.shape.as_deref().unwrap_or("circle"),
        req.parent_id,
        req.description.as_deref().unwrap_or("").trim(),
    )
    .await?;
    // 审计只记不阻断
    let _ = utopia_store::audit::record(
        &state.pool, Some(kb_id), user.id, "entity_type.created", "entity_type", Some(id),
        json!({ "key": key, "label": req.label.trim() }),
    ).await;
    Ok(Json(json!({ "id": id })))
}

pub async fn update_entity_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, id)): Path<(Uuid, Uuid)>,
    Json(req): Json<EntityTypeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::ontology::update_entity_type(
        &state.pool,
        kb_id,
        id,
        req.label.trim(),
        req.color.as_deref().unwrap_or("#8ea5bd"),
        req.shape.as_deref().unwrap_or("circle"),
        req.parent_id,
        req.description.as_deref().unwrap_or("").trim(),
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool, Some(kb_id), user.id, "entity_type.updated", "entity_type", Some(id),
        json!({ "label": req.label.trim(), "color": req.color, "shape": req.shape,
                "description": req.description }),
    ).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_entity_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::ontology::delete_entity_type(&state.pool, kb_id, id).await?;
    let _ = utopia_store::audit::record(
        &state.pool, Some(kb_id), user.id, "entity_type.deleted", "entity_type", Some(id),
        json!({}),
    ).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct RelationTypeReq {
    pub key: Option<String>,
    pub label: String,
    #[serde(default = "default_temporal")]
    pub temporal: String,
    #[serde(default)]
    pub functional: bool,
    #[serde(default)]
    pub inverse_functional: bool,
    #[serde(default)]
    pub description: Option<String>,
    /// relation | attribute（创建时定死，更新时忽略）
    #[serde(default)]
    pub kind: Option<String>,
    /// attribute 专用：所属类
    #[serde(default)]
    pub domain_type_id: Option<Uuid>,
    /// attribute 专用：text | number | date | bool
    #[serde(default)]
    pub datatype: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
}

fn default_temporal() -> String {
    "state".into()
}

pub async fn create_relation_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<RelationTypeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let key = req
        .key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::Validation("key is required".into()))?;
    let kind = req.kind.as_deref().unwrap_or("relation");
    let id = utopia_store::ontology::create_relation_type(
        &state.pool,
        kb_id,
        key,
        req.label.trim(),
        &req.temporal,
        req.functional,
        req.inverse_functional,
        req.description.as_deref().unwrap_or("").trim(),
        kind,
        req.domain_type_id,
        req.datatype.as_deref(),
        req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool, Some(kb_id), user.id, "relation_type.created", "relation_type", Some(id),
        json!({ "key": key, "label": req.label.trim(), "temporal": req.temporal, "kind": kind }),
    ).await;
    Ok(Json(json!({ "id": id })))
}

pub async fn update_relation_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, id)): Path<(Uuid, Uuid)>,
    Json(req): Json<RelationTypeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::ontology::update_relation_type(
        &state.pool,
        kb_id,
        id,
        req.label.trim(),
        &req.temporal,
        req.functional,
        req.inverse_functional,
        req.description.as_deref().unwrap_or("").trim(),
        req.datatype.as_deref(),
        req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    )
    .await?;
    let _ = utopia_store::audit::record(
        &state.pool, Some(kb_id), user.id, "relation_type.updated", "relation_type", Some(id),
        json!({ "label": req.label.trim(), "temporal": req.temporal,
                "functional": req.functional, "inverse_functional": req.inverse_functional,
                "description": req.description }),
    ).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_relation_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::ontology::delete_relation_type(&state.pool, kb_id, id).await?;
    let _ = utopia_store::audit::record(
        &state.pool, Some(kb_id), user.id, "relation_type.deleted", "relation_type", Some(id),
        json!({}),
    ).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct DismissMissReq {
    pub kind: String,
    pub key: String,
}

pub async fn dismiss_miss(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<DismissMissReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::ontology::clear_miss(&state.pool, kb_id, &req.kind, &req.key).await?;
    Ok(Json(json!({ "ok": true })))
}

/// LLM 本体扩展建议：现有本体 + 未匹配统计 → 提案（人审后经 create 端点合入）。
pub async fn suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let kb = require_kb(&state, &user, kb_id, Role::Editor).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| AppError::Validation("Chat model not configured".into()))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| AppError::Validation("Chat model not configured".into()))?;

    let entity_types = utopia_store::ontology::entity_type_views(&state.pool, kb_id).await?;
    let relation_types = utopia_store::ontology::relation_type_views(&state.pool, kb_id).await?;
    let misses = utopia_store::ontology::list_misses(&state.pool, kb_id).await?;
    if misses.is_empty() {
        return Ok(Json(json!({ "entity_types": [], "relation_types": [] })));
    }

    let current_et: Vec<String> = entity_types.iter().map(|t| t.key.clone()).collect();
    let current_rt: Vec<String> = relation_types.iter().map(|r| r.key.clone()).collect();
    let miss_lines: Vec<String> = misses
        .iter()
        .map(|m| {
            format!(
                "- [{}] \"{}\" seen {} times, e.g. {}",
                m.kind,
                m.key,
                m.count,
                m.example.as_deref().unwrap_or("-")
            )
        })
        .collect();

    let prompt = format!(
        "You are an ontology engineer. A knowledge graph has this ontology:\n\
         Entity type keys: {}\n\
         Relation type keys: {}\n\
         \n\
         During extraction, the LLM repeatedly produced types/relations OUTSIDE this ontology:\n{}\n\
         \n\
         Propose ontology extensions that would cover these misses. Merge near-duplicates. \
         Only propose what is clearly warranted by the misses. Output exactly one JSON object:\n\
         {{\"entity_types\":[{{\"key\":\"snake_case\",\"label\":\"Display Name\",\"reason\":\"...\"}}],\n\
          \"relation_types\":[{{\"key\":\"snake_case\",\"label\":\"display label\",\"temporal\":\"state|event|eternal\",\"functional\":false,\"reason\":\"...\"}}]}}",
        current_et.join(", "),
        current_rt.join(", "),
        miss_lines.join("\n")
    );

    let reply = client
        .chat(&[ChatMessage {
            role: "user".into(),
            content: prompt,
        }])
        .await
        .map_err(AppError::Other)?;
    let block = utopia_extract::json_block(&reply).map_err(AppError::Other)?;
    let proposals: serde_json::Value =
        serde_json::from_str(&block).map_err(|e| AppError::Other(e.into()))?;
    Ok(Json(proposals))
}
