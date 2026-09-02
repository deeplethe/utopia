use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Organization {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub org_id: Uuid,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub display_name: String,
    /// 系统管理员（部署的首个注册用户）
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
}

/// 工作区成员视图（成员管理页用）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MemberView {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_admin: bool,
}

/// 部署内用户列表（添加成员的选人器用）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OrgUser {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Workspace {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// 成员角色，按权限从高到低排序。数据库中存小写文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Editor,
    Admin,
    Owner,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Editor => "editor",
            Role::Viewer => "viewer",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "owner" => Some(Role::Owner),
            "admin" => Some(Role::Admin),
            "editor" => Some(Role::Editor),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Document {
    pub id: Uuid,
    pub kb_id: Uuid,
    pub source_id: Option<Uuid>,
    pub filename: String,
    pub mime: String,
    pub size_bytes: i64,
    pub sha256: String,
    /// pending → parsing → indexing → embedding → ready | failed
    pub status: String,
    pub error: Option<String>,
    pub doc_time: Option<DateTime<Utc>>,
    pub doc_time_source: String,
    /// 图谱抽取状态：none → queued → extracting → done | failed
    pub graph_status: String,
    /// 抽取失败原因（失败时才有）。与 error 分列——那列归解析管道，
    /// set_status 会清空它，两者共用一列会互相抹掉。
    pub graph_error: Option<String>,
    pub text_len: i32,
    pub chunk_count: i32,
    pub tags: Vec<String>,
    /// 来源内的逻辑身份（相对路径 / url / rss guid / api external_id）；上传为 NULL
    pub external_key: Option<String>,
    /// watch_folder 同步时发现源文件已消失（默认保留文档，仅标记）
    pub missing_since: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 摄入来源（"来源即文件夹"：容器 + 定时同步）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Source {
    pub id: Uuid,
    pub kb_id: Uuid,
    /// upload | watch_folder | url | rss | api
    pub kind: String,
    pub name: String,
    /// kind 专属配置：watch_folder {path} / url {urls:[..]} / rss {feed_url}
    pub config: serde_json::Value,
    /// lucide 图标名（NULL 时前端按 kind 取默认）
    pub icon: Option<String>,
    /// NULL = 仅手动同步（与 sync_cron 互斥）
    pub sync_interval_minutes: Option<i32>,
    /// 标准 5 段 cron（服务器本地时区；与 sync_interval_minutes 互斥）
    pub sync_cron: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    /// never | queued | running | ok | failed
    pub last_sync_status: String,
    pub last_sync_error: Option<String>,
    pub last_sync_added: i32,
    /// api 来源的推送密钥（明文；查看走 Editor 权限的专用端点，列表响应不带）
    #[serde(skip_serializing)]
    pub ingest_token: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 来源同步运行记录（渠道审计历史）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SyncRun {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// running | ok | failed
    pub status: String,
    pub created_docs: i32,
    pub updated_docs: i32,
    pub error: Option<String>,
}

/// 分块的抽取产物视图（文档查看器右栏：这个 chunk 抽出了什么）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChunkFactView {
    pub chunk_id: Uuid,
    pub fact_id: Uuid,
    pub subject_id: Uuid,
    pub subject: String,
    /// 本体没认下这条关系时回落到原文说法；两者都拿不出时为 None（更早的历史数据长这样）
    pub predicate: Option<String>,
    pub inferred: bool,
    pub object_id: Option<Uuid>,
    pub object: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub confidence: f32,
}

/// 来源列表视图（带文档数）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SourceView {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
    pub icon: Option<String>,
    pub sync_interval_minutes: Option<i32>,
    pub sync_cron: Option<String>,
    pub last_sync_at: Option<DateTime<Utc>>,
    pub last_sync_status: String,
    pub last_sync_error: Option<String>,
    pub last_sync_added: i32,
    pub doc_count: i64,
    /// 已标记"不在来源中"的文档数（url 全集对账 / custom 墓碑产生）
    pub missing_count: i64,
}

/// 审计事件视图（带操作人显示名；删号后为 NULL）。纯审计展示用。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AuditEventView {
    pub id: Uuid,
    pub action: String,
    pub target_kind: String,
    pub target_id: Option<Uuid>,
    pub detail: serde_json::Value,
    pub actor_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 账户层"我的知识库"行信息（成员行可空：open 库凭部署身份进入，无矩阵记录）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MyKbInfo {
    pub kb_id: Uuid,
    pub member_role: Option<String>,
    pub joined_at: Option<DateTime<Utc>>,
    pub added_by_name: Option<String>,
    pub doc_count: i64,
    pub member_count: i64,
}

/// Chat 会话行（左栏列表）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationView {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: i64,
}

