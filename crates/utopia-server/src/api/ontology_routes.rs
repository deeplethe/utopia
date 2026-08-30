//! 本体编辑器 API：类型/关系 CRUD + 未匹配统计 + LLM 扩展建议。
//! 查看 = viewer；修改 = editor（本体直接影响后续抽取的白名单）。

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_core::AppError;
use utopia_llm::ChatMessage;
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
    let entity_types = utopia_store::ontology::entity_type_views(&state.pool, kb_id).await?;
    let relation_types = utopia_store::ontology::relation_type_views(&state.pool, kb_id).await?;
    let misses = utopia_store::ontology::list_misses(&state.pool, kb_id).await?;
    // 已忽略的单列一路：抑制照旧（提案与自动扩本体只看上面那份），
    // 但让人看得见抑制掉了什么、现在涨到多少
    let dismissed_misses =
        utopia_store::ontology::list_dismissed_misses(&state.pool, kb_id).await?;
    Ok(Json(json!({
        "entity_types": entity_types,
        "relation_types": relation_types,
        "misses": misses,
        "dismissed_misses": dismissed_misses,
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
        .ok_or_else(|| AppError::invalid("key_required", "key is required"))?;
    let id = utopia_store::ontology::create_entity_type(
        &state.pool,
        kb_id,
        key,
        req.label.trim(),
        req.color.as_deref().unwrap_or("#8ea5bd"),
        req.shape.as_deref().unwrap_or("circle"),
        &req.parents,
        req.description.as_deref().unwrap_or("").trim(),
    )
    .await?;
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
        &req.parents,
        req.description.as_deref().unwrap_or("").trim(),
    )
    .await?;
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
    /// 可以当主语的类。attribute 至少一个；relation 留空 = 不限。
    /// **更新时缺省 = 不动**，所以是 Option 而不是 Vec——不管 domain 的
    /// 调用方（属性表单）不该因为一次改名就把 domain 清空
    #[serde(default)]
    pub domains: Option<Vec<Uuid>>,
    /// 可以当宾语的类。只对 relation 有意义
    #[serde(default)]
    pub ranges: Option<Vec<Uuid>>,
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
        .ok_or_else(|| AppError::invalid("key_required", "key is required"))?;
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
        req.domains.as_deref().unwrap_or(&[]),
        req.ranges.as_deref().unwrap_or(&[]),
        req.datatype.as_deref(),
        req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    )
    .await?;
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
        req.functional,
        req.inverse_functional,
        req.description.as_deref().unwrap_or("").trim(),
        req.datatype.as_deref(),
        req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        // 请求里没带这两个字段就不动它们——属性表单不管 domain
        req.domains.as_deref(),
        req.ranges.as_deref(),
    )
    .await?;
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
    utopia_store::ontology::dismiss_miss(&state.pool, kb_id, &req.kind, &req.key).await?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn restore_miss(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<DismissMissReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    utopia_store::ontology::restore_miss(&state.pool, kb_id, &req.kind, &req.key).await?;
    Ok(Json(json!({ "ok": true })))
}

/// LLM 本体扩展建议：现有本体 + 未匹配统计 → 提案（人审后经 create 端点合入）。
/// `locale` 是**调用方**说的，不是后端的设置。reason 只给人看，而人就在这次请求的
/// 另一端；界面语言在客户端（docs/decisions/0004），所以它只能这样传进来。
#[derive(Deserialize, Default)]
pub struct SuggestReq {
    #[serde(default)]
    pub locale: Option<String>,
}

pub async fn suggest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    body: Option<Json<SuggestReq>>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let locale = body
        .and_then(|Json(b)| b.locale)
        .filter(|l| matches!(l.as_str(), "en" | "zh"))
        .unwrap_or_else(|| "en".into());
    // 人工那条路 min_docs = 0：面板上「出现在 1 篇」这个数字是显示给人看的，
    // 他自己判断得了。替他滤掉，只是让他少一条信息
    let proposals = build_proposals(&state, kb_id, &locale, 0).await?;
    // 算完就写下来（0049）。从前这批结果只回给前端、存进一个 useState，
    // 刷新一次就没了——而重算要再调一次模型，且未必给出同一批归并
    persist_proposals(&state, kb_id, &proposals).await;
    Ok(Json(proposals))
}

