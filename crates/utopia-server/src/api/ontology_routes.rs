//! 本体编辑器 API：类型/关系 CRUD + 未匹配统计 + LLM 扩展建议。
//! 查看 = viewer；修改 = editor（本体直接影响后续抽取的白名单）。

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
    let (entity_types, relation_types) = tokio::try_join!(
        utopia_store::ontology::entity_type_views(&state.pool, kb_id),
        utopia_store::ontology::relation_type_views(&state.pool, kb_id),
    )?;
    Ok(Json(json!({
        "entity_types": entity_types,
        "relation_types": relation_types,
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
    /// 全部父类，**第一个当主父**（左栏画在那一支下）。
    /// 界面上写明了这条，所以不额外给一个"选主父"的控件
    #[serde(default)]
    pub parents: Vec<Uuid>,
    /// 与它互斥的类。**缺省 = 不动**——不管互斥的调用方（提案采纳、冷启动）
    /// 不该因为一次建类就把已有的声明清空
    #[serde(default)]
    pub disjoint: Option<Vec<Uuid>>,
    /// 语义指引，注入抽取 prompt
    #[serde(default)]
    pub description: Option<String>,
}

/// 编辑器改了本体之后：先补向量索引，再排对齐。
///
/// **新元素要有向量，对齐的短名单才看得见它。** 候选多于短名单长度时按向量挑
/// （`phrase_alignment::shortlist`），类别词的候选类也按向量检索
/// （`type_alignment::candidates_for`）；没向量的元素永远进不了名单，形状的指纹就不变，
/// 建了也不重判——第一次真跑 `bordered_by` 就是这样（0061 cut 1.1 的采纳路径已经补了，
/// 这里是编辑器的四个入口）。描述改了也算：向量按「当时嵌的原文」判陈，改了描述的行
/// 会被同一个任务重嵌。
///
/// **排任务，不就地跑。** 就地嵌一次要付一趟嵌入请求的固定开销，本体页一口气建 28 条
/// 属性就是 28 趟；排任务的话一批编辑只嵌一次。挡的只是排着的（`enqueue_unless_queued_after`），
/// 不挡在跑的：在跑的那份已经读完待嵌集合，这条编辑它看不见，得再排一份。
/// 去抖比对齐短，对齐任务开跑前还会看一眼这个任务有没有排着或跑着，排着就等
/// （见 `align_phrases_reasking` / `align_types_reasking`）。
async fn reindex_then_align(
    state: &AppState,
    kb_id: Uuid,
    align_kind: &str,
) -> Result<(), AppError> {
    utopia_store::jobs::enqueue_unless_queued_after(
        &state.pool,
        "embed_ontology",
        json!({ "kb_id": kb_id }),
        std::time::Duration::from_secs(2),
    )
    .await?;
    utopia_store::jobs::enqueue_unless_pending(
        &state.pool,
        align_kind,
        json!({ "kb_id": kb_id }),
        // 去抖：一批编辑（导一个包、建一串属性）只排一次
        std::time::Duration::from_secs(5),
    )
    .await?;
    Ok(())
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
        .ok_or_else(|| AppError::invalid("key_required", "key is required"))?;
    let id = utopia_store::ontology::create_entity_type(
        &state.pool,
        kb_id,
        key,
        req.label.trim(),
        // 不给颜色就按 key 取一个，而不是所有类共用一个灰蓝
        req.color
            .as_deref()
            .unwrap_or_else(|| utopia_store::palette::color_for_key(key)),
        req.shape.as_deref().unwrap_or("circle"),
        &req.parents,
        req.description.as_deref().unwrap_or("").trim(),
    )
    .await?;
    // 互斥缺省 = 不动：不管它的调用方（提案采纳、冷启动）不该因为建一个类
    // 就把已有的声明清空
    if let Some(d) = req.disjoint.as_deref() {
        utopia_store::ontology::set_disjoint_for(&state.pool, kb_id, id, d).await?;
    }
    // 审计只记不阻断
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "entity_type.created",
        "entity_type",
        Some(id),
        json!({ "key": key, "label": req.label.trim() }),
    )
    .await;
    // 本体多了一个类：类别词的绑定里那些「没有」和「没定」的要重判（0044 对齐第一片）。
    // 先补它的向量，候选类的检索才找得到它
    let _ = reindex_then_align(&state, kb_id, "align_types").await;
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
        // **不给颜色 = 保持原色**，不是重置。这里按 id 改，压根拿不到 key；
        // 而从前无条件写 "#8ea5bd" 意味着任何一次不带颜色的改名
        // 都会把用户挑过的颜色抹掉
        req.color.as_deref(),
        req.shape.as_deref().unwrap_or("circle"),
        &req.parents,
        req.description.as_deref().unwrap_or("").trim(),
    )
    .await?;
    if let Some(d) = req.disjoint.as_deref() {
        utopia_store::ontology::set_disjoint_for(&state.pool, kb_id, id, d).await?;
    }
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "entity_type.updated",
        "entity_type",
        Some(id),
        json!({ "label": req.label.trim(), "color": req.color, "shape": req.shape,
                "description": req.description }),
    )
    .await;
    // 类的定义改了：绑到它的类别词过期，重判。标签或描述改了向量也陈了，先重嵌
    let _ = reindex_then_align(&state, kb_id, "align_types").await;
    // 对账可能在同一事务里收/开过违规行——审核队列变了，说一声（0062）
    state.emit_review(kb_id);
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
        &state.pool,
        Some(kb_id),
        user.id,
        "entity_type.deleted",
        "entity_type",
        Some(id),
        json!({}),
    )
    .await;
    // 对账可能在同一事务里收/开过违规行——审核队列变了，说一声（0062）
    state.emit_review(kb_id);
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
    /// 其余四条 OWL 公理。**必须能在界面上编**——推理机（0002）的判据全部
    /// 来自这几位，而它们从前只能靠导入 OWL 文件带进来：一个在界面上手工建
    /// 本体的用户，永远开不了那台机器。
    ///
    /// 与 `functional` / `inverse_functional` 并排，因为它们本来就是同一族，
    /// 只是那两个先落库了
    #[serde(default)]
    pub is_transitive: bool,
    #[serde(default)]
    pub is_symmetric: bool,
    #[serde(default)]
    pub is_asymmetric: bool,
    #[serde(default)]
    pub is_irreflexive: bool,
    /// 指向另一个关系的两条（`inverseOf` / `subPropertyOf`）。
    ///
    /// **缺省 = 清空，与上面六位同一条规矩。** 它们是同一个表单一次提交的
    /// 一组声明；一半覆盖一半保留，会让「我把逆去掉了」和「我没碰逆」
    /// 长得一模一样。属性表单不填这两个，而属性本来就不许有——
    /// store 层会拦，落库也照样是 NULL
    #[serde(default)]
    pub inverse_of: Option<Uuid>,
    #[serde(default)]
    pub sub_property_of: Option<Uuid>,
    #[serde(default)]
    pub description: Option<String>,
    /// relation | attribute（创建时定死，更新时忽略）
    #[serde(default)]
    pub kind: Option<String>,
    /// 可以当主语的类。attribute 至少一个；relation 留空 = 不限。
    /// **两者都缺省才是不动**：签名是一组一起提交的——只送 ranges 会把
    /// domains 清空，不是「ranges 动、domains 留」
    #[serde(default)]
    pub domains: Option<Vec<Uuid>>,
    /// 这条关系的边能带哪些属性（0037）：属性定义的 id。None = 不动
    #[serde(default)]
    pub qualifiers: Option<Vec<Uuid>>,
    /// 可以当宾语的类。只对 relation 有意义；与 domains 同一条成组提交规矩
    #[serde(default)]
    pub ranges: Option<Vec<Uuid>>,
    /// attribute 专用：text | number | date | bool
    #[serde(default)]
    pub datatype: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
}