/// Chat 消息（含落库的行动轨迹与引用，历史回放用）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationMessage {
    pub id: Uuid,
    pub role: String,
    pub content: String,
    pub steps: serde_json::Value,
    pub sources: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

/// 检索结果用的分块视图（带文档信息）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChunkView {
    pub id: Uuid,
    pub document_id: Uuid,
    pub seq: i32,
    pub text: String,
    pub filename: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LlmSettings {
    pub workspace_id: Uuid,
    pub chat_base_url: Option<String>,
    #[serde(skip_serializing)]
    pub chat_api_key: Option<String>,
    pub chat_model: Option<String>,
    pub embed_base_url: Option<String>,
    #[serde(skip_serializing)]
    pub embed_api_key: Option<String>,
    pub embed_model: Option<String>,
    pub embed_dim: Option<i32>,
    pub updated_at: DateTime<Utc>,
}

impl LlmSettings {
    pub fn chat_ready(&self) -> bool {
        self.chat_base_url.is_some() && self.chat_model.is_some()
    }
    pub fn embed_ready(&self) -> bool {
        self.embed_base_url.is_some() && self.embed_model.is_some()
    }
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntityType {
    pub id: Uuid,
    pub kb_id: Uuid,
    pub key: String,
    pub label: String,
    pub color: String,
    /// 图谱节点形状：circle | square
    pub shape: String,
    pub builtin: bool,
    /// subClassOf 层级（公理推理 P4 点亮，编辑器先维护数据）
    /// 全部父类（subClassOf 可以有多个：FOAF 的 Person 同时是 Agent 与 SpatialThing）
    pub parents: Vec<Uuid>,
    /// 左栏画树时挂在哪一支下。不参与语义，只管展示
    pub primary_parent: Option<Uuid>,
    /// OWL 导入的全局身份。手工建的类为 NULL；重导入按它匹配，不按 key——
    /// 上游改一次 rdfs:label 派生的 key 就变了，按 key 匹配会把同一个类当新类建
    pub iri: Option<String>,
    /// 语义指引：注入抽取 prompt（什么算这个类，举例）
    pub description: String,
}

/// 本体编辑器：某个类下的实体实例行（详情区实例列表用）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntityInstance {
    pub id: Uuid,
    pub name: String,
    pub fact_count: i64,
}

/// 本体编辑器视图：类型 + 使用量（删除保护与 UX 提示用）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntityTypeView {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub color: String,
    pub shape: String,
    pub builtin: bool,
    /// 全部父类（subClassOf 可以有多个：FOAF 的 Person 同时是 Agent 与 SpatialThing）
    pub parents: Vec<Uuid>,
    /// 左栏画树时挂在哪一支下。不参与语义，只管展示
    pub primary_parent: Option<Uuid>,
    /// 与这个类互斥的类：**声明「不可能同时是」**。一致性检查据此报出
    /// 不可满足的类——一个类继承了两个互斥的祖先，就永远不可能有实例（0002）
    pub disjoint: Vec<Uuid>,
    pub description: String,
    pub usage: i64,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RelationTypeView {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub temporal: String,
    pub functional: bool,
    pub inverse_functional: bool,
    /// 其余四条 OWL 公理。**推理机的判据全在这里**（0002）——它们从前只能
    /// 靠导入 OWL 带进来，在界面上建本体的人永远开不了那台机器
    pub is_transitive: bool,
    pub is_symmetric: bool,
    pub is_asymmetric: bool,
    pub is_irreflexive: bool,
    /// 指向另一个关系的两条。**必须回给界面**——下拉框要显示当前选的是谁，
    /// 否则每次打开表单都是空的，编辑一次就把已声明的抹掉了
    pub inverse_of: Option<Uuid>,
    pub sub_property_of: Option<Uuid>,
    pub builtin: bool,
    pub description: String,
    /// relation（宾语是实体）| attribute（宾语是字面值）
    pub kind: String,
    /// 可以当主语的类。attribute 至少一个；relation 可空（未声明 = 不限）
    pub domains: Vec<Uuid>,
    /// 可以当宾语的类。**只对 relation 有意义**——attribute 的值域是字面量类型，
    /// 落在 datatype 上
    pub ranges: Vec<Uuid>,
    /// attribute 专用：text | number | date | bool
    pub datatype: Option<String>,
    pub unit: Option<String>,
    pub usage: i64,
}

/// 抽取未匹配统计（本体扩展建议的信号源）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OntologyMiss {
    pub kind: String,
    pub key: String,
    pub example: Option<String>,
    pub count: i32,
}