/// 四个小节里的每一条都记一行。**失败不拦住返回**——提案已经算出来了，
/// 存不下只是下次要重算，把整个请求判失败反而把算出来的也丢了。
async fn persist_proposals(state: &AppState, kb_id: Uuid, proposals: &serde_json::Value) {
    const SECTIONS: [&str; 4] = [
        "entity_types",
        "relation_types",
        "attribute_types",
        "map_to",
    ];
    let mut rows: Vec<(String, String, serde_json::Value)> = Vec::new();
    for section in SECTIONS {
        let Some(items) = proposals.get(section).and_then(|v| v.as_array()) else {
            continue;
        };
        for it in items {
            // key 是这一条的身份（迁移里的唯一约束用的就是它）。没有 key 的
            // 存不了，也没法在采纳时对回来——跳过而不是编一个
            let Some(key) = it.get("key").and_then(|k| k.as_str()) else {
                continue;
            };
            rows.push((section.to_string(), key.to_string(), it.clone()));
        }
    }
    if let Err(e) = utopia_store::ontology::save_proposals(&state.pool, kb_id, &rows).await {
        tracing::warn!(%kb_id, error = %e, "本体提案落库失败，这一批只在本次响应里");
    }
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
        if let Some(arr) = out.get_mut(&p.section).and_then(|v| v.as_array_mut()) {
            arr.push(p.payload);
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
    utopia_store::ontology::decide_proposal(
        &state.pool,
        kb_id,
        &req.section,
        &req.key,
        &req.status,
        user.id,
    )
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// 生成本体扩展提案。人工点 Suggest 与冷启动自动扩本体走同一条路——
/// 自动那条不该是另一套判断，只是少了点头那一步。
/// 每个说法检索几个候选。
///
/// 小是刻意的：候选是给模型判断"是不是已经有了"用的，不是让它挑一个最像的凑合。
/// 开大了只会让它在一堆勉强相关的条目里硬选一个映射过去。
const CANDIDATES_PER_PROBE: i64 = 5;

/// `min_docs`：**只把出现在这么多篇文档里的说法交给模型**。0 = 全给。
///
/// 这一位存在的理由是它曾经不存在。`ProposedPredicate.doc_count` 的注释写着
/// 「自动扩展据此设门槛，人工提案只作参考不拦」，但接线没接完：
/// `bootstrap_ontology` 把门槛算出来只用于 `forms.len()` 判断值不值得跑一次
/// LLM，随后调这个函数时只传 kb_id，于是这里重查一遍**没过滤的**全量。
///
/// 实测后果（ai-timeline，348 块）：交给模型 526 个说法，其中 456 个只在一篇
/// 里出现过——**86.7% 是噪声**。模型从这堆里只挑出 9 个，`runs_on`（8 篇都有）
/// 和 `founded_by`（4 篇）没被挑中，275 条事实继续没有谓词。反方向也漏：
/// 5 个单篇说法被采纳了，其中两个还建成了新属性，门槛形同虚设。
pub async fn build_proposals(
    state: &AppState,
    kb_id: Uuid,
    reason_lang: &str,
    min_docs: i64,
) -> Result<serde_json::Value, AppError> {
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| AppError::invalid("no_chat_model", "Chat model not configured"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| AppError::invalid("no_chat_model", "Chat model not configured"))?;

    let mut misses = utopia_store::ontology::list_misses(&state.pool, kb_id).await?;
    // 表层谓词比 misses 多一样东西：它连着具体事实，所以提案能承诺"改写 N 条"。
    //
    // **两条线严格分开**，按宾语是实体还是字面值：`收购` 要的是一条关系，
    // `founding_date = "2015"` 要的是一个属性。混起来的后果具体——后者会被
    // 提成关系，于是长出一条指向「2015」这个假实体的边
    let mut forms = utopia_store::graph::proposed_predicates(&state.pool, kb_id).await?;
    let mut value_forms = utopia_store::graph::proposed_attributes(&state.pool, kb_id).await?;
    if min_docs > 0 {
        forms.retain(|f| f.doc_count >= min_docs);
        value_forms.retain(|f| f.doc_count >= min_docs);
        // **三份清单都要滤，滤一份等于没滤。** 同一个说法在提示词里出现两次：
        // miss 行（"seen N times"）和表层谓词行（"on N fact(s)"）。
        // `OntologyMiss` 没有文档维度，所以按活下来的说法集合筛——留下的是
        // 那些既跨了篇、又还没有谓词的。类那一路不动：`ProposedType`
        // 同样没有 doc_count，硬滤等于按另一个判据拦，而不是按这个
        let kept: std::collections::HashSet<&str> = forms.iter().map(|f| f.form.as_str()).collect();
        misses.retain(|m| m.kind != "relation_type" || kept.contains(m.key.as_str()));
    }
    if misses.is_empty() && forms.is_empty() && value_forms.is_empty() {
        return Ok(json!({
            "entity_types": [], "relation_types": [], "attribute_types": [], "map_to": []
        }));
    }

    // **本体的相关切片，不是它的全文。**
    //
    // 从前这里内联两串全量 key。两个毛病：一份 965 类的本体就是 1949 个 key
    // 的提示词；而且只有 key 没有描述，模型据此判断不了"这个说法是不是某个
    // 已有类型的同义"，于是它只会新建，本体里就稳定长出重复。
    //
    // 现在按每个说法各检索几个最近的候选，带着描述给它看。提示词大小从此
    // 与本体规模无关，而"已经有一个了"这件事第一次变得可判断。
    let _ = crate::ontology_index::refresh(state, kb_id).await;
    // 谓词那一路：表层说法 + 关系类的 miss。样例给向量更多着落——
    // 光一个 acquires 太短，带上"星云科技 → 深蓝存储"就有了语境
    let pred_probes: Vec<String> = forms
        .iter()
        .map(|f| match f.example.as_deref() {
            Some(ex) if !ex.is_empty() => format!("{} — {ex}", f.form),
            _ => f.form.clone(),
        })
        .chain(
            misses
                .iter()
                .filter(|m| m.kind == "relation_type")
                .map(|m| m.key.clone()),
        )
        .collect();
    // 类那一路。**必须分开检索**：两张表的描述写的是完全不同的东西，
    // 拿一个词表外的类名去关系里找，回来的一定是勉强相关的关系
    let class_probes: Vec<String> = misses
        .iter()
        .filter(|m| m.kind == "entity_type")
        .map(|m| m.key.clone())
        .collect();
    let per_pred = crate::ontology_index::nearest_for_each(
        state,
        kb_id,
        &pred_probes,
        CANDIDATES_PER_PROBE,
        crate::ontology_index::Target::Predicate(None),
    )
    .await
    .unwrap_or_default();
    let per_class = crate::ontology_index::nearest_for_each(
        state,
        kb_id,
        &class_probes,
        CANDIDATES_PER_PROBE,
        crate::ontology_index::Target::Class,
    )
    .await
    .unwrap_or_default();
    // 属性那一路：只在属性里找。这里若不限 kind，"成立日期"最近的往往是
    // 某条关系，模型就会把一个字面值映射到一条边上去
    let attr_probes: Vec<String> = value_forms
        .iter()
        .map(|f| match f.example.as_deref() {
            Some(ex) if !ex.is_empty() => format!("{} = {ex}", f.form),
            _ => f.form.clone(),
        })
        .chain(
            misses
                .iter()
                .filter(|m| m.kind == "attribute_type")
                .map(|m| m.key.clone()),
        )
        .collect();
    let per_attr = crate::ontology_index::nearest_for_each(
        state,
        kb_id,
        &attr_probes,
        CANDIDATES_PER_PROBE,
        crate::ontology_index::Target::Predicate(Some("attribute")),
    )
    .await
    .unwrap_or_default();
    // 并集去重：几个说法常常指向同一个候选，逐个说法各列一遍是白费令牌
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut candidate_lines: Vec<String> = Vec::new();
    // 模型抄回来的 key 要能对回本体：候选表同时按 key、归一化 key、标签建索引
    let mut by_name: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // 每个候选是关系还是属性。映射到已有类型时，采纳走哪条改写路径由它定
    let mut kind_of_key: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for c in per_pred
        .iter()
        .chain(per_class.iter())
        .chain(per_attr.iter())
        .flatten()
    {
        if !seen.insert(c.key.clone()) {
            continue;
        }
        by_name.insert(normalize_name(&c.key), c.key.clone());
        by_name.insert(normalize_name(&c.label), c.key.clone());
        // 关系行自带 relation / attribute，类行没有 kind
        let kind = c.kind.as_deref().unwrap_or("entity type");
        kind_of_key.insert(c.key.clone(), kind.to_string());
        let d = c.description.trim();
        // **标签只在它确实多说了点什么的时候才写。**
        // 导进来的本体里 label 常常只是 key 的驼峰写法（acquired_from /
        // acquiredFrom），两个几乎一样的名字并排摆着，模型抄走的就是错的那个。
        // 中文标签配英文 key 那种才是标签有信息量的情形，那时才留
        let name = if normalize_name(&c.label) == normalize_name(&c.key) {
            String::new()
        } else {
            format!(" ({})", c.label)
        };
        candidate_lines.push(if d.is_empty() {
            format!("- {} [{kind}]{name}", c.key)
        } else {
            format!("- {} [{kind}]{name}: {d}", c.key)
        });
    }
    // 一个候选都没有（没配嵌入模型，或本体是空的）时如实说，别让模型
    // 以为"本体里什么都没有"从而放开手建
    let candidates_block = if candidate_lines.is_empty() {
        "(no candidates retrieved — the ontology may be empty, or embeddings are unavailable)"
            .to_string()
    } else {
        candidate_lines.join(
            "
",
        )
    };
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

    // 表层谓词行带上事实数与样例：模型据此判断这是不是一个真关系，
    // 而 forms 字段让采纳时知道该改写哪些事实
    let form_lines: Vec<String> = forms
        .iter()
        .map(|f| {
            format!(
                "- \"{}\" on {} fact(s), e.g. {}",
                f.form,
                f.fact_count,
                f.example.as_deref().unwrap_or("-")
            )
        })
        .collect();

    // 字面值那一路多带两样：一条样例值（模型据此判断该是 number 还是 date），
    // 和这个说法实际挂在哪些类上。后者不是给模型看的，是采纳时直接拿来当
    // domain 的——属性的 domain 猜错，主语类型对不上就整条丢弃
    let value_lines: Vec<String> = value_forms
        .iter()
        .map(|f| {
            format!(
                "- \"{}\" on {} fact(s), value e.g. {}, seen on: {}",
                f.form,
                f.fact_count,
                f.example.as_deref().unwrap_or("-"),
                if f.domain_keys.is_empty() {
                    "-".to_string()
                } else {
                    f.domain_keys.join(", ")
                }
            )
        })
        .collect();

    let prompt = format!(
        "You are an ontology engineer.\n\
         \n\
         Below are the ontology entries closest in meaning to the unmatched wordings that \
         follow. This is a retrieved slice, not the whole ontology, so \"not in this list\" \
         does not mean \"not in the ontology\" — it means nothing close to it was found:\n{}\n\
         \n\
         During extraction, the LLM repeatedly produced types/relations OUTSIDE this ontology:\n{}\n\
         \n\
         These predicates were taken from the source text because nothing in the ontology fit. \
         Their facts are currently filed under \"related_to\", which says nothing:\n{}\n\
         \n\
         These carried a literal VALUE rather than pointing at another entity, so each one \
         wants an attribute, never a relation. Turning one into a relation manufactures an \
         entity out of the value — a node named \"2015\" that stands for nothing:\n{}\n\
         \n\
         Each wording gets exactly ONE answer. Do not both map it and propose for it, and do \
         not propose it as a relation and as an attribute — a wording listed above as carrying \
         a value is an attribute, full stop.\n\
         \n\
         For each wording, decide one of two things.\n\
         \n\
         **If one of the candidates above already means it, map to it** — name that \
         candidate's key in \"map_to\". Do this whenever the meaning matches even though the \
         spelling differs (\"founding date\" is founding_date; \"headquartered in\" is \
         location). Adding a second entry for a meaning the ontology already carries is the \
         worst outcome available here: it splits the same facts across two keys permanently, \
         and nothing downstream can tell they were the same.\n\
         \n\
         **Otherwise propose a new entry.** Rules:\n\
         - Merge near-duplicates into ONE relation and list every spelling it covers in \
           \"forms\" (e.g. available_on / available_from / \"available through\" are one relation).\n\
         - Skip generic verbs that carry no domain meaning (is, has, includes, provides, brings).\n\
         - A relation is worth adding when the ontology genuinely lacks that meaning, not merely \
           because a word was frequent.\n\
         - \"functional\" must be false unless the relation truly permits at most one object per \
           subject at a time. Getting this wrong makes the temporal engine manufacture conflicts.\n\
         - An attribute needs a \"datatype\": text, number, date or bool. Read it off the example \
           value. Choose text when unsure — a value that will not convert to the declared type \
           is dropped, and a date stored as text is still the value.\n\
         \n\
         Every proposal needs a \"description\" as well as a \"reason\", and they are not the \
         same thing. The reason argues for adding it and is read by a person. **The description \
         is injected verbatim into the extraction prompt and is the only thing telling the model \
         what belongs here** — write it as a definition: say what the type is, then say what it \
         is not and which existing type those cases belong to. A type that arrives with a weak \
         description becomes the next dumping ground.\n\
         \n\
         Output exactly one JSON object:\n\
         {{\"entity_types\":[{{\"key\":\"snake_case\",\"label\":\"Display Name\",\"description\":\"what belongs here, and what does not\",\"reason\":\"why add it\"}}],\n\
          \"relation_types\":[{{\"key\":\"snake_case\",\"label\":\"display label\",\"temporal\":\"state|event|eternal\",\"functional\":false,\"forms\":[\"surface spellings this covers\"],\"description\":\"what this relation asserts, and what it does not\",\"reason\":\"why add it\"}}],\n\
          \"attribute_types\":[{{\"key\":\"snake_case\",\"label\":\"display label\",\"datatype\":\"text|number|date|bool\",\"unit\":\"optional, e.g. CNY\",\"forms\":[\"surface spellings this covers\"],\"description\":\"what this attribute records, and what it does not\",\"reason\":\"why add it\"}}],\n\
          \"map_to\":[{{\"key\":\"an existing key, copied from the candidate list above\",\"forms\":[\"surface spellings that mean it\"],\"reason\":\"why these are the same thing\"}}]}}\n\
         \n\
         Language, and it overrides the skeleton above — that skeleton is written in English \
         only because these instructions are. Write every \"label\" and \"description\" in {}: \
         they become this knowledge base's own ontology, and the description is read by the \
         extraction model while it reads documents in that language. Write every \"reason\" \
         in {}: a person reads it. \"key\" and \"forms\" stay lowercase ASCII either way.",
        candidates_block,
        miss_lines.join("\n"),
        form_lines.join("\n"),
        value_lines.join("\n"),
        lang_name(&kb.ontology_lang),
        lang_name(reason_lang)
    );

    let reply = client
        .chat(&[ChatMessage {
            role: "user".into(),
            content: prompt,
        }])
        .await
        .map_err(AppError::Other)?;
    let block = utopia_extract::json_block(&reply).map_err(AppError::Other)?;
    let mut proposals: serde_json::Value =
        serde_json::from_str(&block).map_err(|e| AppError::Other(e.into()))?;
    resolve_map_targets(&mut proposals, &by_name, &kind_of_key);
    // 说法归哪一档，服务端说了算——它手里有事实。实测模型会把同一个
    // \"founded_in\" 既提成关系又提成属性，两条都采纳就是同一批事实被抢两次
    let value_only: std::collections::HashSet<&str> =
        value_forms.iter().map(|f| f.form.as_str()).collect();
    let entity_only: std::collections::HashSet<&str> =
        forms.iter().map(|f| f.form.as_str()).collect();
    keep_forms(&mut proposals, "relation_types", &entity_only, &value_only);
    keep_forms(&mut proposals, "attribute_types", &value_only, &entity_only);
    Ok(proposals)
}

/// 把 `map_to` 里的 key 对回本体真正的 key，对不上的整条丢掉。
///
/// 模型抄错很常见，而且抄的往往是候选行里挨着的那个标签——`acquiredFrom`
/// 而不是 `acquired_from`。**这一步必须在服务端做**：界面上那个"用已有的"
/// 按钮承诺的是把一批事实挂到某个已有谓词上，key 对不上时它只会报错，
/// 而承诺已经说出去了。对不上就不该显示这条。
/// 顺带标上目标是关系还是属性：采纳时两条路的改写不一样，而模型答的是
/// 一个 key，看不出这个 key 落在哪张表的哪一档。
fn resolve_map_targets(
    proposals: &mut serde_json::Value,
    by_name: &std::collections::HashMap<String, String>,
    kind_of_key: &std::collections::HashMap<String, String>,
) {
    let Some(items) = proposals.get_mut("map_to").and_then(|v| v.as_array_mut()) else {
        return;
    };
    items.retain_mut(|m| {
        let Some(raw) = m.get("key").and_then(|v| v.as_str()) else {
            return false;
        };
        match by_name.get(&normalize_name(raw)) {
            Some(real) => {
                m["kind"] = json!(kind_of_key
                    .get(real)
                    .map(String::as_str)
                    .unwrap_or("relation"));
                m["key"] = json!(real);
                true
            }
            None => {
                tracing::debug!(key = raw, "map_to 指向了候选之外的 key，丢弃");
                false
            }
        }
    });
}

/// 只保留说法确实属于这一档的提案，并把不属于的那些说法从 `forms` 里剔掉。
///
/// **判据在服务端手里**：一个说法带的是字面值还是实体宾语，事实里写着，
/// 不必问模型。实测模型会把同一个 `founded_in` 既提成关系又提成属性——
/// 两条都采纳的话，同一批事实被抢两次，先跑的那条赢，结果取决于循环顺序。
///
/// `forms` 全被剔光的提案整条丢掉：它承诺的"改写 N 条"已经是零了。
fn keep_forms(
    proposals: &mut serde_json::Value,
    section: &str,
    mine: &std::collections::HashSet<&str>,
    theirs: &std::collections::HashSet<&str>,
) {
    let Some(items) = proposals.get_mut(section).and_then(|v| v.as_array_mut()) else {
        return;
    };
    items.retain_mut(|p| {
        let Some(forms) = p.get_mut("forms").and_then(|v| v.as_array_mut()) else {
            // 没有 forms 的提案只是"加一个类型"，不改写事实，与归档无关
            return true;
        };
        forms.retain(|f| {
            let Some(s) = f.as_str() else { return false };
            // 另一档明确认领的才剔掉。两边都不认识的说法（比如来自 misses
            // 而不是表层谓词）原样留着——剔掉它等于替模型否决了一条提案
            !theirs.contains(s) || mine.contains(s)
        });
        !forms.is_empty()
    });
}

/// key 与标签的比较形式：小写，只留字母数字。
///
/// **分隔符整个扔掉**，因为要对齐的正是分隔符的差异：`acquiredFrom`、
/// `acquired_from`、`Acquired From` 都归到 `acquiredfrom`。折成下划线是不够的
/// ——驼峰里根本没有分隔符可折。
///
/// 只做写法对齐，不做同义判断——那是检索与模型的活。
fn normalize_name(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

#[derive(Deserialize)]
pub struct AdoptReq {
    pub key: String,
    /// true = 这个 key 指的是**已有**的关系/属性，别再建一个。
    ///
    /// 这一位是本体消解的落点：检索告诉模型本体里已经有 `founding_date` 了，
    /// 模型说"这些说法就是它"，采纳时只做改写那一半。少了这一位，同一个意思
    /// 会长出第二个 key，往后这批事实就永久分在两处。
    #[serde(default)]
    pub existing: bool,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_temporal")]
    pub temporal: String,
    /// 缺省 false，且刻意不由建议方决定——本体声明的唯一性会驱动时态引擎自动
    /// 闭合事实，猜错就是成批的假冲突（part_of 就这么烧过一次）
    #[serde(default)]
    pub functional: bool,
    #[serde(default)]
    pub inverse_functional: bool,
    /// 归入这个关系的表层说法（"available_on"、"available through"…）
    pub forms: Vec<String>,
    /// `relation`（缺省）或 `attribute`。
    ///
    /// 属性走另一条改写路径：宾语是字面值，要按 datatype 换算过才能落到
    /// 新属性上；domain 也不由请求带，从事实的主语类型里取
    #[serde(default)]
    pub kind: Option<String>,
    /// 属性专用：text | number | date | bool
    #[serde(default)]
    pub datatype: Option<String>,
    #[serde(default)]
    pub unit: Option<String>,
}

/// 采纳一个表层谓词：建关系类型 **并把等着它的 related_to 事实改写过去**。
///
/// 与单纯 create 的区别就在后半句。只建类型的话本体长大了、图没变好——
/// 那 57 条事实会继续是"有关联"。改写走追加（新行 + supersedes），
/// 实体历史里读得到"先记成 related to，后精化成 available on"。
///
/// `existing` 为真时跳过前半句：本体里已经有这个意思了，这一次只做改写。
/// 这是消解与增长的分界——同一个入口，因为对图做的事完全一样。
pub async fn adopt_predicate(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<AdoptReq>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    let key = req.key.trim();
    if req.forms.is_empty() {
        return Err(AppError::invalid("forms_required", "forms cannot be empty").into());
    }
    if req.kind.as_deref() == Some("attribute") {
        return adopt_attribute(&state, &user, kb_id, &req).await;
    }
    let predicate_id = if req.existing {
        // 按 key 找已有的那一条。找不到就报错而不是退回新建——
        // 前端说的是"映射到已有的"，静默改成新建就是它最不想要的结果
        utopia_store::ontology::relation_type_id_by_key(&state.pool, kb_id, key)
            .await?
            .ok_or_else(|| {
                AppError::invalid("unknown_relation_key", "no relation type with that key")
            })?
    } else {
        utopia_store::ontology::create_relation_type(
            &state.pool,
            kb_id,
            key,
            req.label.trim(),
            &req.temporal,
            req.functional,
            req.inverse_functional,
            req.description.as_deref().unwrap_or("").trim(),
            "relation",
            // 提案与冷启动只建关系，不声明 domain/range —— 留空 = 不限主宾类型
            &[],
            &[],
            None,
            None,
        )
        .await?
    };
    // **人工路径不对调主宾。**
    //
    // 不是因为它不需要——把 `produced_by` 映射到 `produces` 同样该对调——而是
    // 这里的 forms 是人在面板上勾的，完全可能同时勾了 `produced` 和 `produced_by`，
    // 而一个 swap 标志伺候不了混合。自动那条路不存在这个问题：同组说法共享屈折基，
    // 结尾有没有 `by` 必然一致。真要修得让 adopt 逐条判方向，那是另一件事。
    let (batch_id, remapped) = utopia_store::graph::adopt_proposed_predicates(
        &state.pool,
        kb_id,
        predicate_id,
        &req.forms,
        false,
    )
    .await?;
    // 采纳同时清掉对应的未匹配统计——本体已经覆盖它们了
    for form in &req.forms {
        let _ = utopia_store::ontology::clear_miss(&state.pool, kb_id, "relation_type", form).await;
    }
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "ontology.predicate_adopted",
        "relation_type",
        Some(predicate_id),
        json!({
            "key": key, "label": req.label, "forms": req.forms,
            "facts_remapped": remapped, "batch": batch_id,
        }),
    )
    .await;
    state.emit_review(kb_id);
    Ok(Json(
        json!({ "id": predicate_id, "remapped": remapped, "batch": batch_id }),
    ))
}

/// 撤销一次采纳：新写的行作废、旧行复活。关系类型留着（有事实指向过它，
/// 而且一个没人用的关系是惰性的）。
pub async fn unadopt_predicate(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, batch_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Editor).await?;
    // 归因要按关系类型找回这次撤销，所以先拿到它
    let predicate_id: Option<(Uuid,)> = sqlx::query_as(
        "SELECT predicate_id FROM fact_adoptions WHERE batch_id = $1 AND kb_id = $2 LIMIT 1",
    )
    .bind(batch_id)
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(utopia_core::AppError::Db)?;
    // 一次采纳可能同时产生事实改写与实体改类的批次，调用方拿到的是一串
    // 不分种类的批次号——这里两边都试，谁认领谁生效
    if predicate_id.is_none() {
        let n = utopia_store::resolution::unadopt_types(&state.pool, kb_id, batch_id).await?;
        let _ = utopia_store::audit::record(
            &state.pool,
            Some(kb_id),
            user.id,
            "ontology.adoption_reverted",
            "kb",
            Some(kb_id),
            json!({ "batch": batch_id, "entities_reverted": n }),
        )
        .await;
        state.emit_review(kb_id);
        return Ok(Json(json!({ "reverted": n })));
    }
    let reverted = utopia_store::graph::unadopt(&state.pool, kb_id, batch_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "ontology.adoption_reverted",
        "relation_type",
        predicate_id.map(|(id,)| id),
        json!({ "batch": batch_id, "facts_reverted": reverted }),
    )
    .await;
    state.emit_review(kb_id);
    Ok(Json(json!({ "reverted": reverted })))
}

/// 待认领的表层谓词：原文说过、本体没有、事实降级成了 related_to。
pub async fn proposed_predicates(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let forms = utopia_store::graph::proposed_predicates(&state.pool, kb_id).await?;
    Ok(Json(json!({ "forms": forms })))
}

/// 最近一次自动扩本体做了什么，以及还能不能撤销。
///
/// 默认开启的前提是它的动作**可见且可退**。只记在审计台账里不算可见——
/// 那是查证用的，不是通知用的。这里给 Ontology 页一条明确的横幅。
pub async fn last_auto_extension(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kb(&state, &user, kb_id, Role::Viewer).await?;
    let row: Option<(serde_json::Value, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT detail, created_at FROM audit_events
         WHERE kb_id = $1 AND action = 'ontology.bootstrapped'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(kb_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(AppError::Db)?;
    let Some((detail, at)) = row else {
        return Ok(Json(json!({ "run": null })));
    };
    // 已经被撤销干净的就不再提示——那一轮已经没有任何痕迹留在图上了
    let batches: Vec<Uuid> = detail
        .get("batches")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().and_then(|x| x.parse().ok()))
                .collect()
        })
        .unwrap_or_default();
    let (live,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM fact_adoptions
         WHERE kb_id = $1 AND batch_id = ANY($2) AND reverted_at IS NULL",
    )
    .bind(kb_id)
    .bind(&batches)
    .fetch_one(&state.pool)
    .await
    .map_err(AppError::Db)?;
    if live == 0 {
        return Ok(Json(json!({ "run": null })));
    }
    Ok(Json(json!({ "run": {
        "at": at,
        "relations": detail.get("relations"),
        "classes": detail.get("classes"),
        "facts_remapped": detail.get("facts_remapped"),
        "batches": batches,
    }})))
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
    let (import_id, plan) =
        crate::owl_import::apply(&state, kb_id, user.id, &filename, &bytes).await?;
    Ok(Json(json!({ "import_id": import_id, "plan": plan })))
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

/// 语言代码 → 提示词里写给模型看的名字。模型认得懂 "Chinese"，未必认得懂 "zh"。
fn lang_name(code: &str) -> &'static str {
    match code {
        "zh" => "Chinese",
        _ => "English",
    }
}

