import { S, lang } from "./i18n";

export class ApiError extends Error {
  status: number;
  /** 服务端给的稳定错误码（没有则是尚未转换的契约守卫，message 已是英文原句） */
  code?: string;
  constructor(status: number, message: string, code?: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "include",
    headers:
      init?.body instanceof FormData
        ? {}
        : { "Content-Type": "application/json" },
    ...init,
  });
  if (!res.ok) {
    // 措辞在这一个收口点定：22 个文件里的 toast.error(e.message) 一处都不用改。
    // 有 code 就查 i18n，没有（或这一条还没进表）就退回服务端的英文原句——
    // 服务端永远说英文，因为界面语言在客户端（docs/decisions/0004）
    let message = res.statusText;
    let code: string | undefined;
    try {
      const body = (await res.json()) as {
        error?: string;
        code?: string;
        detail?: string;
      };
      if (body.error) message = body.error;
      code = body.code;
      // code 来自网络，不是字面量——这一处 cast 换来 err 表本身的全量类型检查
      const worded = code
        ? (S.err as Record<string, string | undefined>)[code]
        : undefined;
      if (worded) message = worded;
      if (body.detail) message = S.errDetail(message, body.detail);
    } catch {
      // 非 JSON 响应体，保留 statusText
    }
    throw new ApiError(res.status, message, code);
  }
  return res.json() as Promise<T>;
}

export interface User {
  id: string;
  org_id: string;
  email: string;
  display_name: string;
  is_admin: boolean;
  created_at: string;
}

export interface Workspace {
  id: string;
  org_id: string;
  name: string;
  created_at: string;
}

/** 建库时可选的预置本体包（`GET /ontology-packs`）。 */
export type OntologyPack = {
  id: string;
  name: string;
  summary: string;
  classes: number;
  properties: number;
};

export interface Kb {
  id: string;
  workspace_id: string;
  name: string;
  kind: string;
  description: string | null;
  visibility: "open" | "restricted";
  /** 部署的公共默认空间（第一个建的库）：永远 open、不可删除 */
  is_default: boolean;
  /** 抽取遇到本体外的说法时，是否允许系统自动补进本体并改写等它的事实。
      关掉不影响"留意"：未匹配统计照常累积可见，只是变成你点一下的提案 */
  auto_extend_ontology: boolean;
  /** 把推出来的事实写进账本（R1）。**缺省关**——推理往图里加东西，
   *  而声明可能是错的，不该在用户没表态时就按它改图 */
  materialize_inferences: boolean;
  /** 多久重推一次（分钟）。事实持续在变，只靠手点会让派生一直是缺的 */
  inference_interval_minutes: number;
  /** 上次推完的时间 */
  last_inference_at: string | null;
  /** 内置本体按哪种语言播种、新描述写哪种语言。**跟语料走，不跟界面走**
      （界面语言在客户端，见 docs/decisions/0004） */
  ontology_lang: "en" | "zh";
  /** 调用者在本库的角色（仅详情接口返回）：前端据此门控破坏性入口 */
  my_role?: "viewer" | "editor" | "admin" | "owner" | null;
}

/** 账户层"我的知识库"行：库 + 我的角色 + 加入信息 + 概览统计。 */
export interface MyKb {
  kb: Kb;
  my_role: "viewer" | "editor" | "admin" | "owner" | null;
  joined_at: string | null;
  added_by_name: string | null;
  doc_count: number;
  member_count: number;
}

/** 审计事件（纯审计展示）。 */
export interface AuditEvent {
  id: string;
  action: string;
  target_kind: string;
  target_id: string | null;
  detail: Record<string, unknown>;
  actor_name: string | null;
  created_at: string;
}

export interface KbMember {
  user_id: string;
  email: string;
  display_name: string;
  role: "viewer" | "editor" | "admin";
}

export interface Doc {
  id: string;
  kb_id: string;
  source_id: string | null;
  filename: string;
  mime: string;
  size_bytes: number;
  status: string;
  graph_status: string;
  /** 摄入管道的失败原因 */
  error: string | null;
  /** 图谱抽取管道的失败原因（与 error 分列：两条管道各存各的） */
  graph_error: string | null;
  chunk_count: number;
  /** 文档标签。**今天没有任何界面用它**——故意留着，理由写在
   *  `migrations/0002_ingest.sql` 的 `tags` 列上 */
  tags: string[];
  missing_since: string | null;
  created_at: string;
}

/** 一类抽取丢弃在一篇文档里的聚合：事实抽出来了，却没能落地。 */
export interface ExtractionDrop {
  document_id: string;
  /** 稳定的原因码，前端按它查文案（attr_domain_mismatch / low_confidence / ...） */
  reason: string;
  /** 该原因下的具体对象（属性 key、谓词名、"salary@organization"） */
  detail: string;
  count: number;
  example: string | null;
}

export interface SourceView {
  id: string;
  kind: "folder" | "url" | "rss" | "api" | "custom" | "memory" | "upload";
  name: string;
  config: { urls?: string[]; feed_url?: string; endpoint?: string } | null;
  icon: string | null;
  sync_interval_minutes: number | null;
  sync_cron: string | null;
  last_sync_at: string | null;
  last_sync_status: "never" | "queued" | "running" | "ok" | "failed";
  last_sync_error: string | null;
  last_sync_added: number;
  doc_count: number;
  missing_count: number;
}

export interface SearchResult {
  id: string;
  document_id: string;
  seq: number;
  text: string;
  filename: string;
}

export interface LlmSettingsView {
  chat_base_url?: string | null;
  chat_model?: string | null;
  has_chat_key?: boolean;
  embed_base_url?: string | null;
  embed_model?: string | null;
  embed_dim?: number | null;
  has_embed_key?: boolean;
}

export interface Member {
  user_id: string;
  email: string;
  display_name: string;
  role: string;
  is_admin: boolean;
}

export interface OrgUser {
  id: string;
  email: string;
  display_name: string;
  is_admin: boolean;
}

/** 问数数据源（凭据不下发,只有 host:port/db 摘要）。 */
export interface DataSourceView {
  id: string;
  name: string;
  engine: string;
  summary: string;
  created_at: string;
  last_test_at: string | null;
  last_test_ok: boolean | null;
}

export interface GraphNode {
  id: string;
  name: string;
  // 没判出类型时为 null（0009）
  type_key: string | null;
  type_label: string | null;
  color: string;
  shape: "circle" | "square";
  degree: number;
  disambiguator: string | null;
}

export interface SyncRun {
  id: string;
  started_at: string;
  finished_at: string | null;
  status: "running" | "ok" | "failed";
  created_docs: number;
  updated_docs: number;
  error: string | null;
}

/** 分块的抽取产物（文档查看器右栏）。 */
export interface ChunkFact {
  chunk_id: string;
  fact_id: string;
  subject_id: string;
  subject: string;
  /** 本体没认下这条关系时是原文说法；两者都拿不出时为 null */
  predicate: string | null;
  /** true = 名字来自原文，不是本体认下的关系 */
  inferred: boolean;
  object_id: string | null;
  object: string | null;
  valid_from: string | null;
  valid_to: string | null;
  confidence: number;
}