/// 一个待认领的表层谓词：原文这么说过，但本体里没有对应关系，事实降级成了
/// related_to。与 `OntologyMiss` 的纯计数不同，它连着具体事实——所以采纳时
/// 能说清"将重新归类 57 条"，并真的去改。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProposedPredicate {
    pub form: String,
    /// 有多少条 live 的 related_to 事实由这个说法而来
    pub fact_count: i64,
    /// 出现在多少篇文档里。只在一篇里出现过的是那篇文档的用词，不是这个
    /// 组织的词汇——自动扩展据此设门槛，人工提案只作参考不拦
    pub doc_count: i64,
    /// 一条样例（"Dino Crisis (Steam) → GeForce NOW"），让人一眼判断这是什么关系
    pub example: Option<String>,
}

/// 一次 OWL 导入的记录。原文按内容寻址存在 blob 里，这行只是账。
/// `summary` 记下那次投影做了什么，包括**暂未投影**的公理——将来补上消费者
/// 时据此知道哪些导入值得重跑。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OntologyImportView {
    pub id: Uuid,
    pub filename: String,
    pub format: String,
    pub byte_size: i64,
    pub summary: serde_json::Value,
    pub imported_at: DateTime<Utc>,
    pub imported_by_name: Option<String>,
}

/// 一个模型的并发上限。约束来自供应商的速率限制，那是按 (base_url, model) 算的——
/// 本地 Ollama 与托管 API 用同一个数字本来就不对。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ModelLimit {
    pub base_url: String,
    pub model: String,
    pub max_concurrent: i32,
}

/// 一个待认领的实体类型：模型提议过、本体没有、实体因此降级成了 concept。
/// 与 `ProposedPredicate` 对称——它连着具体实体，所以采纳时能说清"将重新归类
/// 43 个"并真的去改，而不是只建一个空类。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProposedType {
    pub form: String,
    pub entity_count: i64,
    /// 一个样例名字，让人一眼判断这是什么类
    pub example: Option<String>,
}