impl RelationTypeReq {
    /// 公理打包。**散着传迟早传错顺序**——六位都是 bool，编译器帮不上忙；
    /// 后两位都是 `Option<Uuid>`，一样。
    fn axioms(&self) -> utopia_core::models::RelationAxioms {
        utopia_core::models::RelationAxioms {
            functional: self.functional,
            inverse_functional: self.inverse_functional,
            transitive: self.is_transitive,
            symmetric: self.is_symmetric,
            asymmetric: self.is_asymmetric,
            irreflexive: self.is_irreflexive,
            inverse_of: self.inverse_of,
            sub_property_of: self.sub_property_of,
        }
    }
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
        .ok_or_else(|| AppError::invalid("key_required", "key is required"))?;
    let kind = req.kind.as_deref().unwrap_or("relation");
    let id = utopia_store::ontology::create_relation_type(
        &state.pool,
        kb_id,
        key,
        req.label.trim(),
        &req.temporal,
        req.axioms(),
        req.description.as_deref().unwrap_or("").trim(),
        kind,
        req.domains.as_deref().unwrap_or(&[]),
        req.ranges.as_deref().unwrap_or(&[]),
        req.datatype.as_deref(),
        req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    )
    .await?;
    if let Some(q) = req.qualifiers.as_deref() {
        utopia_store::ontology::set_relation_qualifiers(&state.pool, kb_id, id, q).await?;
    }
    // 多了一个属性：判成 none / undecided 的签名也许对得上了（0044 对齐第二片）。
    // 先补它的向量，短名单才看得见它
    reindex_then_align(&state, kb_id, "align_phrases").await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "relation_type.created",
        "relation_type",
        Some(id),
        json!({ "key": key, "label": req.label.trim(), "temporal": req.temporal, "kind": kind }),
    )
    .await;
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
        req.axioms(),
        req.description.as_deref().unwrap_or("").trim(),
        req.datatype.as_deref(),
        req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        // 请求里没带这两个字段就不动它们——属性表单不管 domain
        req.domains.as_deref(),
        req.ranges.as_deref(),
    )
    .await?;
    if let Some(q) = req.qualifiers.as_deref() {
        utopia_store::ontology::set_relation_qualifiers(&state.pool, kb_id, id, q).await?;
    }
    // 属性改了定义或域/值域：绑到它的签名过期，判成 none 的也许对得上了。
    // 标签或描述改了向量也陈了，先重嵌（只改域/值域的话补齐任务一查就退）
    reindex_then_align(&state, kb_id, "align_phrases").await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "relation_type.updated",
        "relation_type",
        Some(id),
        json!({ "label": req.label.trim(), "temporal": req.temporal,
                "functional": req.functional, "inverse_functional": req.inverse_functional,
                "description": req.description }),
    )
    .await;
    // 对账可能在同一事务里收/开过违规行——审核队列变了，说一声（0062）
    state.emit_review(kb_id);
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
        &state.pool,
        Some(kb_id),
        user.id,
        "relation_type.deleted",
        "relation_type",
        Some(id),
        json!({}),
    )
    .await;
    // 对账可能在同一事务里收/开过违规行——审核队列变了，说一声（0062）
    state.emit_review(kb_id);
    Ok(Json(json!({ "ok": true })))
}

