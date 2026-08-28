use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

pub(super) async fn require_kb(
    state: &AppState,
    user: &utopia_core::models::User,
    kb_id: Uuid,
    min: Role,
) -> Result<utopia_core::models::KnowledgeBase, AppError> {
    utopia_store::access::require_kb(&state.pool, user, kb_id, min).await
}

/// `at`：可选 as-of 日期（YYYY-MM-DD 或 RFC3339）——服务端时间旅行。
fn parse_at(raw: Option<&str>) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(Some(d.and_hms_opt(0, 0, 0).unwrap().and_utc()));
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| Some(t.with_timezone(&chrono::Utc)))
        .map_err(|_| AppError::Validation("Invalid `at` (expected YYYY-MM-DD or RFC3339)".into()))
}

#[derive(Deserialize)]
pub struct OverviewQuery {
    #[serde(default)]
    pub at: Option<String>,
}

pub async fn overview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<OverviewQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let at = parse_at(q.at.as_deref())?;
    let (nodes, edges) = utopia_store::graph::overview(&state.pool, kb_id, 150, at).await?;
    Ok(Json(json!({ "nodes": nodes, "edges": edges })))
}

#[derive(Deserialize)]
pub struct NeighborhoodQuery {
    pub entity: Uuid,
    #[serde(default)]
    pub hops: Option<u8>,
    #[serde(default)]
    pub at: Option<String>,
}

pub async fn neighborhood(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<NeighborhoodQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let at = parse_at(q.at.as_deref())?;
    let (nodes, edges) =
        utopia_store::graph::neighborhood(&state.pool, kb_id, q.entity, q.hops.unwrap_or(2), at)
            .await?;
    Ok(Json(json!({ "nodes": nodes, "edges": edges })))
}

#[derive(Deserialize)]
pub struct EntitySearchQuery {
    pub q: String,
}

pub async fn search_entities(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(query): Query<EntitySearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    if query.q.trim().is_empty() {
        return Ok(Json(json!({ "entities": [] })));
    }
    let entities = utopia_store::graph::search_entities(&state.pool, kb_id, &query.q, 10).await?;
    Ok(Json(json!({ "entities": entities })))
}

pub async fn entity_detail(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, entity_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let (entity, facts) = utopia_store::graph::entity_detail(&state.pool, kb_id, entity_id).await?;
    Ok(Json(json!({ "entity": entity, "facts": facts })))
}

#[derive(Deserialize)]
pub struct EntityPatch {
    #[serde(default)]
    pub type_id: Option<Uuid>,
    #[serde(default)]
    pub canonical_name: Option<String>,
}

/// 人工修正实体的类型或名字。抽取给的是初判，此前判错只能整库重抽。
///
/// 改名撞上同名实体不拦（两个张伟是"宁分勿合"的正当产物），改完把同名的报回去，
/// 由界面提示是否合并——判定它们是否真是同一个，是人的事。
pub async fn update_entity(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, entity_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<EntityPatch>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if req.type_id.is_none() && req.canonical_name.is_none() {
        return Err(AppError::Validation("Nothing to update".into()).into());
    }
    let (before, after) = utopia_store::graph::update_entity(
        &state.pool,
        kb_id,
        entity_id,
        req.type_id,
        req.canonical_name.as_deref(),
    )
    .await?;

    // 台账快照自包含：类型以后被删掉，这条记录仍然读得懂。
    // P4 要按 from/to 聚合"一个月里 37 个实体从 Product 挪到 Concept"，所以分开记两个动作。
    if before.type_key != after.type_key {
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "entity.retyped",
            "entity",
            Some(entity_id),
            json!({
                "name": after.name,
                "from": { "key": before.type_key, "label": before.type_label },
                "to": { "key": after.type_key, "label": after.type_label },
            }),
        )
        .await;
    }
    if before.name != after.name {
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "entity.renamed",
            "entity",
            Some(entity_id),
            json!({ "from": before.name, "to": after.name, "type": after.type_label }),
        )
        .await;
    }

    let peers = utopia_store::graph::same_name_peers(&state.pool, kb_id, entity_id).await?;
    state.emit_review(kb_id);
    Ok(Json(json!({ "entity": after, "same_name": peers })))
}

pub async fn fact_evidence(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, fact_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let evidence = utopia_store::graph::fact_evidence(&state.pool, fact_id).await?;
    Ok(Json(json!({ "evidence": evidence })))
}

/// 手动触发抽取（failed 重试 / 补配模型后补抽）。
pub async fn extract(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(document_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, document_id).await?;
    require_kb(&state, &user, doc.kb_id, Role::Editor).await?;
    // 手动触发 = 强制全量：清增量标记、解雇在跑的任务、置 queued、建任务，一个事务办完
    let job_id = utopia_store::documents::queue_extraction_one(&state.pool, document_id).await?;
    state.emit_document(doc.kb_id, document_id);
    Ok(Json(json!({ "job_id": job_id })))
}

/// 图谱重建（清算语义，KB admin）：清空整个图层后全量重抽。
/// 与来源级重抽的分工——重抽保留既有决策，重建放弃它们，换取
/// "当前语料 × 当前本体"的确定性重演（早期脏抽取、本体大改后的收场手段）。
/// 决策台账与裁决缓存刻意保留（见 store::graph::purge_graph）。
pub async fn rebuild(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Admin).await?;
    let (entities, facts) = utopia_store::graph::purge_graph(&state.pool, kb_id).await?;
    // 任务由 queue_extraction 与状态同事务建好，这里只负责推送
    let ids = utopia_store::documents::queue_extraction(&state.pool, kb_id, None).await?;
    for id in &ids {
        state.emit_document(kb_id, *id);
    }
    state.emit_review(kb_id);
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "graph.rebuild",
        "kb",
        Some(kb_id),
        json!({ "entities_removed": entities, "facts_removed": facts, "documents": ids.len() }),
    )
    .await;
    Ok(Json(json!({
        "entities_removed": entities, "facts_removed": facts, "queued": ids.len()
    })))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    #[serde(default)]
    pub page: i64,
    #[serde(default = "default_history_per")]
    pub per: i64,
}

fn default_history_per() -> i64 {
    30
}

/// 实体的认知变更历史（记录时间轴）：我们何时这么认为、又何时改了主意。
/// 与 entity_detail（有效时间轴，只看现行）互补。
pub async fn entity_history(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, entity_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<HistoryQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let per = q.per.clamp(1, 200);
    let page = q.page.max(0);
    let (events, total) =
        utopia_store::graph::entity_history(&state.pool, kb_id, entity_id, per, page * per).await?;
    Ok(Json(json!({ "events": events, "total": total })))
}