/// 抽取丢弃信号：事实抽出来了却没能落地，以及为什么。
/// 与 `OntologyMiss` 分开——那个说"你的本体缺这些"（读者是本体维护者），
/// 这个说"这些事实没落地"（读者是上传文档的人）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ExtractionDrop {
    pub document_id: Uuid,
    pub reason: String,
    pub detail: String,
    pub count: i32,
    pub example: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RelationType {
    pub id: Uuid,
    pub kb_id: Uuid,
    pub key: String,
    pub label: String,
    /// state | event | eternal
    pub temporal: String,
    /// 主语侧唯一：同一时刻一个主语至多一个宾语
    pub functional: bool,
    /// 宾语侧唯一：同一时刻一个宾语至多一个主语（如一个项目只有一个 leads 它的人）
    pub inverse_functional: bool,
    pub builtin: bool,
    /// 语义指引：注入抽取 prompt
    pub description: String,
    /// OWL 导入的全局身份；手工建的为 NULL。重导入按它匹配，不按 key——
    /// 上游改一次 rdfs:label 派生的 key 就变了，按 key 匹配会把同一个当成新的
    pub iri: Option<String>,
    /// relation（宾语是实体）| attribute（宾语是字面值，走 facts.object_value）
    pub kind: String,
    /// 可以当主语的类（多值：OWL 里一个属性有多个 rdfs:domain 是常态）
    pub domains: Vec<Uuid>,
    /// 可以当宾语的类。只对 relation 有意义
    pub ranges: Vec<Uuid>,
    /// attribute 专用：text | number | date | bool
    pub datatype: Option<String>,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Entity {
    pub id: Uuid,
    pub kb_id: Uuid,
    /// `None` = 还没判出来。见 `docs/decisions/0009`——它不是一个类，
    /// 是「抽取器抽到了东西，但本体里没有对应的类」这个状态
    pub type_id: Option<Uuid>,
    pub canonical_name: String,
    pub aliases: Vec<String>,
    pub merged_into: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 图渲染节点。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GraphNode {
    pub id: Uuid,
    pub name: String,
    /// 类型 key。**可能没有**（0009：没判出来就是 NULL），
    /// 前端据此显示"未分类"而不是编一个名字
    pub type_key: Option<String>,
    pub type_label: Option<String>,
    pub color: String,
    /// 类型形状：circle | square
    pub shape: String,
    pub degree: i64,
    /// 同名并存时的展示消歧后缀（如所属组织名）
    pub disambiguator: Option<String>,
}

/// 图渲染边（= 一条 live 事实）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GraphEdge {
    pub id: Uuid,
    pub source: Uuid,
    pub target: Uuid,
    /// 本体里没有对应关系时回落到原文说法（见 `facts.predicate_id`）。
    /// 两个来源都拿不出时为 None——那是 add_evidence 记录原文说法之前的老数据
    pub predicate: Option<String>,
    pub label: Option<String>,
    /// true = 这条边的名字来自原文，不是本体认下的关系。界面要显示得看得出区别
    pub inferred: bool,
    /// true = 这条边是**推出来的**，不是任何人断言的（R1，住在 `derived_facts`）。
    ///
    /// **与 `inferred` 不是一回事**，尽管两个词很近：那一位说的是「名字来自原文
    /// 而不是本体」，这一位说的是「这条边根本不是谁说的，是引擎推的」
    pub derived: bool,
    /// 推它出来的那条规则（`transitive` / `symmetric` / `inverse` / `sub_property`）；
    /// 断言的边为 None。
    ///
    /// **界面需要分辨 `inverse`**：`A works_at B` 与它推出的 `B employs A` 是
    /// 同一件事的两种说法，画成两条边只是把冗余画了两遍；而 `sub_property`
    /// 推出的是另一条粒度不同的事实，该各画各的
    pub rule: Option<String>,
    /// 推它出来用到的前提事实（按证明顺序）。断言的边为空。
    ///
    /// **界面并边要靠它认准来源。** 只按「同一对节点」找，会把 `contains`
    /// 挂到恰好也连着那两点的 `allied_with` 上——那条说法属于 `part_of`，
    /// 挂错的结果看着完全正常，正是最难发现的那种
    pub premises: Vec<Uuid>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub confidence: f32,
}

/// 实体详情页的事实行（时间线）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntityFact {
    pub id: Uuid,
    /// out = 该实体为主语；in = 为宾语
    pub direction: String,
    /// 本体没认下这条关系时回落到原文说法；两者都拿不出时为 None（更早的历史数据长这样）
    pub predicate_key: Option<String>,
    pub predicate_label: Option<String>,
    /// true = 这条事实的名字来自原文，不是本体认下的关系。界面要显示得看得出区别
    pub inferred: bool,
    /// 关系的时态类别（point/state/eternal）。没有谓词就无从谈起，为 None
    pub temporal: Option<String>,
    pub other_id: Option<Uuid>,
    pub other_name: Option<String>,
    /// 字面值宾语（属性事实/问数映射）：{"value":…,"unit":…} 或 {"summary":…}
    pub object_value: Option<serde_json::Value>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// 精度描述的是这条事实**有的那些日期**的粒度。两端都没有日期时为 None——
    /// 从前这里是 NOT NULL DEFAULT day，于是没日期的事实也自称精确到日（见 `facts.valid_from_precision`）
    /// 起始端的粒度：year | month | day。没有 valid_from 时为 None
    pub valid_from_precision: Option<String>,
    /// 结束端的粒度，外加一个 `unknown`——**原文说它结束了，但没说哪天**。
    /// `valid_to` 与它都为 None 才是「仍在持续」（见 `facts.valid_to_precision`）
    pub valid_to_precision: Option<String>,
    pub confidence: f32,
    pub evidence_count: i64,
    /// 证据全部停留在来源文档的旧版（未被现行内容确认；不代表事实失效）
    pub stale: bool,
    /// 修正行（supersedes 链上）：区间闭合来自引擎对账/人工裁决而非抽取原文
    pub corrected: bool,
    /// 证据集合里最新的文档时间——开放事实的"最后确认时间"（时效性透明化）
    pub last_evidence_time: Option<DateTime<Utc>>,
}

