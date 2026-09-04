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

/// 时刻参数（YYYY-MM-DD 或 RFC3339）——服务端时间旅行。
///
/// 两根轴共用这一个解析：`at` 走世界轴（那时世界是什么样），`as_of` 走记录轴
/// （那时我们以为世界是什么样）。**两个参数一路分开**（0019）：合成一个控件，
/// 答出来的是另一个问题，而屏幕上看不出来
fn parse_instant(
    field: &str,
    raw: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, AppError> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(Some(d.and_hms_opt(0, 0, 0).unwrap().and_utc()));
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| Some(t.with_timezone(&chrono::Utc)))
        .map_err(|_| {
            AppError::Validation(format!(
                "Invalid `{field}` (expected YYYY-MM-DD or RFC3339)"
            ))
        })
}

/// 总览一次画多少个节点的**默认值**。上限本身是合理的——画一万个点没人看得懂；
/// 骗人的是把它当成规模显示，所以接口同时回总数
const GRAPH_NODE_CAP: i64 = 150;
/// 调得再高也得有个天花板。**这个数不是拍的**：节点按度数降序取，越往后越是
/// 边缘节点，而力导布局是 O(n²) 量级的——超过这个数，先垮的是「拖得动」
/// 而不是「看得清」。要真看上万个点，那是另一种视图，不是把这个调大
const GRAPH_NODE_CAP_MAX: i64 = 1000;

#[derive(Deserialize)]
pub struct OverviewQuery {
    #[serde(default)]
    pub at: Option<String>,
    /// 记录轴：那时**我们持有**的图（0019）。不给就是现在
    #[serde(default)]
    pub as_of: Option<String>,
    /// 画多少个。不给就用默认值；给了也钳在 [10, GRAPH_NODE_CAP_MAX]——
    /// 界面上的按钮只给几档，但接口是公开的，别让一个 `limit=999999` 把库拖垮
    #[serde(default)]
    pub limit: Option<i64>,
}

pub async fn overview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<OverviewQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let at = parse_instant("at", q.at.as_deref())?;
    let as_of = parse_instant("as_of", q.as_of.as_deref())?;
    // 画多少个是渲染的事，库里有多少是知识库的事——两个数都回，界面才说得出
    // 「画了 150 个，共 325 个」而不是把上限说成规模
    let limit = q
        .limit
        .unwrap_or(GRAPH_NODE_CAP)
        .clamp(10, GRAPH_NODE_CAP_MAX);
    let (nodes, edges, total_nodes, total_edges) =
        utopia_store::graph::overview(&state.pool, kb_id, limit, at, as_of).await?;
    Ok(Json(json!({
        "nodes": nodes, "edges": edges,
        "total_nodes": total_nodes, "total_edges": total_edges,
    })))
}

#[derive(Deserialize)]
pub struct NeighborhoodQuery {
    pub entity: Uuid,
    #[serde(default)]
    pub hops: Option<u8>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub as_of: Option<String>,
}

pub async fn neighborhood(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<NeighborhoodQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let at = parse_instant("at", q.at.as_deref())?;
    let as_of = parse_instant("as_of", q.as_of.as_deref())?;
    let (nodes, edges) = utopia_store::graph::neighborhood(
        &state.pool,
        kb_id,
        q.entity,
        q.hops.unwrap_or(2),
        at,
        as_of,
    )
    .await?;
    Ok(Json(json!({ "nodes": nodes, "edges": edges })))
}

#[derive(Deserialize)]
pub struct EntitySearchQuery {
    pub q: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

pub async fn search_entities(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(query): Query<EntitySearchQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    if query.q.trim().is_empty() {
        return Ok(Json(json!({ "entities": [], "total": 0 })));
    }
    // 一并回总数：「宁分勿合」本来就会造出一堆同名，固定十条时想找的那个
    // 可能根本不在这十条里，而界面上看不出来
    let (entities, total) = utopia_store::graph::search_entities(
        &state.pool,
        kb_id,
        &query.q,
        query.limit.unwrap_or(10).clamp(1, 100),
        query.offset.unwrap_or(0).max(0),
    )
    .await?;
    Ok(Json(json!({ "entities": entities, "total": total })))
}

#[derive(Deserialize)]
pub struct EntityDetailQuery {
    /// 记录轴：回放中的图上点开一个节点，面板该说**当时**的事实（0019）
    #[serde(default)]
    pub as_of: Option<String>,
}

pub async fn entity_detail(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, entity_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<EntityDetailQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let as_of = parse_instant("as_of", q.as_of.as_deref())?;
    let (entity, facts) =
        utopia_store::graph::entity_detail(&state.pool, kb_id, entity_id, as_of).await?;
    // 推出来的那些**单独回一个键**，不掺进 `facts`。前端据此给它们自己的一档：
    // 一条派生边跟一条断言边混在同一个列表里，用户看不出「这条是文档里写的」
    // 和「这条是引擎推的」的区别，而那正是推理会污染知识的样子
    let derived =
        utopia_store::reasoning::derived_for_entity(&state.pool, kb_id, entity_id).await?;
    // 同名的那些**打开面板时就给**，不是等改名之后才回。
    //
    // 从前它只随 `update_entity` 的响应回来，于是「把同名的合并进来」这个动作
    // 只有先改一次名才够得着——而两个张伟并存是「宁分勿合」的正当产物，不是
    // 改名改出来的。合并入口该长在能看见同名的地方。
    let same_name = utopia_store::graph::same_name_peers(&state.pool, kb_id, entity_id).await?;
    // 没落地的派生（0017 §3）也单独一个键：它们连 `derived_facts` 都不在
    let blocked =
        utopia_store::reasoning::blocked_for_entity(&state.pool, kb_id, entity_id).await?;
    Ok(Json(json!({
        "entity": entity, "facts": facts,
        "derived": derived, "blocked": blocked, "same_name": same_name,
    })))
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
        return Err(AppError::invalid("nothing_to_update", "Nothing to update").into());
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

/// 一条派生事实的证明（0002 R2）：前提按推导顺序，每条带证据，一路到原句。
/// 派生已失效或不存在时 `proof` 为 null——不是错误，界面据此退回文本前提
pub async fn derived_proof(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, derived_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let proof = utopia_store::reasoning::proof(&state.pool, kb_id, derived_id).await?;
    Ok(Json(json!({ "proof": proof })))
}

/// 没落地的派生的证明链（0017 §3）：前提在那条 `derived_contradiction` 违规的
/// `path` 里，展开方式与落了地的一样。违规不存在时 `steps` 为 null
pub async fn blocked_proof(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, violation_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let steps = utopia_store::reasoning::blocked_proof(&state.pool, kb_id, violation_id).await?;
    Ok(Json(json!({ "steps": steps })))
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