/// 采纳一个**字面值**说法：建（或指向已有的）属性，并把等着它的事实改挂过去。
///
/// 跟关系那条路有三处不同，每一处都是属性特有的：
///
/// 1. **domain 从数据里取，不由请求带。** 属性必须声明能挂在哪些类下，而猜错
///    的代价是硬的——主语类型对不上就整条丢弃（`attr_domain_mismatch`）。
///    这些事实的主语现在是什么类是事实不是判断，直接读。
/// 2. **值要按 datatype 换算。** 库里存的是抽取当时的原样（字符串 "2015"），
///    落到一个 date 属性上得先变成日期。
/// 3. **换不出来的不改写。** 宁可让它继续没有谓词，等下一次，也不把
///    一个换不动的值硬塞进类型化的属性里——那是"宁缺勿脏"的同一条。
async fn adopt_attribute(
    state: &AppState,
    user: &utopia_core::models::User,
    kb_id: Uuid,
    req: &AdoptReq,
) -> ApiResult<Json<serde_json::Value>> {
    let spec = AttributeAdoption {
        key: req.key.trim(),
        label: req.label.trim(),
        description: req.description.as_deref().unwrap_or("").trim(),
        datatype: req.datatype.as_deref().unwrap_or("text"),
        unit: req.unit.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        forms: &req.forms,
        existing: req.existing,
    };
    let done = adopt_attribute_core(state, kb_id, &spec).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        user.id,
        "ontology.attribute_adopted",
        "relation_type",
        Some(done.attribute_id),
        json!({ "key": spec.key, "forms": req.forms, "existing": req.existing,
                "remapped": done.remapped, "unconvertible": done.unconvertible }),
    )
    .await;
    // unconvertible 要回给调用方：改写了 3 条、丢下 2 条，界面得说得出后半句
    Ok(Json(json!({
        "id": done.attribute_id, "batch": done.batch_id,
        "remapped": done.remapped, "unconvertible": done.unconvertible
    })))
}