/// 实体的一次认知变更（记录时间轴上的事件，与 EntityFact 的有效时间轴正交）。
///
/// 账本 append-only，所以"我们曾经怎么认为"全部留存：一条事实行最多产出两个
/// 事件——写入（asserted / corrected）与作废（rejected，仅当没有后继修正行时；
/// 有后继的话这次死亡已由那条 corrected 解释，不重复记）。
///
/// **不是每个事件都来自一条事实。** 改类（`retyped` / `retype_reverted`）来自
/// `entity_retypes`：它没有谓词、没有对方、没有方向，那几个字段因此可空。
/// 从前这里只有事实事件，于是「改了类」在实体历史里完全不显形——0001 P3a 记着
/// 「可撤销不等于会被撤销」，错了不会自己冒出来。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntityHistoryEvent {
    /// 事实事件才有。改类事件为 None
    pub fact_id: Option<Uuid>,
    /// 事件发生的记录时刻（写入 = recorded_at，作废 = invalidated_at，
    /// 改类 = entity_retypes.created_at / reverted_at）
    pub at: DateTime<Utc>,
    /// asserted（首次断言）| corrected（区间被修正）| rejected（认知被推翻）
    /// | merged（并进了另一条断言）| retyped（改了类）| retype_reverted（改类被撤销）
    pub kind: String,
    pub direction: Option<String>,
    pub predicate_label: Option<String>,
    pub other_name: Option<String>,
    pub object_value: Option<serde_json::Value>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// 精度描述的是这条事实**有的那些日期**的粒度。两端都没有日期时为 None——
    /// 从前这里是 NOT NULL DEFAULT day，于是没日期的事实也自称精确到日（见 `facts.valid_from_precision`）
    /// 起始端的粒度：year | month | day。没有 valid_from 时为 None
    pub valid_from_precision: Option<String>,
    /// 结束端的粒度，外加一个 `unknown`——**原文说它结束了，但没说哪天**。
    /// `valid_to` 与它都为 None 才是「仍在持续」（见 `facts.valid_to_precision`）
    pub valid_to_precision: Option<String>,
    pub confidence: Option<f32>,
    /// 人工操作者；NULL = 引擎自动（抽取写入 / 时态对账闭合 / 高置信改类）
    pub actor_name: Option<String>,
    /// 触发本次变更的审计动作（fact.close / conflict.close_old / fact.reject …）
    pub action: Option<String>,
    pub document_id: Option<Uuid>,
    pub filename: Option<String>,
    pub quote: Option<String>,
    /// 改类事件的两端。起点可空——0009 之后「从没有类到有类」是最常见的一次改类
    pub from_type_label: Option<String>,
    pub to_type_label: Option<String>,
}

/// 一段**记录时间**窗口里，整个库上发生的认知变更。
///
/// 跟 `EntityHistoryEvent` 是同一批事件，两处不同：
/// 1. 开窗在**认知轴**上（recorded_at / invalidated_at），不锁定单个实体——
///    "上季度有什么变了"这种问题没有一个先验的实体可问；
/// 2. 主宾都写全（`direction` 是"以某实体为中心"才有的概念，这里没有中心）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct GraphChange {
    pub fact_id: Uuid,
    /// 事件落在认知轴上的时刻（写入 = recorded_at，作废 = invalidated_at）
    pub at: DateTime<Utc>,
    /// asserted（新断言）| corrected（订正了前一条）| rejected（被推翻）| merged（并入他条）
    pub kind: String,
    pub subject_id: Uuid,
    pub subject_name: String,
    pub predicate_label: Option<String>,
    pub object_name: Option<String>,
    pub object_value: Option<serde_json::Value>,
    /// 这条断言说的是**世界轴**上的哪一段——与 `at` 正交，别读混
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    /// 精度描述的是这条事实**有的那些日期**的粒度。两端都没有日期时为 None——
    /// 从前这里是 NOT NULL DEFAULT day，于是没日期的事实也自称精确到日（见 `facts.valid_from_precision`）
    /// 起始端的粒度：year | month | day。没有 valid_from 时为 None
    pub valid_from_precision: Option<String>,
    /// 结束端的粒度，外加一个 `unknown`——**原文说它结束了，但没说哪天**。
    /// `valid_to` 与它都为 None 才是「仍在持续」（见 `facts.valid_to_precision`）
    pub valid_to_precision: Option<String>,
    pub confidence: f32,
    pub document_id: Option<Uuid>,
    pub filename: Option<String>,
    pub quote: Option<String>,
}

/// 消解审核项的一侧实体摘要。
#[derive(Debug, Clone, Serialize)]
pub struct ReviewSide {
    pub id: Uuid,
    pub name: String,
    /// 没判出类型时为 None（0009）。颜色另有缺省值——它是画布必须拿到的
    pub type_label: Option<String>,
    pub color: String,
    pub disambiguator: Option<String>,
    pub degree: i64,
    pub top_facts: Vec<String>,
}

/// 消解审核项：疑似同一实体的灰区对。
#[derive(Debug, Clone, Serialize)]
pub struct ReviewItem {
    pub id: Uuid,
    pub score: f32,
    pub reason: Option<String>,
    /// adjudicating = 等 LLM 裁决；human = 等人工终审
    pub stage: String,
    pub created_at: DateTime<Utc>,
    pub left: ReviewSide,
    pub right: ReviewSide,
}

/// 合并日志行（审核页历史区）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MergeLogView {
    pub id: Uuid,
    pub source_name: String,
    pub target_name: String,
    /// NULL = LLM 自动合并
    pub merged_by_name: Option<String>,
    pub reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reverted_at: Option<DateTime<Utc>>,
}

/// 低置信事实审核行。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FactReviewItem {
    pub id: Uuid,
    pub subject_name: String,
    pub predicate_label: Option<String>,
    pub object_name: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub evidence_count: i64,
    pub quote: Option<String>,
}