/// 一条谓词的一端挂着两个以上开放值的持有者（#341）。
///
/// 本体自己长出来的库里没人声明过唯一性，接任不会闭合前任，三个人同时在管
/// 一个项目——而「谁在管」正是这个产品的题。引擎永不自动推断 functional
/// （bootstrap_ontology.rs 写了为什么），所以它只能被**问**：这里把证据摆出来
/// ——哪条谓词、哪一端、多少持有者、对账会闭合几条、几条要进人审——人决定。
/// `declared` 为真的那些是声明了却还没对过账的（导入的、声明之前就在的）。
pub async fn uniqueness_candidates(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let cands = utopia_store::temporal::uniqueness_candidates(&state.pool, kb_id).await?;
    let items: Vec<serde_json::Value> = cands
        .iter()
        .map(|c| {
            json!({
                "predicate_id": c.predicate_id,
                "key": c.key,
                "label": c.label,
                "kind": c.kind,
                "side": c.side,
                "axiom": if c.side == "subject" { "functional" } else { "inverse_functional" },
                "declared": c.declared,
                "holders": c.holders,
                "open_facts": c.open_facts,
                "would_close": c.would_close,
                "would_review": c.would_review,
                "examples": c.examples.iter().map(|e| json!({
                    "holder": e.holder,
                    "values": e.values.iter().map(|v| json!({
                        "fact_id": v.fact_id,
                        "name": v.name,
                        "valid_from": v.valid_from.map(|t| t.to_rfc3339()),
                        "confidence": v.confidence,
                    })).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "candidates": items })))
}

/// 补上声明之后，把这条谓词已经在账上的开放行对一遍（#341）。
///
/// 与落库时同一条路：作废 + 改写，supersedes 链回旧行，记录轴倒回声明之前
/// 仍看得见原来的行；拿不准的进人审。谓词没有唯一性声明时拒绝——
/// 声明是人的事，这里只执行。响应里报闭合了几条、几条进了人审。
pub async fn reconcile_relation_type(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let report = utopia_store::temporal::reconcile_predicate(&state.pool, kb_id, id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "relation_type.reconciled",
        "relation_type",
        Some(id),
        json!({ "corrected": report.corrected.len(), "conflicts": report.conflicts }),
    )
    .await;
    Ok(Json(json!({
        "corrected": report.corrected.len(),
        "conflicts": report.conflicts,
        "corrected_ids": report.corrected,
    })))
}

/// 还等着人看的提案，按接口原来的形状拼回去。
///
/// 前端因此不必区分「刚算出来的」与「上次存下的」——两者同一个类型、同一套渲染。
pub async fn stored_proposals(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let stored = utopia_store::ontology::open_proposals(&state.pool, kb_id).await?;
    let mut out = json!({
        "entity_types": [], "relation_types": [], "attribute_types": [], "map_to": []
    });
    for p in stored {
        // 只给代理提的（0061）。旧的 Suggest 那条路已经退役，它留在老库里的提案没有
        // 采纳的入口了，摆出来只是一排点不动的按钮
        if p.proposed_by != "agent" {
            continue;
        }
        if let Some(arr) = out.get_mut(&p.section).and_then(|v| v.as_array_mut()) {
            let mut item = p.payload;
            // 多带三格：谁提的、服务哪些问题、会绑上哪些形状
            if let Some(o) = item.as_object_mut() {
                o.insert("proposed_by".into(), json!("agent"));
                o.insert("serves".into(), json!(p.serves));
                o.insert("signatures".into(), p.signatures);
            }
            arr.push(item);
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct DecideProposalReq {
    pub section: String,
    pub key: String,
    /// adopted | rejected
    pub status: String,
    /// 拒绝的理由（0061：下一轮代理读得到）。采纳时忽略
    #[serde(default)]
    pub reason: Option<String>,
}

/// 一条提案有人表态了。**改状态不删行**：采纳发生过、拒绝也发生过，
/// 而拒绝留痕正是下一轮 Suggest 不再把它刷回待看的依据。
pub async fn decide_proposal(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<DecideProposalReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    if !matches!(req.status.as_str(), "adopted" | "rejected") {
        return Err(AppError::invalid("bad_status", "status 只能是 adopted 或 rejected").into());
    }
    let reason = req
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if req.status == "rejected" && reason.is_some() {
        utopia_store::ontology::reject_proposal(
            &state.pool,
            kb_id,
            &req.section,
            &req.key,
            reason,
            user.id,
        )
        .await?;
    } else {
        utopia_store::ontology::decide_proposal(
            &state.pool,
            kb_id,
            &req.section,
            &req.key,
            &req.status,
            user.id,
        )
        .await?;
    }
    Ok(Json(json!({ "ok": true })))
}

/// OWL 导入：先看会发生什么，确认了才落库。
///
/// **绝不让上传一个文件就不可逆地改掉本体**——预览与落库走同一个 plan，
/// 两条独立路径迟早分叉，而分叉的后果是确认之后发生的事与刚看过的不一样。
pub async fn preview_import(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    multipart: axum::extract::Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let (filename, bytes) = read_upload(multipart).await?;
    let (plan, _, _) = crate::owl_import::plan(&state, kb_id, &filename, &bytes).await?;
    Ok(Json(json!({ "filename": filename, "plan": plan })))
}

pub async fn apply_import(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    multipart: axum::extract::Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let (filename, bytes) = read_upload(multipart).await?;
    // 导入会改判据（公理链、domain/range、父边）：先把旧本体下会检出的违规
    // 记下来（0062），apply 落库后按新本体对账——在旧判据下检出过的陈行以
    // criterion_changed 收场，不是整批抹掉；对账失败如实报错，不假装干净
    // 基线跑在同一条事务里：autocommit 连接上每条语句各看一个快照，
    // 半途有别的提交进来会把两个时刻混成一份基线
    let mut baseline_tx = state.pool.begin().await?;
    let was_detected = utopia_store::reasoning::detection_keys(&mut baseline_tx, kb_id).await?;
    baseline_tx.commit().await?;
    let (import_id, plan) =
        match crate::owl_import::apply(&state, kb_id, user.id, &filename, &bytes).await {
            Ok(v) => v,
            Err(e) => {
                // apply 是多笔提交：中途失败时一部分判据已经落了。尽力对账——
                // 已提交的判据是「当前本体」，open 行不能带着旧判据过夜。对账
                // 自身失败只记告警，原错误照样返回，不吞
                match state.pool.begin().await {
                    Ok(mut tx) => {
                        if let Err(re) = utopia_store::reasoning::reconcile_ontology(
                            &mut tx,
                            kb_id,
                            &was_detected,
                        )
                        .await
                        {
                            tracing::warn!(error = ?re, "导入失败后的违规对账也没跑成");
                        } else if let Err(ce) = tx.commit().await {
                            tracing::warn!(error = ?ce, "导入失败后的违规对账提交失败");
                        }
                    }
                    Err(be) => tracing::warn!(error = ?be, "导入失败后连对账事务都开不了"),
                }
                return Err(e.into());
            }
        };
    // apply 自己分多笔提交，这笔事务只做对账：`open` 的意思是对账于当前本体
    let mut tx = state.pool.begin().await?;
    let violations =
        utopia_store::reasoning::reconcile_ontology(&mut tx, kb_id, &was_detected).await?;
    tx.commit().await?;
    state.emit_review(kb_id);
    Ok(Json(
        json!({ "import_id": import_id, "plan": plan, "violations": violations }),
    ))
}

pub async fn list_imports(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let imports = utopia_store::ontology::list_imports(&state.pool, kb_id).await?;
    Ok(Json(json!({ "imports": imports })))
}

/// 取 multipart 里的第一个文件。上限 8 MB——FOAF 44 KB、DCTerms 48 KB，
/// FIBO 那种大部头分模块也在几百 KB 量级；再大多半是传错了东西。
const MAX_ONTOLOGY_BYTES: usize = 8 * 1024 * 1024;

async fn read_upload(
    mut multipart: axum::extract::Multipart,
) -> Result<(String, Vec<u8>), AppError> {
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::invalid_detail("bad_upload", "Could not read the upload", e.to_string())
    })? {
        let Some(filename) = field.file_name().map(String::from) else {
            continue;
        };
        let bytes = field.bytes().await.map_err(|e| {
            AppError::invalid_detail(
                "upload_read_failed",
                "Could not read the uploaded file",
                e.to_string(),
            )
        })?;
        if bytes.len() > MAX_ONTOLOGY_BYTES {
            return Err(AppError::invalid(
                "file_too_large",
                "Ontology file is too large (max 8 MB)",
            ));
        }
        if bytes.is_empty() {
            return Err(AppError::invalid("empty_file", "Ontology file is empty"));
        }
        return Ok((filename, bytes.to_vec()));
    }
    Err(AppError::invalid("no_files", "No file in the upload"))
}

/// 类型消解的**只算不写**那一步：每个待精化实体的画像与候选类。
///
/// 跟本体导入同一个模式：先看计划，再决定落不落。在这里它还多一层用处——
/// 检索找不着的时候，回执里带着"我们拿什么去找的"，第一眼就知道该改画像
/// 还是该改类的描述。
pub async fn type_resolution_preview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let items = crate::type_resolution::preview(&state, kb_id).await?;
    Ok(Json(json!({ "items": items })))
}

/// 跑一遍类型消解并落库：检索候选 → 裁决 → 三档处置。
///
/// 与 preview 分开是本仓库既有的形状（本体导入也是先看计划再落库）。这里还多
/// 一层理由：改类**不进时间轴**，所以它不像事实改写那样在实体历史里自己显形，
/// 先看一眼再动是唯一能看见它的时机。
pub async fn type_resolution_apply(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let outcome = crate::type_resolution::resolve(&state, kb_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "ontology.types_resolved",
        "knowledge_base",
        Some(kb_id),
        json!({ "retyped": outcome.retyped, "for_review": outcome.for_review.len(),
                "left_alone": outcome.left_alone.len(), "batch": outcome.batch }),
    )
    .await;
    Ok(Json(json!(outcome)))
}

/// 撤销一次类型消解：把那一批实体放回原来的类。
pub async fn type_resolution_undo(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, batch_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let reverted = utopia_store::resolution::unadopt_types(&state.pool, kb_id, batch_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "ontology.types_resolution_reverted",
        "knowledge_base",
        Some(kb_id),
        json!({ "batch": batch_id, "reverted": reverted }),
    )
    .await;
    Ok(Json(json!({ "reverted": reverted })))
}

#[derive(Deserialize)]
pub struct ApproveRefinementReq {
    pub from_type_id: Uuid,
    pub to_type_id: Uuid,
    /// 这一次要一并改掉的实体。**认可的是类对，改的是实体**——
    /// 两件事分开，所以调用方可以只认可规则而暂不动任何实体
    #[serde(default)]
    pub entity_ids: Vec<Uuid>,
}

/// 认可一个"粗类 → 细类"的配对，并把随请求带来的实体改过去。
///
/// 待人工那一档由"跨没跨分类轴"触发，而实测那条判据测的往往是**种子类跟导入
/// 词汇表连没连上**，不是风险——schema.org 的 Place 另起 key，于是每个城市都
/// 要问一遍。配对认可一次就不再问：那是类与类之间的判断，实体只是碰巧撞上它。
pub async fn approve_refinement(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<ApproveRefinementReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::resolution::approve_refinement(
        &state.pool,
        kb_id,
        req.from_type_id,
        req.to_type_id,
        user.id,
    )
    .await?;
    let picks: Vec<(Uuid, Uuid)> = req
        .entity_ids
        .iter()
        .map(|id| (*id, req.to_type_id))
        .collect();
    let (batch, moved) = if picks.is_empty() {
        (None, 0)
    } else {
        let (b, n) =
            utopia_store::resolution::retype_entities(&state.pool, kb_id, &picks, Some(user.id))
                .await?;
        (Some(b), n)
    };
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "ontology.refinement_approved",
        "entity_type",
        Some(req.to_type_id),
        json!({ "from": req.from_type_id, "to": req.to_type_id, "moved": moved }),
    )
    .await;
    Ok(Json(json!({ "moved": moved, "batch": batch })))
}
