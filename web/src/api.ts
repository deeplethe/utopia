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
  kbAudit: (kbId: string) =>
    request<{ events: AuditEvent[] }>(`/api/v1/kbs/${kbId}/audit`),
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

  documents: (kbId: string) => request<Doc[]>(`/api/v1/kbs/${kbId}/documents`),
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
    request<{ nodes: GraphNode[]; edges: GraphEdge[] }>(
      `/api/v1/kbs/${kbId}/graph/overview`,
    ),
  graphNeighborhood: (kbId: string, entityId: string) =>
    request<{ nodes: GraphNode[]; edges: GraphEdge[] }>(
      `/api/v1/kbs/${kbId}/graph/neighborhood?entity=${entityId}&hops=2`,
    ),
  searchEntities: (kbId: string, q: string) =>
    request<{ entities: GraphNode[] }>(
      `/api/v1/kbs/${kbId}/entities?q=${encodeURIComponent(q)}`,
    ),
  entityDetail: (kbId: string, entityId: string) =>
    request<{ entity: GraphNode; facts: EntityFact[] }>(
      `/api/v1/kbs/${kbId}/entities/${entityId}`,
    ),
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

  review: (kbId: string) =>
    request<{
      reviews: ReviewItem[];
      facts: FactReviewItem[];
      merges: MergeLog[];
      conflicts: ConflictItem[];
      unconfirmed: FactReviewItem[];
    }>(`/api/v1/kbs/${kbId}/review`),
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
  list: (kbId: string) =>
    request<{ conversations: ConversationRow[] }>(
      `/api/v1/kbs/${kbId}/conversations`,
    ),
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