/// 一次属性采纳要的全部输入。
pub(crate) struct AttributeAdoption<'a> {
    pub key: &'a str,
    pub label: &'a str,
    pub description: &'a str,
    pub datatype: &'a str,
    pub unit: Option<&'a str>,
    pub forms: &'a [String],
    /// true = key 指的是已有属性，只改写、不新建
    pub existing: bool,
}

pub(crate) struct AttributeAdopted {
    pub attribute_id: Uuid,
    pub batch_id: Uuid,
    pub remapped: u32,
    /// 值换不动那个 datatype、因而**没有**被改写的条数。
    /// 必须往上传：改写了 3 条、丢下 2 条，只报前半句就是报喜不报忧
    pub unconvertible: usize,
}

/// 建（或指向已有的）属性，并把等着它的字面值事实改挂过去。人工与自动共用。
pub(crate) async fn adopt_attribute_core(
    state: &AppState,
    kb_id: Uuid,
    spec: &AttributeAdoption<'_>,
) -> Result<AttributeAdopted, AppError> {
    // 先把待改写的事实取出来：它既定 domain，也定值换不换得动
    let facts = utopia_store::graph::value_facts_for_forms(&state.pool, kb_id, spec.forms).await?;
    let attribute_id = if spec.existing {
        utopia_store::ontology::relation_type_id_by_key(&state.pool, kb_id, spec.key)
            .await?
            .ok_or_else(|| {
                AppError::invalid("unknown_relation_key", "no relation type with that key")
            })?
    } else {
        // **domain 从数据里取。** 属性必须声明能挂在哪些类下，猜错的代价是硬的：
        // 主语类型对不上就整条丢弃。这些事实的主语现在是什么类是事实，不是判断
        let mut domains: Vec<Uuid> = facts.iter().map(|(_, type_id, _)| *type_id).collect();
        domains.sort_unstable();
        domains.dedup();
        if domains.is_empty() {
            return Err(AppError::invalid(
                "no_facts_for_forms",
                "nothing is waiting on those wordings",
            ));
        }
        utopia_store::ontology::create_relation_type(
            &state.pool,
            kb_id,
            spec.key,
            spec.label,
            "state",
            // 建议方不替时态引擎做决定：functional 会驱动它自动闭合旧值
            false,
            false,
            spec.description,
            "attribute",
            &domains,
            &[],
            Some(spec.datatype),
            spec.unit,
        )
        .await?
    };

    // 换算按**库里那一条**的 datatype，不按请求——指向已有属性时请求里根本
    // 没有 datatype，而即便有，也该听本体的
    let datatype = utopia_store::ontology::relation_type_datatype(&state.pool, attribute_id)
        .await?
        .unwrap_or_else(|| "text".to_string());
    let mut rewrites: Vec<(Uuid, serde_json::Value)> = Vec::new();
    let mut unconvertible = 0usize;
    for (fact_id, _, object_value) in &facts {
        // 抽取写进去的形状是 {"value": …}，取里面那一层来换算
        let raw = object_value.get("value").unwrap_or(object_value);
        match utopia_extract::normalize_attr_value(&datatype, raw) {
            Some(v) => rewrites.push((*fact_id, json!({ "value": v }))),
            // 换不动的**不改写**：宁可让它继续没有谓词，等下一次，
            // 也不把一个换不动的值硬塞进类型化的属性里
            None => unconvertible += 1,
        }
    }
    let (batch_id, remapped) =
        utopia_store::graph::adopt_value_facts(&state.pool, kb_id, attribute_id, &rewrites).await?;
    for form in spec.forms {
        let _ =
            utopia_store::ontology::clear_miss(&state.pool, kb_id, "attribute_type", form).await;
    }
    Ok(AttributeAdopted {
        attribute_id,
        batch_id,
        remapped,
        unconvertible,
    })
}