/// 事实的证据（引句 + 原文定位）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EvidenceView {
    /// 模型在这一块里实际用的谓词说法。词表外谓词被降级成 related_to 后，
    /// 事实行上只剩"有关联"——原意只在这里
    pub proposed_predicate: Option<String>,
    pub quote: Option<String>,
    pub chunk_id: Uuid,
    pub document_id: Uuid,
    pub filename: String,
    pub seq: i32,
    /// 证据出自文档的第几版
    pub doc_version: i32,
    /// 文档已有更新的版本（证据停留在旧版；不代表事实失效）
    pub stale: bool,
}

/// 时态冲突（S3 自动闭合拿不准的那些）：旧事实 vs 新事实，Review 页人裁。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConflictView {
    pub id: Uuid,
    /// no_time | simultaneous | low_confidence
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub predicate_label: String,
    /// 双方完整三元组：主语侧冲突变的是宾语，宾语侧冲突变的是主语
    pub old_fact_id: Uuid,
    pub old_subject: String,
    pub old_object: Option<String>,
    pub old_valid_from: Option<DateTime<Utc>>,
    pub new_fact_id: Uuid,
    pub new_subject: String,
    pub new_object: Option<String>,
    pub new_valid_from: Option<DateTime<Utc>>,
    pub new_confidence: f32,
}

/// 文档查看器用的分块视图。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ChunkFull {
    pub id: Uuid,
    pub seq: i32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KnowledgeBase {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    /// `knowledge` | `memory`（Agent 记忆空间）
    pub kind: String,
    pub description: Option<String>,
    /// open = 全员按部署角色；restricted = 仅 kb_members 名单可见
    pub visibility: String,
    /// 部署的公共默认空间（第一个建的库）：永远 open、不可删除
    pub is_default: bool,
    /// 抽取遇到本体外的说法时，是否允许系统自动把它补进本体并改写等它的事实。
    /// 缺省开——新库的十个默认关系不是任何人选的，等人手工补齐之前图基本没法用。
    /// 关掉不影响"留意"：未匹配统计照常累积、照常可见，只是变成你点一下的提案。
    pub auto_extend_ontology: bool,
    /// 内置本体按哪种语言播种，以及新的类/关系描述写成哪种语言（`en` | `zh`）。
    /// **跟语料走，不跟界面走**——description 的读者是正在读这些文档的模型。
    /// 见 docs/decisions/0004。
    /// 是否把推出来的事实写进账本（R1）。**缺省关**——这一步往图里加东西，
    /// 而 0001 判据 2 说「本体是引导不是执法」：声明可能是错的，不该在用户
    /// 没表态时就按它改图
    pub materialize_inferences: bool,
    /// 多久重推一次（分钟）。见 `knowledge_bases.inference_interval_minutes`
    pub inference_interval_minutes: i32,
    /// 上次推完的时间。**答的是「上次看过没有」，不是「上次改过没有」**
    pub last_inference_at: Option<DateTime<Utc>>,
    pub ontology_lang: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// KB 成员矩阵行（库 Settings 的 Members 区）。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct KbMemberView {
    pub user_id: Uuid,
    pub email: String,
    pub display_name: String,
    /// viewer | editor | admin
    pub role: String,
}

/// 问数数据源列表视图：连接串不下发（凭据只进不出），只露 host:port/db 摘要。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DataSourceView {
    pub id: Uuid,
    pub name: String,
    pub engine: String,
    /// 连接摘要（host:port/db，无凭据）
    pub summary: String,
    pub created_at: DateTime<Utc>,
    pub last_test_at: Option<DateTime<Utc>>,
    pub last_test_ok: Option<bool>,
}

/// 向量检索出来的一个候选本体行（类 / 关系 / 属性）。
///
/// `distance` 是余弦距离，越小越近。原样带给调用方而不是先折成"相似度"：
/// 阈值该定在哪由消费者按自己的数据定，这里不替它归一化。
#[derive(Debug, Clone, Serialize)]
pub struct TypeCandidate {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub description: String,
    /// 关系行才有：`relation` 或 `attribute`
    pub kind: Option<String>,
    pub distance: f32,
}