/** 一条推出来的事实，连同它的证明（实体面板的「推出来的」那一档）。
 *
 * `premises` 是这一档存在的理由：不给出前提的话，一条派生边跟一条普通的边
 * 在界面上看不出区别，而那正是「推理污染知识」的样子。 */
export interface DerivedFact {
  id: string;
  subject_id: string;
  subject: string;
  object_id: string;
  object: string;
  predicate: string;
  /** 靠哪条规则推的 */
  rule: "transitive" | "symmetric";
  valid_from: string | null;
  valid_to: string | null;
  confidence: number;
  derived_at: string;
  /** 直接前提，按推导顺序 */
  premises: string[];
}

/** 审核页的分档。**与服务端的 queue 参数是同一组字面量**——拼错会拿到
 *  一个明确的 unknown_queue 错误，而不是悄悄的空列表。 */
export type ReviewQueue =
  | "duplicates"
  | "conflicts"
  | "unconfirmed"
  | "lowconf"
  | "mappings"
  | "violations"
  | "defects"
  | "merges";

/** 各档的真实条数。左栏的徽标读它，不读列表长度 */
export interface ReviewCounts {
  duplicates: number;
  conflicts: number;
  unconfirmed: number;
  lowconf: number;
  mappings: number;
  violations: number;
  defects: number;
  merges: number;
}

/** 类型消解的一条建议：一个待精化的实体、送去检索的画像、以及候选类。
 *
 * `profile` 回给调用方是有意的——检索找不着的时候，第一个要看的就是
 * 「我们拿什么去找的」，而不是猜是画像不对还是类的描述不对。 */
export interface TypeSuggestion {
  entity_id: string;
  name: string;
  /** 现在挂着的类，可能没有（0009） */
  coarse: string | null;
  coarse_description: string | null;
  proposed_type: string | null;
  specific_type: string | null;
  fact_count: number;
  profile: string;
  candidates: {
    id: string;
    key: string;
    label: string;
    description: string;
    distance: number;
  }[];
}

/** 跑完一轮的结果。**三档分开报**：自动改了的、留给人的、裁决说「都不是」的。
 *  最后那一档带着理由——这一步押在「选择都不是是个体面答案」上，
 *  不记理由，最大的那一档就是不透明的。 */
export interface ResolutionOutcome {
  batch: string | null;
  retyped: number;
  for_review: {
    entity_id: string;
    name: string;
    coarse: string | null;
    from_type_id: string | null;
    to_type_id: string;
    choice: string;
    confidence: number;
    reason: string | null;
    /** 选中的类不在粗类的子树里——换的是分类轴，不是往下走一格 */
    crosses_axis: boolean;
  }[];
  left_alone: {
    name: string;
    coarse: string | null;
    specific_type: string | null;
    reason: string | null;
    top_candidate: string | null;
  }[];
}
export interface ReviewSide {
  id: string;
  name: string;
  // 没判出类型时为 null（0009）
  type_label: string | null;
  color: string;
  disambiguator: string | null;
  degree: number;
  top_facts: string[];
}

export interface ReviewItem {
  id: string;
  score: number;
  reason: string | null;
  stage: "adjudicating" | "human";
  created_at: string;
  left: ReviewSide;
  right: ReviewSide;
}


/** 语义层的一条映射：业务概念 → 数据资产定义（见 docs/decisions/0011）。
 *
 * 字段是列而不是 JSON 里的键——从前它是一条 `mapped_to` 事实，
 * 这几样全塞在 `object_value` 里。 */
export interface ConceptMapping {
  id: string;
  concept_id: string;
  concept_name: string;
  /** 挂载的数据源。同一概念在不同源上可以有不同定义 */
  source: string;
  table_name: string | null;
  expr: string | null;
  sql: string | null;
  unit: string | null;
  summary: string | null;
  /** 派生指标：算出来的，不是表里的列 */
  derived: boolean;
  status: "proposed" | "confirmed" | "rejected";
}
/** 一处公理违规（0002 R0）。判据来自本体自己声明的公理，没声明就不报 */
export interface AxiomViolation {
  id: string;
  kind: "self_loop" | "asymmetry" | "cycle" | "functional";
  /** 判据来自哪条关系。判「公理写错了」时从这里进本体去改 */
  predicate: string | null;
  left_fact: string;
  left_text: string;
  /** 自反那一类与 left 相同——一条事实跟自己矛盾 */
  right_fact: string;
  right_text: string;
  /** 环的长度；其余三类为 0 */
  path_len: number;
  detected_at: string;
}
/** 本体自己的一处自相矛盾。**与 AxiomViolation 不是一回事**：那个说
 *  「事实与定义抵触」，这个说「定义自己站不住」，后者更根本 */
export interface OntologyDefect {
  id: string;
  kind:
    | "symmetric_and_asymmetric"
    | "transitive_and_functional"
    | "subclass_cycle"
    | "disjoint_with_ancestor"
    | "inherits_disjoint";
  subject_label: string | null;
  other_label: string | null;
  path_labels: string[];
  detected_at: string;
}
export interface FactReviewItem {
  id: string;
  subject_name: string;
  predicate_label: string | null;
  object_name: string | null;
  valid_from: string | null;
  valid_to: string | null;
  confidence: number;
  evidence_count: number;
  quote: string | null;
}

/** 时态冲突（自动闭合拿不准的那些）：旧事实 vs 新事实。 */
export interface ConflictItem {
  id: string;
  reason: "no_time" | "simultaneous" | "low_confidence";
  created_at: string;
  predicate_label: string;
  old_fact_id: string;
  old_subject: string;
  old_object: string | null;
  old_valid_from: string | null;
  new_fact_id: string;
  new_subject: string;
  new_object: string | null;
  new_valid_from: string | null;
  new_confidence: number;
}

/** 决策台账事件（audit_events 的 review 域切片）：detail 是决策时的自包含快照。 */
export interface ReviewHistoryEvent {
  id: string;
  action: string;
  target_kind: string;
  target_id: string | null;
  detail: Record<string, unknown>;
  /** null = 系统（AI 裁决器） */
  actor_name: string | null;
  created_at: string;
}

export interface MergeLog {
  id: string;
  source_name: string;
  target_name: string;
  merged_by_name: string | null;
  reason: string | null;
  created_at: string;
  reverted_at: string | null;
}

export interface GraphEdge {
  id: string;
  source: string;
  target: string;
  /** 本体没认下这条关系时是原文说法；两者都拿不出时为 null（0052 之前的老数据） */
  predicate: string | null;
  label: string | null;
  /** true = 这条边的名字来自原文，不是本体认下的关系 */
  inferred: boolean;
  /** true = 这条边是**推出来的**，不是任何人断言的（R1）。
   *  与 `inferred` 不是一回事：那个说「名字来自原文」，这个说「不是谁说的」 */
  derived: boolean;
  valid_from: string | null;
  valid_to: string | null;
  confidence: number;
}