/// 映射到**已有属性**：不建东西，只把这些说法的字面值事实挂过去。
pub(crate) async fn adopt_attribute_existing(
    state: &AppState,
    kb_id: Uuid,
    key: &str,
    forms: &[String],
) -> Result<(Uuid, u32), AppError> {
    let done = adopt_attribute_core(
        state,
        kb_id,
        &AttributeAdoption {
            key,
            label: "",
            description: "",
            datatype: "text",
            unit: None,
            forms,
            existing: true,
        },
    )
    .await?;
    Ok((done.batch_id, done.remapped))
}

/// 自动扩本体那条路的入口。参数摊开而不是传 `AdoptReq`——那个结构是 HTTP
/// 请求体，自动路径没有请求。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn adopt_attribute_auto(
    state: &AppState,
    kb_id: Uuid,
    key: &str,
    label: &str,
    description: &str,
    datatype: &str,
    unit: Option<&str>,
    forms: &[String],
) -> Result<(Uuid, u32), AppError> {
    let done = adopt_attribute_core(
        state,
        kb_id,
        &AttributeAdoption {
            key,
            label,
            description,
            datatype,
            unit,
            forms,
            existing: false,
        },
    )
    .await?;
    if done.unconvertible > 0 {
        tracing::info!(
            %kb_id, key, dropped = done.unconvertible,
            "有值换不动这个 datatype，那些事实继续没有谓词"
        );
    }
    Ok((done.batch_id, done.remapped))
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

#[cfg(test)]
mod tests {
    use super::{normalize_name, resolve_map_targets};
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn names_align_across_spellings() {
        // 导进来的本体里 label 常常只是 key 的驼峰写法，两者必须归到一起
        assert_eq!(
            normalize_name("acquiredFrom"),
            normalize_name("acquired_from")
        );
        assert_eq!(
            normalize_name("Acquired From"),
            normalize_name("acquired_from")
        );
        // 不同的名字仍然不同——这一步只对齐写法，不做同义判断
        assert_ne!(normalize_name("acquired_from"), normalize_name("acquires"));
        // 中文标签原样保留（去不掉也不该去）
        assert_eq!(normalize_name("员工数"), "员工数");
    }

    #[test]
    fn a_map_target_outside_the_candidates_is_dropped() {
        let by_name: HashMap<String, String> = [
            ("acquiredfrom".to_string(), "acquired_from".to_string()),
            ("员工数".to_string(), "headcount".to_string()),
        ]
        .into_iter()
        .collect();
        let mut p = json!({"map_to": [
            // 抄的是标签而不是 key —— 实测里模型就是这么干的，要能对回去
            {"key": "acquiredFrom", "forms": ["acquired from"]},
            // 按中文标签抄的，同样对得回去
            {"key": "员工数", "forms": ["员工总数"]},
            // 候选表里没有：界面上那个按钮会承诺一件做不到的事，所以整条丢掉
            {"key": "invented_key", "forms": ["whatever"]},
            // 连 key 都没有
            {"forms": ["x"]},
        ]});
        resolve_map_targets(&mut p, &by_name, &HashMap::new());
        let items = p["map_to"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["key"], "acquired_from");
        assert_eq!(items[1]["key"], "headcount");
    }

    #[test]
    fn proposals_without_a_map_to_section_are_left_alone() {
        let mut p = json!({"entity_types": [], "relation_types": []});
        resolve_map_targets(&mut p, &HashMap::new(), &HashMap::new());
        assert!(p.get("map_to").is_none());
    }

    #[test]
    fn a_wording_that_carries_a_value_cannot_also_become_a_relation() {
        // 实测过的形状：模型把同一个 founded_in 既提成关系又提成属性。
        // 两条都采纳的话同一批事实被抢两次，谁先跑谁赢
        let value_only: HashSet<&str> = ["founded_in", "registered_capital"].into_iter().collect();
        let entity_only: HashSet<&str> = ["acquires"].into_iter().collect();
        let mut p = json!({
            "relation_types": [
                {"key": "founded", "forms": ["founded_in", "founding date"]},
                {"key": "acquires", "forms": ["acquires"]}
            ],
            "attribute_types": [
                {"key": "founding_year", "forms": ["founded_in"]},
                {"key": "registered_capital", "forms": ["registered_capital"]}
            ]
        });
        super::keep_forms(&mut p, "relation_types", &entity_only, &value_only);
        super::keep_forms(&mut p, "attribute_types", &value_only, &entity_only);

        let rels = p["relation_types"].as_array().unwrap();
        // founded 只剩 "founding date"——那个说法两边都不认识，不该替模型否决
        assert_eq!(rels.len(), 2);
        assert_eq!(rels[0]["forms"].as_array().unwrap().len(), 1);
        assert_eq!(rels[0]["forms"][0], "founding date");
        assert_eq!(rels[1]["key"], "acquires");
        // 属性那边一条不动
        assert_eq!(p["attribute_types"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_proposal_left_with_no_wordings_is_dropped() {
        // forms 被剔光 = 它承诺的"改写 N 条"已经是零，留着只会让人点一下什么也没发生
        let value_only: HashSet<&str> = ["founded_in"].into_iter().collect();
        let mut p = json!({"relation_types": [{"key": "founded", "forms": ["founded_in"]}]});
        super::keep_forms(&mut p, "relation_types", &HashSet::new(), &value_only);
        assert!(p["relation_types"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_type_proposal_without_wordings_is_left_alone() {
        // 没有 forms 的提案只是"加一个类型"，不改写任何事实，与归档无关
        let mut p = json!({"entity_types": [{"key": "platform", "label": "Platform"}]});
        super::keep_forms(&mut p, "entity_types", &HashSet::new(), &HashSet::new());
        assert_eq!(p["entity_types"].as_array().unwrap().len(), 1);
    }
}
