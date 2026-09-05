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
pub(super) fn parse_instant(
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

/// 一条事实的新有效区间。**整体替换，不是部分更新**：区间的两端互相定义，
/// 「把结束端清空」与「这次不动结束端」必须能分辨，而缺字段与 null 在 JSON 里
/// 分不开。界面提交的是表单里的四个值，不是差异。
#[derive(Deserialize)]
pub struct FactTimePatch {
    #[serde(default)]
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub valid_from_precision: Option<String>,
    #[serde(default)]
    pub valid_to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub valid_to_precision: Option<String>,
    /// 为什么改。落进台账，不进图——「之前记错了」与「新证据说是六月」在账本上
    /// 长得一样，硬加一个类型枚举会让入口变重而判据模糊；真需要分辨了，再从
    /// 这里升格成字段
    #[serde(default)]
    pub note: Option<String>,
}

const DATE_PRECISIONS: [&str; 3] = ["year", "month", "day"];

/// 修改前的区间，用于「一字未改」的判定与台账里的前后对照。
#[derive(sqlx::FromRow)]
struct FactInterval {
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_from_precision: Option<String>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_to_precision: Option<String>,
}

/// 复刻 `facts` 表的两条 CHECK，外加一条数据库没有的：起点不能晚于终点。
///
/// 在这里挡住是为了让错误说人话。交给数据库挡会得到一句约束名，而这三态
/// （仍在持续 / 结束了不知哪天 / 某日结束）本来就是账本里最容易记混的地方。
fn check_interval(p: &FactTimePatch) -> Result<(), AppError> {
    // 起始端：没日期就没精度，且没有 unknown 档——「开始了但不知何时」与
    // 「不知道有没有开始」在这个账本里不可区分（见迁移 0003 的注释）
    match (&p.valid_from, p.valid_from_precision.as_deref()) {
        (Some(_), Some(prec)) if DATE_PRECISIONS.contains(&prec) => {}
        (None, None) => {}
        _ => return Err(AppError::invalid(
            "bad_valid_from",
            "A start date needs a precision of year, month or day, and a precision needs a date.",
        )),
    }
    // 结束端：有日期必有精度；没日期只能是仍在持续（都为空）或结束了不知哪天
    match (&p.valid_to, p.valid_to_precision.as_deref()) {
        (Some(_), Some(prec)) if DATE_PRECISIONS.contains(&prec) => {}
        (None, None) => {}
        (None, Some(prec)) if prec == utopia_store::graph::ENDED_UNKNOWN => {}
        _ => {
            return Err(AppError::invalid(
                "bad_valid_to",
                "An end date needs a precision of year, month or day. Leave the date empty for still going, or mark it ended with an unknown date.",
            ))
        }
    }
    if let (Some(from), Some(to)) = (p.valid_from, p.valid_to) {
        if from > to {
            return Err(AppError::invalid(
                "interval_inverted",
                "The start of the interval is after its end.",
            ));
        }
    }
    Ok(())
}

/// 人工修正一条事实的有效区间。抽取给的是初判，此前判错只能删掉文档重抽——
/// 名字判错有 Review 可改（见 `update_entity`），时间判错没有入口，而这个
/// 产品卖的正是时间。
///
/// 走账本不走原地改：`correct_interval` 作废旧行、插带 `supersedes` 的修正行。
/// 于是这次修改本身出现在记录轴上（实体面板的 History），世界轴上只留新区间——
/// 旧区间是我们记错了，不是世界曾经的样子，两段并排会被读成换过两次岗。
pub async fn update_fact_time(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, fact_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<FactTimePatch>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    check_interval(&req)?;

    // 归属与状态校验：本 KB 的、未作废的事实才可改。派生事实不在此列——
    // 它的区间来自前提，改它等于改一个算出来的值，下一轮推理就会覆盖
    let before: Option<FactInterval> = sqlx::query_as(
        "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision
         FROM facts
         WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL AND derived_by_rule IS NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Db)?;
    let Some(before) = before else {
        return Err(AppError::NotFound.into());
    };
    // 一字未改就不写一条修正进账本：History 上多一行「改过」而区间没动，
    // 读者会去找那个不存在的差异
    if before.valid_from == req.valid_from
        && before.valid_from_precision == req.valid_from_precision
        && before.valid_to == req.valid_to
        && before.valid_to_precision == req.valid_to_precision
    {
        return Ok(Json(json!({ "ok": true, "unchanged": true })));
    }

    let snap = crate::api::review_routes::fact_snapshot(&state, kb_id, fact_id).await;
    let validity = utopia_store::graph::Validity {
        from: req.valid_from,
        from_precision: req.valid_from_precision.as_deref(),
        to: req.valid_to,
        to_precision: req.valid_to_precision.as_deref(),
    };
    let Some(corrected) =
        utopia_store::temporal::correct_interval(&state.pool, fact_id, validity).await?
    else {
        // 并发：另一处已经改写或作废了它。让界面重取，不要把两次修正叠起来
        return Err(AppError::invalid(
            "fact_changed",
            "This fact was changed by someone else. Reload and try again.",
        )
        .into());
    };

    // 改完要重新对账：把起点往前挪可能撞上前任的开放区间，那正是时态引擎
    // 该管的事。复用搬移后的那条路径——换了区间与换了主宾一样，都让唯一性
    // 不变量第一次看到这次相撞
    let report =
        utopia_store::temporal::reconcile_moved_facts(&state.pool, kb_id, &[corrected]).await?;

    if let Some(mut d) = snap {
        d["from"] = json!({
            "valid_from": before.valid_from.map(|t| t.to_rfc3339()),
            "valid_from_precision": before.valid_from_precision,
            "valid_to": before.valid_to.map(|t| t.to_rfc3339()),
            "valid_to_precision": before.valid_to_precision,
        });
        d["to"] = json!({
            "valid_from": req.valid_from.map(|t| t.to_rfc3339()),
            "valid_from_precision": req.valid_from_precision,
            "valid_to": req.valid_to.map(|t| t.to_rfc3339()),
            "valid_to_precision": req.valid_to_precision,
        });
        if let Some(note) = req.note.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            d["note"] = json!(note);
        }
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "fact.time_corrected",
            "fact",
            Some(fact_id),
            d,
        )
        .await;
    }

    state.emit_review(kb_id);
    Ok(Json(json!({
        "ok": true,
        "fact_id": corrected,
        "closed": report.corrected.len(),
        "conflicts": report.conflicts,
    })))
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