/// 一个被记下来、但本体里没有对应属性的**字面值**说法。
///
/// 跟 [`ProposedPredicate`] 是一对：那个是宾语指向实体的（"收购"），
/// 这个是宾语是字面值的（"成立日期 = 2015"）。两者不能混——提案要产出的东西
/// 不一样（关系 vs 属性），而混起来的后果具体：一条 `founding_date` 会变成
/// 一条指向「2015」这个假实体的边。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ProposedAttribute {
    pub form: String,
    pub fact_count: i64,
    pub doc_count: i64,
    /// 一条样例值（`"2015"`、`1200`），让人一眼看出这是什么类型的数
    pub example: Option<String>,
    /// 这个说法**实际挂在哪些类上**（主语的类型）。
    ///
    /// 属性必须声明 domain，而 domain 猜错的代价是硬的：主语类型对不上
    /// 就整条丢弃（`attr_domain_mismatch`）。所以不问模型，直接从数据里取——
    /// 事实已经在那儿了，它们的主语是什么类是事实，不是判断
    pub domain_keys: Vec<String>,
}

/// 一条口径改动之前的样子。
///
/// **存整版快照而不是差异**（0006）：读的时候要回答的是「当时是什么」，
/// 而差异得从头重放才答得出来。`before` 是改动前那一行的 `to_jsonb`，
/// 去掉了 id 与 kb_id。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MappingRevision {
    pub id: Uuid,
    pub before: serde_json::Value,
    /// 改的人。**裸外键 + 用户软删除**，所以归因不会因为人离职而丢；
    /// 真被硬删过才会是 NULL
    pub changed_by_name: Option<String>,
    pub changed_at: DateTime<Utc>,
}

/// 语义层的一条映射：业务概念 → 数据资产定义（见 `docs/decisions/0011`）。
///
/// **字段是列，不是 JSON 里的键。** 从前它是一条 `mapped_to` 事实，
/// 这几样全塞在 `object_value` 里——于是「哪些概念映射到了 orders 这张表」
/// 要扒 JSON，而「同一个概念同一个源只该有一条」这条约束数据库管不到。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConceptMapping {
    pub id: Uuid,
    pub concept_id: Uuid,
    /// 概念的名字。读的一侧总要它（问数 prompt、Review 列表），
    /// 每次再查一遍实体表是白跑
    pub concept_name: String,
    /// 挂载的数据源。同一个概念在不同源上可以有不同定义，这是有意支持的
    pub source: String,
    pub table_name: Option<String>,
    pub expr: Option<String>,
    pub sql: Option<String>,
    pub unit: Option<String>,
    pub summary: Option<String>,
    /// 派生指标（「转化率 = 成交数 / 访问数」）：算出来的，不是表里的列
    pub derived: bool,
    /// proposed | confirmed | rejected
    ///
    /// **状态而不是置信度。** 从前借事实的 confidence 表达「提议 0.6 / 确认 1.0」，
    /// 那是把二值状态编码成浮点数，还顺带让它落进「低置信事实」那一档
    pub status: String,
}

/// 一处公理违规，配好展示所需的三元组文本（见 `axiom_violations`）。
///
/// **两条事实都展开成 主-谓-宾 文本**：Review 页要让人一眼看出矛盾在哪，
/// 而两个 UUID 看不出任何东西。自反那一类两条相同——它就是一条事实。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AxiomViolation {
    pub id: Uuid,
    /// self_loop | asymmetry | cycle | functional
    pub kind: String,
    /// 判据来自哪条关系。人若判「公理写错了」，从这里进本体去改
    pub predicate: Option<String>,
    pub left_fact: Uuid,
    pub left_text: String,
    pub right_fact: Uuid,
    pub right_text: String,
    /// 环的长度（含首尾）。其余三类为 0——前端据此决定要不要显示「查看路径」
    pub path_len: i32,
    pub detected_at: chrono::DateTime<chrono::Utc>,
}

/// 本体自己的一处自相矛盾（见 `ontology_defects`）。
///
/// **与 [`AxiomViolation`] 不是一回事**：那个说「事实与定义抵触」，这个说
/// 「定义自己站不住」。后者更根本——一个自相矛盾的本体会让前者的结论全部可疑。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct OntologyDefect {
    pub id: Uuid,
    /// symmetric_and_asymmetric | transitive_and_functional | subclass_cycle
    /// | disjoint_with_ancestor | inherits_disjoint
    pub kind: String,
    /// 出问题那个对象的标签（类或谓词）。查不到就是它已经被删了
    pub subject_label: Option<String>,
    /// 另一方：互斥的那个类
    pub other_label: Option<String>,
    /// 环上类的标签，按顺序
    pub path_labels: Vec<String>,
    pub detected_at: chrono::DateTime<chrono::Utc>,
}

/// 一条推出来的事实，连同它的证明（实体面板的「推出来的」那一档）。
///
/// **`premises` 是这一档存在的理由**：不给出前提的话，一条派生边跟一条普通的边
/// 在界面上看不出区别，而那正是「推理污染知识」的样子。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DerivedFactView {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub subject: String,
    pub object_id: Uuid,
    pub object: String,
    pub predicate: String,
    /// transitive | symmetric——靠哪条规则推的
    pub rule: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub derived_at: DateTime<Utc>,
    /// 直接前提，按推导顺序展开成三元组文本
    pub premises: Vec<String>,
}

