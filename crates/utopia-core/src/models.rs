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
    pub predicate: String,
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
    pub parent_id: Option<Uuid>,
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
    pub parent_id: Option<Uuid>,
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
    pub builtin: bool,
    pub description: String,
    /// relation（宾语是实体）| attribute（宾语是字面值）
    pub kind: String,
    /// attribute 专用：挂在哪个类下
    pub domain_type_id: Option<Uuid>,
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
    /// attribute 专用：属性挂在哪个类下
    pub domain_type_id: Option<Uuid>,
    /// attribute 专用：text | number | date | bool
    pub datatype: Option<String>,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Entity {
    pub id: Uuid,
    pub kb_id: Uuid,
    pub type_id: Uuid,
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
    pub type_key: String,
    pub type_label: String,
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
    pub predicate: String,
    pub label: String,
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
    pub predicate_key: String,
    pub predicate_label: String,
    pub temporal: String,
    pub other_id: Option<Uuid>,
    pub other_name: Option<String>,
    /// 字面值宾语（属性事实/问数映射）：{"value":…,"unit":…} 或 {"summary":…}
    pub object_value: Option<serde_json::Value>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_precision: String,
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
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EntityHistoryEvent {
    pub fact_id: Uuid,
    /// 事件发生的记录时刻（写入 = recorded_at，作废 = invalidated_at）
    pub at: DateTime<Utc>,
    /// asserted（首次断言）| corrected（区间被修正）| rejected（认知被推翻）
    pub kind: String,
    pub direction: String,
    pub predicate_label: String,
    pub other_name: Option<String>,
    pub object_value: Option<serde_json::Value>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_precision: String,
    pub confidence: f32,
    /// 人工操作者；NULL = 引擎自动（抽取写入 / 时态对账闭合）
    pub actor_name: Option<String>,
    /// 触发本次变更的审计动作（fact.close / conflict.close_old / fact.reject …）
    pub action: Option<String>,
    pub document_id: Option<Uuid>,
    pub filename: Option<String>,
    pub quote: Option<String>,
}

/// 消解审核项的一侧实体摘要。
#[derive(Debug, Clone, Serialize)]
pub struct ReviewSide {
    pub id: Uuid,
    pub name: String,
    pub type_label: String,
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
    pub predicate_label: String,
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