export interface EntityFact {
  id: string;
  direction: "out" | "in";
  /** 同 GraphEdge：本体外的关系回落到原文说法，两者都没有时为 null */
  predicate_key: string | null;
  predicate_label: string | null;
  /** true = 名字来自原文，不是本体认下的关系 */
  inferred: boolean;
  /** 关系的时态类别。没有谓词就无从谈起，为 null */
  temporal: string | null;
  other_id: string | null;
  other_name: string | null;
  /** 字面值宾语（属性事实/问数映射）：{"value":…} 或 {"summary":…} */
  object_value: Record<string, unknown> | null;
  valid_from: string | null;
  valid_to: string | null;
  valid_from_precision: string | null;
  /** year | month | day，外加 unknown = 原文说它结束了但没说哪天 */
  valid_to_precision: string | null;
  confidence: number;
  evidence_count: number;
  /** 证据全部停留在来源文档的旧版（未被现行内容确认；不代表事实失效） */
  stale: boolean;
  /** 修正行：区间闭合来自引擎对账/人工裁决而非抽取原文 */
  corrected: boolean;
  /** 证据集合里最新的文档时间（开放事实的"最后确认时间"） */
  last_evidence_time: string | null;
}

/** 实体的一次认知变更（记录时间轴上的事件，与 EntityFact 的有效时间轴正交）。
 *
 * 不是每个事件都来自一条事实：retyped / retype_reverted 来自改类账本，
 * 没有谓词、没有对方、没有方向。 */
export interface EntityHistoryEvent {
  /** 事实事件才有；改类事件为 null */
  fact_id: string | null;
  /** 记录时刻：写入 = recorded_at，作废 = invalidated_at，改类 = 改类那一刻 */
  at: string;
  kind:
    | "asserted"
    | "corrected"
    | "rejected"
    | "merged"
    | "retyped"
    | "retype_reverted";
  direction: "out" | "in" | null;
  predicate_label: string | null;
  other_name: string | null;
  object_value: Record<string, unknown> | null;
  valid_from: string | null;
  valid_to: string | null;
  valid_from_precision: string | null;
  /** year | month | day，外加 unknown = 原文说它结束了但没说哪天 */
  valid_to_precision: string | null;
  confidence: number | null;
  /** null = 引擎自动（抽取写入 / 时态对账闭合 / 高置信自动改类） */
  actor_name: string | null;
  action: string | null;
  document_id: string | null;
  filename: string | null;
  quote: string | null;
  /** 改类事件的两端。起点为 null = 从「未分类」改过来（0009 之后最常见的一种） */
  from_type_label: string | null;
  to_type_label: string | null;
}

export interface Evidence {
  /** 模型在这一块里实际用的谓词说法。本体外的谓词不落到关系上，界面显示的就是这里 */
  proposed_predicate: string | null;
  quote: string | null;
  chunk_id: string;
  document_id: string;
  filename: string;
  seq: number;
  /** 证据出自文档的第几版 */
  doc_version: number;
  /** 文档已有更新版本（证据停留在旧版；不代表事实失效） */
  stale: boolean;
}

export interface ChunkFull {
  id: string;
  seq: number;
  text: string;
}

export interface EntityTypeView {
  id: string;
  key: string;
  label: string;
  color: string;
  shape: "circle" | "square";
  builtin: boolean;
  /** 全部父类（subClassOf 可以有多个） */
  parents: string[];
  /** 与它互斥的类：**声明「不可能同时是」** */
  disjoint: string[];
  /** 左栏画树时挂在哪一支下。不参与语义，只管展示 */
  primary_parent: string | null;
  description: string;
  usage: number;
}

export interface RelationTypeView {
  id: string;
  key: string;
  label: string;
  temporal: string;
  functional: boolean;
  inverse_functional: boolean;
  /** 其余四条 OWL 公理。**推理机的判据全在这里** */
  is_transitive: boolean;
  is_symmetric: boolean;
  is_asymmetric: boolean;
  is_irreflexive: boolean;
  builtin: boolean;
  description: string;
  /** relation（宾语是实体）| attribute（宾语是字面值） */
  kind: "relation" | "attribute";
  /** 可以当主语的类。attribute 至少一个；relation 留空 = 不限 */
  domains: string[];
  /** 可以当宾语的类。只对 relation 有意义——attribute 的值域是 datatype */
  ranges: string[];
  datatype: "text" | "number" | "date" | "bool" | null;
  unit: string | null;
  usage: number;
}

export interface OntologyMiss {
  kind: "entity_type" | "relation_type";
  key: string;
  example: string | null;
  count: number;
}

/** `description` 与 `reason` 不是一回事：description 逐字进抽取提示词，是模型判断
    "什么算这个类"的唯一依据；reason 只是给人看的"为什么该加"。喂错了这个类会成为
    下一个倾倒场——实测 technology 就是这么来的。 */
export interface OntologyProposals {
  entity_types: {
    key: string;
    label: string;
    description?: string;
    reason?: string;
  }[];
  relation_types: {
    key: string;
    label: string;
    temporal?: string;
    functional?: boolean;
    description?: string;
    reason?: string;
    /** 这条关系归并了哪些表层说法。有它才谈得上把等待的事实改写过去 */
    forms?: string[];
  }[];
  /**
   * 宾语是字面值的说法（"成立日期 = 2015"）。
   *
   * 跟 relation_types 分开是因为它们要的东西不一样：属性有 datatype，
   * 而把它当关系建出来，那个值就会变成一个假实体。domain 不在这里——
   * 服务端从事实的主语类型里取，猜错会让整条被丢弃
   */
  attribute_types?: {
    key: string;
    label: string;
    datatype?: string;
    unit?: string;
    description?: string;
    reason?: string;
    forms?: string[];
  }[];
  /**
   * 本体里**已经有**这个意思，只需把说法挂过去。
   *
   * 跟 relation_types 的区别是不建东西：同一个意思长出第二个 key，
   * 这批事实就永久分在两处，谁也认不出它们本是一回事。
   */
  map_to?: {
    key: string;
    /** 服务端标的：目标落在关系还是属性上。两条改写路径不一样，
        而模型只答得出一个 key，看不出它在哪一档 */
    kind?: string;
    forms?: string[];
    reason?: string;
  }[];
}

/** 原文说过、本体里没有、因而事实没有谓词的说法。 */
export interface ProposedPredicate {
  form: string;
  fact_count: number;
  example: string | null;
}

/** 一个类/属性在这次导入里的去向。key_taken = key 被另一个 IRI 占着，报告但不动 */
export interface PlannedItem {
  iri: string;
  key: string;
  label: string;
  has_description: boolean;
  disposition: "create" | "update" | "key_taken";
  functional?: boolean;
  conflict_with?: string | null;
}

/** 预览与落库返回同一个计划：点确认之后发生的事就是刚看过的事 */
export interface ImportPlan {
  format: string;
  triples: number;
  classes: PlannedItem[];
  relations: PlannedItem[];
  attributes: PlannedItem[];
  /** 出现过但今天不消费的公理 → 次数。不是已跳过，是暂未投影 */
  unprojected: [string, number][];
  classes_without_description: number;
  functional_relations: number;
}