/// 审核队列各档的**真实条数**。
///
/// 与列表分开取是有意的：列表有上限（一页十条），数数没有。从前左栏读的是
/// 数组长度，而接口固定只回 100 条——一个有 164 条待办的库，界面写着 100，
/// 清完还会再冒出来。
/// 等人点头的一条事实（0015）。`quote` 是那句记忆的全文——确认界面要把原句和
/// 三元组并排显示，只列三元组等于要人凭空判断它对不对。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PendingFactView {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub subject_name: String,
    pub predicate_id: Option<Uuid>,
    /// 本体里的关系名；为空时前端显示 `proposed_predicate`（斜体，标明是原话）
    pub predicate_label: Option<String>,
    pub proposed_predicate: Option<String>,
    pub object_id: Option<Uuid>,
    pub object_name: Option<String>,
    pub object_value: Option<serde_json::Value>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_from_precision: Option<String>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_to_precision: Option<String>,
    pub confidence: f32,
    pub chunk_id: Uuid,
    pub quote: String,
    pub proposed_by: Option<Uuid>,
    pub proposed_by_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, sqlx::FromRow)]
pub struct ReviewCounts {
    /// 记忆抽出、等人点头的事实（0015）。排第一：它是人自己说的话
    pub pending: i64,
    pub duplicates: i64,
    pub conflicts: i64,
    pub unconfirmed: i64,
    pub lowconf: i64,
    pub mappings: i64,
    pub violations: i64,
    pub defects: i64,
    pub merges: i64,
}

/// 一个关系声明了哪些 OWL 公理。
///
/// **打包成一个东西传，不是一串参数。** 它们本来就是同一族——推理机
/// （0002）拿它们当判据，界面上也该并排出现；散成参数表里的六个 bool，
/// 调用点迟早传错顺序，而 `bool` 之间编译器帮不上忙。
///
/// 后两位不是 bool：`inverseOf` 与 `subPropertyOf` 指向**另一个关系**，
/// 界面上是下拉框而不是复选框。形状不同不改变它们属于这一族——推理机
/// 的四种规则源正是这六位里的两条加上这两条（0002）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationAxioms {
    /// 主语侧唯一（一个人一个出生地）
    pub functional: bool,
    /// 宾语侧唯一（一个项目一个 leader）
    pub inverse_functional: bool,
    /// A→B ∧ B→C ⟹ A→C
    pub transitive: bool,
    /// A→B ⟹ B→A
    pub symmetric: bool,
    /// A→B ⟹ 不存在 B→A
    pub asymmetric: bool,
    /// 不存在 A→A
    pub irreflexive: bool,
    /// `p⁻¹ = q`：`A p B ⟹ B q A`。**单向存，双向用**——载入公理时归一化
    /// （`reasoning::axioms`），所以只需在一侧声明，反向那条自动成立
    pub inverse_of: Option<Uuid>,
    /// `p ⊑ q`：`A p B ⟹ A q B`。断言了具体的，通用的也成立
    pub sub_property_of: Option<Uuid>,
}

/// 文库的一页，连同这一页之外的统计。
///
/// **统计不受名字/状态筛选影响**：`ready` / `extracting` / `failed` 说的是这个
/// 来源里有多少，那是批量按钮的作用范围，跟你此刻在搜什么无关。
#[derive(Debug, Clone, Serialize)]
pub struct DocumentPage {
    pub docs: Vec<Document>,
    /// 命中筛选的总数（分页器用它）
    pub total: i64,
    pub ready: i64,
    pub extracting: i64,
    pub failed: i64,
}

/// 一枚个人访问令牌的元信息（0014）。**永远不含明文**——
/// 明文只在 `tokens::issue` 返回的那一次存在，库里只有哈希。
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct TokenView {
    pub id: Uuid,
    pub name: String,
    /// 给人认的那一小截（`utp_pat_ab12`）。够对上配置文件里那一串，
    /// 又不足以复原
    pub token_prefix: String,
    /// read | write。**上限不是授权**：有效权限 = 这个人的角色 ∩ 这个 scope
    pub scope: String,
    /// None = 这个人能进的全部库
    pub kb_ids: Option<Vec<Uuid>>,
    pub expires_at: Option<DateTime<Utc>>,
    /// 「这把还在用吗」。撤之前要答得出，否则没人敢撤
    pub last_used_at: Option<DateTime<Utc>>,
    /// **撤销打戳不删行**：撤过这件事本身要留痕
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