export interface OntologyImportView {
  id: string;
  filename: string;
  format: string;
  byte_size: number;
  summary: Record<string, unknown>;
  imported_by_name: string | null;
  imported_at: string;
}

export interface Source {
  n: number;
  /** 缺省 = 文档 chunk 引用；charter = 内置手册（跳 /docs/{slug}#{anchor}） */
  kind?: "charter";
  chunk_id?: string;
  document_id?: string;
  slug?: string;
  anchor?: string;
  heading?: string;
  filename: string;
  excerpt: string;
}

/** Agentic 对话的行动轨迹（工具调用一步一条）。 */
export interface ChatStep {
  kind: "search" | "docs" | "entity" | "facts" | "changes" | "query" | "tool";
  label: string;
  detail: string;
}

/** 会话行（Chat 左栏列表）。 */
export interface ConversationRow {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  message_count: number;
}

/** 会话消息（含落库的行动轨迹与引用，历史回放用）。 */
export interface ConversationMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  steps: ChatStep[];
  sources: Source[];
  created_at: string;
}

/** 告警中心的一组（0005）：**连着的、同类的几次故障**。
 *
 * 存储那边仍是一次故障一行，折叠在服务端读的时候做——这样翻页数的是组，
 * 一段连续故障不会被页边界切断。 */
export type AlertGroup = {
  kb_id: string | null;
  /** 系统级告警没有库名 */
  kb_name: string | null;
  /** `source.sync_failed` / `llm.unreachable` —— 措辞在 i18n 里按这个查 */
  kind: string;
  severity: "info" | "warning" | "error";
  /** 这一组几次 */
  count: number;
  /** 其中我没读过的几次 */
  unread: number;
  latest_at: string;
  /** 跟 latest_at 一起圈出这一组，标已读时原样发回去 */
  earliest_at: string;
  /** 明细，最多几条，新的在前 */
  lines: { name?: string; error?: string; job?: string }[];
};

export const api = {
  health: () =>
    request<{ status: string; name: string; version: string }>(
      "/api/v1/health",
    ),
  me: () => request<User>("/api/v1/auth/me"),
  alerts: (o: { q?: string; limit?: number; offset?: number }) => {
    const p = new URLSearchParams();
    if (o.q?.trim()) p.set("q", o.q.trim());
    if (o.limit != null) p.set("limit", String(o.limit));
    if (o.offset) p.set("offset", String(o.offset));
    return request<{ items: AlertGroup[]; total: number }>(
      `/api/v1/alerts?${p}`,
    );
  },
  alertsUnread: () => request<{ unread: number }>("/api/v1/alerts/unread"),
  alertReadGroup: (g: {
    kb_id: string | null;
    kind: string;
    from: string;
    to: string;
  }) =>
    request<{ marked: number }>("/api/v1/alerts/read-group", {
      method: "POST",
      body: JSON.stringify(g),
    }),
  alertsReadAll: () =>
    request<{ ok: boolean }>("/api/v1/alerts/read-all", { method: "POST" }),
  login: (email: string, password: string) =>
    request<{ user: User }>("/api/v1/auth/login", {
      method: "POST",
      body: JSON.stringify({ email, password }),
    }),
  register: (email: string, password: string, displayName: string) =>
    request<{ user: User; workspace: Workspace }>("/api/v1/auth/register", {
      method: "POST",
      body: JSON.stringify({ email, password, display_name: displayName }),
    }),
  logout: () =>
    request<{ ok: boolean }>("/api/v1/auth/logout", { method: "POST" }),
  updateMe: (displayName: string) =>
    request<User>("/api/v1/auth/me", {
      method: "PATCH",
      body: JSON.stringify({ display_name: displayName }),
    }),
  changePassword: (currentPassword: string, newPassword: string) =>
    request<{ ok: boolean }>("/api/v1/auth/password", {
      method: "POST",
      body: JSON.stringify({
        current_password: currentPassword,
        new_password: newPassword,
      }),
    }),
  workspaces: () => request<Workspace[]>("/api/v1/workspaces"),

  kbs: (workspaceId: string) =>
    request<Kb[]>(`/api/v1/workspaces/${workspaceId}/kbs`),
  /** 审计台账。**分页 + 筛选**——台账是合规材料，只看最近 100 条等于查不了历史。
   *  `action` 是前缀：`entity.` 捞出 entity.retyped / entity.renamed 一族。 */
  kbAudit: (
    kbId: string,
    opts: {
      action?: string;
      actor?: string;
      since?: string;
      until?: string;
      limit: number;
      offset: number;
    },
  ) => {
    const p = new URLSearchParams({
      limit: String(opts.limit),
      offset: String(opts.offset),
    });
    if (opts.action) p.set("action", opts.action);
    if (opts.actor) p.set("actor", opts.actor);
    if (opts.since) p.set("since", opts.since);
    if (opts.until) p.set("until", opts.until);
    return request<{
      events: AuditEvent[];
      total: number;
      /** 这个库实际发生过的动作，筛选下拉按它填 */
      actions: string[];
    }>(`/api/v1/kbs/${kbId}/audit?${p}`);
  },
  myKbs: (workspaceId: string) =>
    request<{ kbs: MyKb[] }>(`/api/v1/workspaces/${workspaceId}/my-kbs`),
  createKb: (
    workspaceId: string,
    body: {
      name: string;
      description?: string | null;
      visibility?: string;
      /** 预置本体包 id，顺序有意义：第一个会认领同名的种子类 */
      ontology_packs?: string[];
    },
  ) =>
    request<Kb>(`/api/v1/workspaces/${workspaceId}/kbs`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  ontologyPacks: () =>
    request<{ packs: OntologyPack[] }>("/api/v1/ontology-packs"),

  kbDetail: (kbId: string) => request<Kb>(`/api/v1/kbs/${kbId}`),
  updateKb: (kbId: string, body: Record<string, unknown>) =>
    request<Kb>(`/api/v1/kbs/${kbId}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteKb: (kbId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}`, { method: "DELETE" }),
  kbMembers: (kbId: string) =>
    request<{ members: KbMember[] }>(`/api/v1/kbs/${kbId}/members`),
  setKbMember: (kbId: string, userId: string, role: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/members/${userId}`, {
      method: "PUT",
      body: JSON.stringify({ role }),
    }),
  removeKbMember: (kbId: string, userId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/members/${userId}`, {
      method: "DELETE",
    }),

  adminDeployment: () =>
    request<{
      open_registration: boolean;
      /** 外层兜底，防任务无限堆积；真正的节流是按模型的限额 */
      worker_concurrency: number;
      default_model_concurrency: number;
      /** 新建知识库的本体语言默认值。**不是界面语言**——那个在客户端 */
      default_ontology_lang: "en" | "zh";
      model_limits: {
        base_url: string;
        model: string;
        max_concurrent: number;
      }[];
      models_in_use: { base_url: string; model: string; kind: string }[];
    }>("/api/v1/admin/deployment"),
  saveAdminDeployment: (
    openRegistration: boolean,
    workerConcurrency?: number,
    defaultModelConcurrency?: number,
    modelLimit?: {
      base_url: string;
      model: string;
      max_concurrent: number | null;
    },
    defaultOntologyLang?: "en" | "zh",
  ) =>
    request<{ ok: boolean }>("/api/v1/admin/deployment", {
      method: "PUT",
      body: JSON.stringify({
        open_registration: openRegistration,
        ...(workerConcurrency !== undefined
          ? { worker_concurrency: workerConcurrency }
          : {}),
        ...(defaultModelConcurrency !== undefined
          ? { default_model_concurrency: defaultModelConcurrency }
          : {}),
        ...(modelLimit ? { model_limit: modelLimit } : {}),
        ...(defaultOntologyLang
          ? { default_ontology_lang: defaultOntologyLang }
          : {}),
      }),
    }),
  adminCreateUser: (body: {
    email: string;
    display_name: string;
    password: string;
    role: string;
  }) =>
    request<{ user: User }>("/api/v1/admin/users", {
      method: "POST",
      body: JSON.stringify(body),
    }),

  /** 已停用的账号。**没有它恢复就够不着**——那个人从所有列表里消失，
   *  而恢复接口要的正是他的 id */
  deactivatedUsers: () =>
    request<OrgUser[]>("/api/v1/users/deactivated"),
  /** 恢复一个停用的账号 */
  adminReactivateUser: (userId: string) =>
    request<{ ok: boolean }>(`/api/v1/admin/users/${userId}`, {
      method: "POST",
    }),
  /** 停用一个账号（软删除）。归因照旧查得到——审计、合并日志、改类账本都靠它 */
  adminDeactivateUser: (userId: string) =>
    request<{ ok: boolean }>(`/api/v1/admin/users/${userId}`, {
      method: "DELETE",
    }),
  adminDataSources: () =>
    request<{ data_sources: DataSourceView[] }>("/api/v1/admin/data-sources"),
  adminCreateDataSource: (body: { name: string; conn_string: string }) =>
    request<{ id: string }>("/api/v1/admin/data-sources", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  adminDeleteDataSource: (id: string) =>
    request<{ ok: boolean }>(`/api/v1/admin/data-sources/${id}`, {
      method: "DELETE",
    }),
  adminTestDataSource: (id: string) =>
    request<{ ok: boolean }>(`/api/v1/admin/data-sources/${id}/test`, {
      method: "POST",
    }),
  kbDataSources: (kbId: string) =>
    request<{ data_sources: DataSourceView[] }>(
      `/api/v1/kbs/${kbId}/data-sources`,
    ),
  kbDataSourcesAvailable: (kbId: string) =>
    request<{ data_sources: DataSourceView[] }>(
      `/api/v1/kbs/${kbId}/data-sources/available`,
    ),
  mountDataSource: (kbId: string, dsId: string) =>
    request<{ ok: boolean; schema_tables: number }>(
      `/api/v1/kbs/${kbId}/data-sources/${dsId}`,
      {
        method: "PUT",
      },
    ),
  unmountDataSource: (kbId: string, dsId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/data-sources/${dsId}`, {
      method: "DELETE",
    }),
  exploreMappings: (kbId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/data-sources/explore`, {
      method: "POST",
    }),
  syncDataSourceSchema: (kbId: string, dsId: string) =>
    request<{ ok: boolean; schema_tables: number }>(
      `/api/v1/kbs/${kbId}/data-sources/${dsId}/sync-schema`,
      { method: "POST" },
    ),

  /** 文库一页。**服务端筛选与分页**——从前一次取回整库、前端切片，
   *  而客户端筛选只筛得到已经拿下来的那些 */
  documents: (
    kbId: string,
    opts: {
      source?: string;
      q?: string;
      graph?: string;
      limit: number;
      offset: number;
    },
  ) => {
    const p = new URLSearchParams({
      limit: String(opts.limit),
      offset: String(opts.offset),
    });
    if (opts.source) p.set("source", opts.source);
    if (opts.q) p.set("q", opts.q);
    if (opts.graph) p.set("graph", opts.graph);
    return request<{
      docs: Doc[];
      total: number;
      /** 下面三个**只按来源作用域算**，不受名字/状态筛选影响——
       *  它们是批量按钮的作用范围 */
      ready: number;
      extracting: number;
      failed: number;
    }>(`/api/v1/kbs/${kbId}/documents?${p}`);
  },
  /** 一键重试这个作用域里全部抽取失败的文档 */
  retryFailedDocs: (kbId: string, source?: string) =>
    request<{ queued: number; found: number }>(
      `/api/v1/kbs/${kbId}/documents/retry-failed${source ? `?source=${source}` : ""}`,
      { method: "POST" },
    ),
  /** 整库一次取回：行数按 (文档 × 原因 × 对象) 聚合后很小，避免逐行发请求 */
  extractionDrops: (kbId: string) =>
    request<{ drops: ExtractionDrop[] }>(
      `/api/v1/kbs/${kbId}/extraction-drops`,
    ),
  upload: (kbId: string, files: File[], sourceId?: string) => {
    const form = new FormData();
    for (const f of files) form.append("files", f, f.name);
    const qs = sourceId ? `?source=${sourceId}` : "";
    return request<{ created: Doc[]; skipped: unknown[] }>(
      `/api/v1/kbs/${kbId}/documents${qs}`,
      { method: "POST", body: form },
    );
  },
  deleteDocument: (id: string) =>
    request<{ ok: boolean }>(`/api/v1/documents/${id}`, { method: "DELETE" }),

  search: (kbId: string, q: string) =>
    request<{ results: SearchResult[] }>(`/api/v1/kbs/${kbId}/search`, {
      method: "POST",
      body: JSON.stringify({ q }),
    }),

  settings: (workspaceId: string) =>
    request<LlmSettingsView>(`/api/v1/workspaces/${workspaceId}/settings`),
  saveSettings: (workspaceId: string, body: Record<string, unknown>) =>
    request<{ ok: boolean }>(`/api/v1/workspaces/${workspaceId}/settings`, {
      method: "PUT",
      body: JSON.stringify(body),
    }),
  graphOverview: (kbId: string) =>
    request<{
      nodes: GraphNode[];
      edges: GraphEdge[];
      /** 库里一共有多少。**与 nodes.length 不是一回事**——画布只画度数最高的
       *  那一批，把上限当成规模显示是这个接口从前最误导人的地方 */
      total_nodes?: number;
      total_edges?: number;
    }>(`/api/v1/kbs/${kbId}/graph/overview`),
  /** 邻域视图**没有总数**：它本来就只是一小片，说「共 325 个」没有意义。
   *  两个字段声明成可选，好让调用方与总览共用一个类型 */
  graphNeighborhood: (kbId: string, entityId: string) =>
    request<{
      nodes: GraphNode[];
      edges: GraphEdge[];
      total_nodes?: number;
      total_edges?: number;
    }>(`/api/v1/kbs/${kbId}/graph/neighborhood?entity=${entityId}&hops=2`),
  /** 按名字找实体。**一并回总数**——「宁分勿合」会造出一堆同名，
   *  固定十条时想找的那个可能根本不在这十条里 */
  searchEntities: (kbId: string, q: string, limit = 10) =>
    request<{ entities: GraphNode[]; total: number }>(
      `/api/v1/kbs/${kbId}/entities?q=${encodeURIComponent(q)}&limit=${limit}`,
    ),
  entityDetail: (kbId: string, entityId: string) =>
    request<{
      entity: GraphNode;
      facts: EntityFact[];
      /** 推出来的那些**单独一个键**，不掺进 facts：混在同一个列表里，
       *  用户看不出「文档里写的」和「引擎推的」的区别 */
      derived: DerivedFact[];
      /** 同名的其他实体。**打开面板就给**——合并入口要长在能看见同名的地方，
       *  而不是藏在「改一次名」之后 */
      same_name: GraphNode[];
    }>(`/api/v1/kbs/${kbId}/entities/${entityId}`),
  /** 认知变更历史（记录时间轴）：服务端分页 */
  /** 人工修正实体的类型或名字。同名不拦——返回的 same_name 供界面提示是否合并。 */
  updateEntity: (
    kbId: string,
    entityId: string,
    body: { type_id?: string; canonical_name?: string },
  ) =>
    request<{ entity: GraphNode; same_name: GraphNode[] }>(
      `/api/v1/kbs/${kbId}/entities/${entityId}`,
      { method: "PATCH", body: JSON.stringify(body) },
    ),

  entityHistory: (kbId: string, entityId: string, page: number, per = 30) =>
    request<{ events: EntityHistoryEvent[]; total: number }>(
      `/api/v1/kbs/${kbId}/entities/${entityId}/history?page=${page}&per=${per}`,
    ),
  factEvidence: (kbId: string, factId: string) =>
    request<{ evidence: Evidence[] }>(
      `/api/v1/kbs/${kbId}/facts/${factId}/evidence`,
    ),
  documentDetail: (id: string) =>
    request<{ document: Doc; chunks: ChunkFull[] }>(`/api/v1/documents/${id}`),
  extractDocument: (id: string) =>
    request<{ job_id: number }>(`/api/v1/documents/${id}/extract`, {
      method: "POST",
    }),
  reprocessDocument: (id: string) =>
    request<{ job_id: number }>(`/api/v1/documents/${id}/reprocess`, {
      method: "POST",
    }),
  /** 来源级全量重抽（增量语义：既有决策保留） */
  reExtractSource: (kbId: string, sourceId: string) =>
    request<{ queued: number }>(
      `/api/v1/kbs/${kbId}/sources/${sourceId}/re-extract`,
      {
        method: "POST",
      },
    ),
  /** 图谱重建（清算语义：清空图层后全量重抽；KB admin） */
  rebuildGraph: (kbId: string) =>
    request<{
      entities_removed: number;
      facts_removed: number;
      queued: number;
    }>(`/api/v1/kbs/${kbId}/graph/rebuild`, { method: "POST" }),

  ontology: (kbId: string) =>
    request<{
      entity_types: EntityTypeView[];
      relation_types: RelationTypeView[];
      misses: OntologyMiss[];
      /** 已忽略的，连同它此后继续累积的计数。抑制照旧，只是看得见 */
      dismissed_misses: OntologyMiss[];
    }>(`/api/v1/kbs/${kbId}/ontology`),
  createEntityType: (kbId: string, body: Record<string, unknown>) =>
    request<{ id: string }>(`/api/v1/kbs/${kbId}/ontology/entity-types`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  updateEntityType: (kbId: string, id: string, body: Record<string, unknown>) =>
    request<{ ok: boolean }>(
      `/api/v1/kbs/${kbId}/ontology/entity-types/${id}`,
      {
        method: "PATCH",
        body: JSON.stringify(body),
      },
    ),
  deleteEntityType: (kbId: string, id: string) =>
    request<{ ok: boolean }>(
      `/api/v1/kbs/${kbId}/ontology/entity-types/${id}`,
      {
        method: "DELETE",
      },
    ),
  typeEntities: (kbId: string, typeId: string, page: number, per = 12) =>
    request<{
      entities: { id: string; name: string; fact_count: number }[];
      total: number;
    }>(
      `/api/v1/kbs/${kbId}/ontology/entity-types/${typeId}/entities?page=${page}&per=${per}`,
    ),
  createRelationType: (kbId: string, body: Record<string, unknown>) =>
    request<{ id: string }>(`/api/v1/kbs/${kbId}/ontology/relation-types`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  updateRelationType: (
    kbId: string,
    id: string,
    body: Record<string, unknown>,
  ) =>
    request<{ ok: boolean }>(
      `/api/v1/kbs/${kbId}/ontology/relation-types/${id}`,
      {
        method: "PATCH",
        body: JSON.stringify(body),
      },
    ),
  deleteRelationType: (kbId: string, id: string) =>
    request<{ ok: boolean }>(
      `/api/v1/kbs/${kbId}/ontology/relation-types/${id}`,
      {
        method: "DELETE",
      },
    ),
  /** 上次算出来、还没人表态的提案（0049）。刷新页面靠它，不必重跑模型 */
  storedProposals: (kbId: string) =>
    request<OntologyProposals>(`/api/v1/kbs/${kbId}/ontology/proposals`),
  /** 一条提案有人表态了。改状态不删行——拒绝留痕，下一轮 Suggest 不再刷回待看 */
  decideProposal: (
    kbId: string,
    section: string,
    key: string,
    status: "adopted" | "rejected",
  ) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/proposals`, {
      method: "POST",
      body: JSON.stringify({ section, key, status }),
    }),
  dismissMiss: (kbId: string, kind: string, key: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/misses/dismiss`, {
      method: "POST",
      body: JSON.stringify({ kind, key }),
    }),
  restoreMiss: (kbId: string, kind: string, key: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/misses/restore`, {
      method: "POST",
      body: JSON.stringify({ kind, key }),
    }),
  /** reason 只给人看，而人就在这次请求的另一端——所以语言由调用方说，
      不是后端的设置（docs/decisions/0004）。description 的语言跟知识库走，服务端自己知道 */
  suggestOntology: (kbId: string) =>
    request<OntologyProposals>(`/api/v1/kbs/${kbId}/ontology/suggest`, {
      method: "POST",
      body: JSON.stringify({ locale: lang }),
    }),

  /** 上传本体文件只算出计划，一个字节都不写库 */
  previewOntologyImport: (kbId: string, file: File) => {
    const form = new FormData();
    form.append("file", file, file.name);
    return request<{ filename: string; plan: ImportPlan }>(
      `/api/v1/kbs/${kbId}/ontology/imports/preview`,
      { method: "POST", body: form },
    );
  },
  /** 执行刚看过的那个计划——服务端重新算一遍，两条路径共用同一段代码 */
  applyOntologyImport: (kbId: string, file: File) => {
    const form = new FormData();
    form.append("file", file, file.name);
    return request<{ import_id: string; plan: ImportPlan }>(
      `/api/v1/kbs/${kbId}/ontology/imports`,
      { method: "POST", body: form },
    );
  },
  ontologyImports: (kbId: string) =>
    request<{ imports: OntologyImportView[] }>(
      `/api/v1/kbs/${kbId}/ontology/imports`,
    ),

  proposedPredicates: (kbId: string) =>
    request<{ forms: ProposedPredicate[] }>(
      `/api/v1/kbs/${kbId}/ontology/proposed-predicates`,
    ),
  /** 最近一次自动扩本体做了什么，以及还能不能撤销（撤干净了返回 null） */
  lastAutoExtension: (kbId: string) =>
    request<{
      run: {
        at: string;
        relations: string[] | null;
        classes: string[] | null;
        facts_remapped: number | null;
        batches: string[];
      } | null;
    }>(`/api/v1/kbs/${kbId}/ontology/auto-extension`),
  /** 建关系 **并**把等着它的无谓词事实认过去——后半句才是收益 */
  adoptPredicate: (
    kbId: string,
    body: {
      key: string;
      /** true = key 指的是已有的关系/属性，只改写事实，不建新类型 */
      existing?: boolean;
      /** attribute 走另一条改写路径：值要按 datatype 换算 */
      kind?: "relation" | "attribute";
      datatype?: string;
      unit?: string;
      label?: string;
      temporal?: string;
      functional?: boolean;
      description?: string;
      forms: string[];
    },
  ) =>
    request<{
      id: string;
      remapped: number;
      batch: string;
      /** 值换不动那个 datatype、因而没被改写的条数。改写了 3 条丢下 2 条，
          只报前半句就是报喜不报忧 */
      unconvertible?: number;
    }>(`/api/v1/kbs/${kbId}/ontology/adopt-predicate`,
      { method: "POST", body: JSON.stringify(body) },
    ),
  /** 撤销一次采纳：新写的行作废、旧行复活。关系类型留着 */
  unadoptPredicate: (kbId: string, batchId: string) =>
    request<{ reverted: number }>(
      `/api/v1/kbs/${kbId}/ontology/adopt-predicate/${batchId}`,
      { method: "DELETE" },
    ),

  sources: (kbId: string) =>
    request<{ sources: SourceView[] }>(`/api/v1/kbs/${kbId}/sources`),
  createSource: (kbId: string, body: Record<string, unknown>) =>
    request<{ source: SourceView; ingest_token?: string | null }>(
      `/api/v1/kbs/${kbId}/sources`,
      {
        method: "POST",
        body: JSON.stringify(body),
      },
    ),
  sourceToken: (kbId: string, sourceId: string) =>
    request<{ ingest_token: string | null }>(
      `/api/v1/kbs/${kbId}/sources/${sourceId}/token`,
    ),
  rotateSourceToken: (kbId: string, sourceId: string) =>
    request<{ ingest_token: string }>(
      `/api/v1/kbs/${kbId}/sources/${sourceId}/rotate-token`,
      { method: "POST" },
    ),
  updateSource: (
    kbId: string,
    sourceId: string,
    body: Record<string, unknown>,
  ) =>
    request<{ source: SourceView }>(`/api/v1/kbs/${kbId}/sources/${sourceId}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteSource: (kbId: string, sourceId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/sources/${sourceId}`, {
      method: "DELETE",
    }),
  cleanupMissing: (kbId: string, sourceId: string) =>
    request<{ deleted: number }>(
      `/api/v1/kbs/${kbId}/sources/${sourceId}/missing/cleanup`,
      {
        method: "POST",
      },
    ),
  syncSource: (kbId: string, sourceId: string) =>
    request<{ queued: boolean }>(
      `/api/v1/kbs/${kbId}/sources/${sourceId}/sync`,
      {
        method: "POST",
      },
    ),
  sourceRuns: (kbId: string, sourceId: string) =>
    request<{ runs: SyncRun[] }>(
      `/api/v1/kbs/${kbId}/sources/${sourceId}/runs`,
    ),
  documentExtractions: (docId: string) =>
    request<{ facts: ChunkFact[] }>(`/api/v1/documents/${docId}/extractions`),

  /** 审核队列的各档**真实条数**。与列表分开取——列表有一页的上限，数数没有。
   *  从前徽标读的是数组长度，而接口固定只回 100 条，于是 164 条写成 100。 */
  review: (
    kbId: string,
    queue: ReviewQueue,
    limit: number,
    offset: number,
  ) =>
    request<{
      counts: ReviewCounts;
      queue: ReviewQueue;
      /** 只有当前这一档的一页。类型按档不同，调用处按 queue 收窄 */
      items: unknown[];
    }>(
      `/api/v1/kbs/${kbId}/review?queue=${queue}&limit=${limit}&offset=${offset}`,
    ),
  closeFact: (kbId: string, factId: string, validTo: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/facts/${factId}/close`, {
      method: "POST",
      body: JSON.stringify({ valid_to: validTo }),
    }),
  resolveConflict: (
    kbId: string,
    conflictId: string,
    body: { action: "close" | "keep" | "reject_new"; close_at?: string },
  ) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/conflicts/${conflictId}`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  decideReview: (kbId: string, reviewId: string, action: "merge" | "keep") =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/review/${reviewId}`, {
      method: "POST",
      body: JSON.stringify({ action }),
    }),
  /** 对一条语义层映射表态（0011）。改状态不删行——拒绝留痕，下一轮探索不再提议它 */
  decideMapping: (kbId: string, mappingId: string, status: "confirmed" | "rejected") =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/review/mappings/${mappingId}`, {
      method: "POST",
      body: JSON.stringify({ status }),
    }),
  /** 跑一遍一致性检查。同步返回——纯计算，没有模型调用也没有网络 */
  runConsistencyCheck: (kbId: string) =>
    request<{
      edges: number;
      /** **零和零不一样**：没有公理时结论是「没有判据」，不是「未发现矛盾」 */
      predicates_with_axioms: number;
      found: number;
      inserted: number;
      cleared: number;
      classes: number;
      /** 本体自己的矛盾**单独回**，不加进 found：两个数不是一类东西 */
      defects_found: number;
      defects_new: number;
    }>(`/api/v1/kbs/${kbId}/consistency/check`, { method: "POST" }),
  /** 对一处本体缺陷表态。**两个出路**——它压根没看数据，没有「数据错了」这条 */
  decideDefect: (
    kbId: string,
    defectId: string,
    resolution: "fixed" | "accepted",
  ) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/review/defects/${defectId}`, {
      method: "POST",
      body: JSON.stringify({ resolution }),
    }),
  /** 跑一遍推理（R1）。开关关着时后端回 inference_off */
  /** 类型消解：**只算不写**。回执里带着送去检索的画像——检索找不着时，
   *  第一个要看的就是「我们拿什么去找的」 */
  /** 手动合并：把 source 并进 target。**方向要紧**——source 消失，
   *  它的事实搬到 target 上；合并可整体回滚（entity_merges 记着快照） */
  mergeEntities: (kbId: string, source: string, target: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/entities/merge`, {
      method: "POST",
      body: JSON.stringify({ source, target }),
    }),
  typeResolutionPreview: (kbId: string) =>
    request<{ items: TypeSuggestion[] }>(
      `/api/v1/kbs/${kbId}/ontology/type-resolution/preview`,
      { method: "POST" },
    ),
  /** 跑一遍并落库。三档分开回：自动改的、留给人的、说「都不是」的 */
  typeResolutionApply: (kbId: string) =>
    request<ResolutionOutcome>(`/api/v1/kbs/${kbId}/ontology/type-resolution`, {
      method: "POST",
    }),
  /** 认可一个「粗类 → 细类」的配对，并把带上的实体改过去。
   *  **认可的是类对，改的是实体**——认可一次，之后同一对不再进人工 */
  approveRefinement: (
    kbId: string,
    body: { from_type_id: string; to_type_id: string; entity_ids: string[] },
  ) =>
    request<{ retyped: number }>(
      `/api/v1/kbs/${kbId}/ontology/type-resolution/approve`,
      { method: "POST", body: JSON.stringify(body) },
    ),
  /** 撤销一整批：把那批实体放回原来的类 */
  typeResolutionUndo: (kbId: string, batchId: string) =>
    request<{ reverted: number }>(
      `/api/v1/kbs/${kbId}/ontology/type-resolution/${batchId}`,
      { method: "DELETE" },
    ),
  runInference: (kbId: string) =>
    request<{
      /** 编译出来的规则条数。为零时是「没有规则」而不是「推不出东西」 */
      rules: number;
      edges: number;
      derived: number;
      inserted: number;
      /** 前提没了、跟着作废的 */
      invalidated: number;
      /** 撞上单谓词上限、没推完的谓词个数 */
      capped: number;
    }>(`/api/v1/kbs/${kbId}/inference/run`, { method: "POST" }),
  /** 对一处公理违规表态。三个出路——第三个是这一档独有的：可能是定义错了 */
  decideViolation: (
    kbId: string,
    violationId: string,
    resolution: "fact_retracted" | "axiom_relaxed" | "accepted",
  ) =>
    request<{ ok: boolean }>(
      `/api/v1/kbs/${kbId}/review/violations/${violationId}`,
      { method: "POST", body: JSON.stringify({ resolution }) },
    ),
  confirmFact: (kbId: string, factId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/facts/${factId}/confirm`, {
      method: "POST",
    }),
  rejectFact: (kbId: string, factId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/facts/${factId}/reject`, {
      method: "POST",
    }),
  revertMerge: (kbId: string, mergeId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/merges/${mergeId}/revert`, {
      method: "POST",
    }),
  reviewHistory: (kbId: string, page: number, per = 20) =>
    request<{ events: ReviewHistoryEvent[]; total: number }>(
      `/api/v1/kbs/${kbId}/review/history?page=${page}&per=${per}`,
    ),

  members: (workspaceId: string) =>
    request<Member[]>(`/api/v1/workspaces/${workspaceId}/members`),
  orgUsers: () => request<OrgUser[]>("/api/v1/users"),
  setMemberRole: (workspaceId: string, userId: string, role: string) =>
    request<{ ok: boolean }>(
      `/api/v1/workspaces/${workspaceId}/members/${userId}`,
      {
        method: "PUT",
        body: JSON.stringify({ role }),
      },
    ),
  removeMember: (workspaceId: string, userId: string) =>
    request<{ ok: boolean }>(
      `/api/v1/workspaces/${workspaceId}/members/${userId}`,
      {
        method: "DELETE",
      },
    ),

  testSettings: (workspaceId: string) =>
    request<{
      chat: { ok: boolean; reply?: string; error?: string };
      embed: { ok: boolean; dim?: number; error?: string };
    }>(`/api/v1/workspaces/${workspaceId}/settings/test`, { method: "POST" }),
};

/** RAG 对话：SSE 流式。返回中止函数。 */
export const conversationsApi = {
  /** **可搜可翻页**：标题会重（同一个问题问两次就重了），而固定一百条之后的
   *  会话界面上根本不存在。搜的是标题与消息正文两处——人记得住的往往是
   *  问过的那句话，不是标题 */
  list: (kbId: string, q = "", limit = 30, offset = 0) => {
    const p = new URLSearchParams({
      limit: String(limit),
      offset: String(offset),
    });
    if (q.trim()) p.set("q", q.trim());
    return request<{ conversations: ConversationRow[]; total: number }>(
      `/api/v1/kbs/${kbId}/conversations?${p}`,
    );
  },
  /** 改标题。标题本来是从第一句话自动取的，而一段对话跑偏是常态 */
  rename: (kbId: string, id: string, title: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/conversations/${id}`, {
      method: "PATCH",
      body: JSON.stringify({ title }),
    }),
  detail: (kbId: string, id: string) =>
    request<{ messages: ConversationMessage[] }>(
      `/api/v1/kbs/${kbId}/conversations/${id}`,
    ),
  remove: (kbId: string, id: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/conversations/${id}`, {
      method: "DELETE",
    }),
};

export function streamChat(
  kbId: string,
  body: { conversation_id?: string; message: string },
  handlers: {
    onConversation: (id: string) => void;
    onSources: (s: Source[]) => void;
    onStep: (s: ChatStep) => void;
    onDelta: (text: string) => void;
    onDone: () => void;
    onError: (message: string) => void;
  },
): () => void {
  const controller = new AbortController();
  (async () => {
    try {
      const res = await fetch(`/api/v1/kbs/${kbId}/chat`, {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      if (!res.ok || !res.body) {
        let message = res.statusText;
        try {
          const body = (await res.json()) as { error?: string };
          if (body.error) message = body.error;
        } catch {
          /* ignore */
        }
        handlers.onError(message);
        return;
      }
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buf = "";
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let idx: number;
        while ((idx = buf.indexOf("\n\n")) >= 0) {
          const frame = buf.slice(0, idx);
          buf = buf.slice(idx + 2);
          let event = "message";
          let data = "";
          for (const line of frame.split("\n")) {
            if (line.startsWith("event:")) event = line.slice(6).trim();
            else if (line.startsWith("data:")) data += line.slice(5).trim();
          }
          if (event === "conversation")
            handlers.onConversation((JSON.parse(data) as { id: string }).id);
          else if (event === "sources")
            handlers.onSources(JSON.parse(data || "[]"));
          else if (event === "step")
            handlers.onStep(JSON.parse(data) as ChatStep);
          else if (event === "delta")
            handlers.onDelta((JSON.parse(data) as { text: string }).text);
          else if (event === "done") handlers.onDone();
          else if (event === "error") handlers.onError(data);
        }
      }
      handlers.onDone();
    } catch (e) {
      if (!controller.signal.aborted) handlers.onError(String(e));
    }
  })();
  return () => controller.abort();
}
