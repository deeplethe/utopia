export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.status = status;
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, {
    credentials: "include",
    headers: init?.body instanceof FormData ? {} : { "Content-Type": "application/json" },
    ...init,
  });
  if (!res.ok) {
    let message = res.statusText;
    try {
      const body = (await res.json()) as { error?: string };
      if (body.error) message = body.error;
    } catch {
      // 非 JSON 响应体，保留 statusText
    }
    throw new ApiError(res.status, message);
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

export interface Kb {
  id: string;
  workspace_id: string;
  name: string;
  kind: string;
  description: string | null;
  visibility: "open" | "restricted";
  /** 部署的公共默认空间（第一个建的库）：永远 open、不可删除 */
  is_default: boolean;
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
  error: string | null;
  chunk_count: number;
  tags: string[];
  missing_since: string | null;
  created_at: string;
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
  type_key: string;
  type_label: string;
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
  predicate: string;
  object_id: string | null;
  object: string | null;
  valid_from: string | null;
  valid_to: string | null;
  confidence: number;
}

export interface ReviewSide {
  id: string;
  name: string;
  type_label: string;
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
  predicate_label: string;
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
  predicate: string;
  label: string;
  valid_from: string | null;
  valid_to: string | null;
  confidence: number;
}

export interface EntityFact {
  id: string;
  direction: "out" | "in";
  predicate_key: string;
  predicate_label: string;
  temporal: string;
  other_id: string | null;
  other_name: string | null;
  /** 字面值宾语（属性事实/问数映射）：{"value":…} 或 {"summary":…} */
  object_value: Record<string, unknown> | null;
  valid_from: string | null;
  valid_to: string | null;
  valid_precision: string;
  confidence: number;
  evidence_count: number;
  /** 证据全部停留在来源文档的旧版（未被现行内容确认；不代表事实失效） */
  stale: boolean;
  /** 修正行：区间闭合来自引擎对账/人工裁决而非抽取原文 */
  corrected: boolean;
  /** 证据集合里最新的文档时间（开放事实的"最后确认时间"） */
  last_evidence_time: string | null;
}

export interface Evidence {
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
  parent_id: string | null;
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
  domain_type_id: string | null;
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

export interface OntologyProposals {
  entity_types: { key: string; label: string; reason?: string }[];
  relation_types: {
    key: string;
    label: string;
    temporal?: string;
    functional?: boolean;
    reason?: string;
  }[];
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
  kind: "search" | "docs" | "entity" | "facts" | "query" | "tool";
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

export const api = {
  health: () => request<{ status: string; name: string; version: string }>("/api/v1/health"),
  me: () => request<User>("/api/v1/auth/me"),
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
  logout: () => request<{ ok: boolean }>("/api/v1/auth/logout", { method: "POST" }),
  updateMe: (displayName: string) =>
    request<User>("/api/v1/auth/me", {
      method: "PATCH",
      body: JSON.stringify({ display_name: displayName }),
    }),
  changePassword: (currentPassword: string, newPassword: string) =>
    request<{ ok: boolean }>("/api/v1/auth/password", {
      method: "POST",
      body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
    }),
  workspaces: () => request<Workspace[]>("/api/v1/workspaces"),

  kbs: (workspaceId: string) => request<Kb[]>(`/api/v1/workspaces/${workspaceId}/kbs`),
  kbAudit: (kbId: string) =>
    request<{ events: AuditEvent[] }>(`/api/v1/kbs/${kbId}/audit`),
  myKbs: (workspaceId: string) =>
    request<{ kbs: MyKb[] }>(`/api/v1/workspaces/${workspaceId}/my-kbs`),
  createKb: (
    workspaceId: string,
    body: { name: string; description?: string | null; visibility?: string },
  ) =>
    request<Kb>(`/api/v1/workspaces/${workspaceId}/kbs`, {
      method: "POST",
      body: JSON.stringify(body),
    }),

  kbDetail: (kbId: string) => request<Kb>(`/api/v1/kbs/${kbId}`),
  updateKb: (kbId: string, body: Record<string, unknown>) =>
    request<Kb>(`/api/v1/kbs/${kbId}`, { method: "PATCH", body: JSON.stringify(body) }),
  deleteKb: (kbId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}`, { method: "DELETE" }),
  kbMembers: (kbId: string) => request<{ members: KbMember[] }>(`/api/v1/kbs/${kbId}/members`),
  setKbMember: (kbId: string, userId: string, role: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/members/${userId}`, {
      method: "PUT",
      body: JSON.stringify({ role }),
    }),
  removeKbMember: (kbId: string, userId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/members/${userId}`, { method: "DELETE" }),

  adminDeployment: () =>
    request<{ open_registration: boolean; worker_concurrency: number }>(
      "/api/v1/admin/deployment",
    ),
  saveAdminDeployment: (openRegistration: boolean, workerConcurrency?: number) =>
    request<{ ok: boolean }>("/api/v1/admin/deployment", {
      method: "PUT",
      body: JSON.stringify({
        open_registration: openRegistration,
        ...(workerConcurrency !== undefined ? { worker_concurrency: workerConcurrency } : {}),
      }),
    }),
  adminCreateUser: (body: { email: string; display_name: string; password: string; role: string }) =>
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
    request<{ ok: boolean }>(`/api/v1/admin/data-sources/${id}`, { method: "DELETE" }),
  adminTestDataSource: (id: string) =>
    request<{ ok: boolean }>(`/api/v1/admin/data-sources/${id}/test`, { method: "POST" }),
  kbDataSources: (kbId: string) =>
    request<{ data_sources: DataSourceView[] }>(`/api/v1/kbs/${kbId}/data-sources`),
  kbDataSourcesAvailable: (kbId: string) =>
    request<{ data_sources: DataSourceView[] }>(`/api/v1/kbs/${kbId}/data-sources/available`),
  mountDataSource: (kbId: string, dsId: string) =>
    request<{ ok: boolean; schema_tables: number }>(`/api/v1/kbs/${kbId}/data-sources/${dsId}`, {
      method: "PUT",
    }),
  unmountDataSource: (kbId: string, dsId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/data-sources/${dsId}`, { method: "DELETE" }),
  exploreMappings: (kbId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/data-sources/explore`, { method: "POST" }),
  syncDataSourceSchema: (kbId: string, dsId: string) =>
    request<{ ok: boolean; schema_tables: number }>(
      `/api/v1/kbs/${kbId}/data-sources/${dsId}/sync-schema`,
      { method: "POST" },
    ),

  documents: (kbId: string) => request<Doc[]>(`/api/v1/kbs/${kbId}/documents`),
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
    request<{ nodes: GraphNode[]; edges: GraphEdge[] }>(`/api/v1/kbs/${kbId}/graph/overview`),
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
  factEvidence: (kbId: string, factId: string) =>
    request<{ evidence: Evidence[] }>(`/api/v1/kbs/${kbId}/facts/${factId}/evidence`),
  documentDetail: (id: string) =>
    request<{ document: Doc; chunks: ChunkFull[] }>(`/api/v1/documents/${id}`),
  extractDocument: (id: string) =>
    request<{ job_id: number }>(`/api/v1/documents/${id}/extract`, { method: "POST" }),
  reprocessDocument: (id: string) =>
    request<{ job_id: number }>(`/api/v1/documents/${id}/reprocess`, { method: "POST" }),

  ontology: (kbId: string) =>
    request<{
      entity_types: EntityTypeView[];
      relation_types: RelationTypeView[];
      misses: OntologyMiss[];
    }>(`/api/v1/kbs/${kbId}/ontology`),
  createEntityType: (kbId: string, body: Record<string, unknown>) =>
    request<{ id: string }>(`/api/v1/kbs/${kbId}/ontology/entity-types`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  updateEntityType: (kbId: string, id: string, body: Record<string, unknown>) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/entity-types/${id}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteEntityType: (kbId: string, id: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/entity-types/${id}`, {
      method: "DELETE",
    }),
  typeEntities: (kbId: string, typeId: string, page: number, per = 12) =>
    request<{ entities: { id: string; name: string; fact_count: number }[]; total: number }>(
      `/api/v1/kbs/${kbId}/ontology/entity-types/${typeId}/entities?page=${page}&per=${per}`,
    ),
  createRelationType: (kbId: string, body: Record<string, unknown>) =>
    request<{ id: string }>(`/api/v1/kbs/${kbId}/ontology/relation-types`, {
      method: "POST",
      body: JSON.stringify(body),
    }),
  updateRelationType: (kbId: string, id: string, body: Record<string, unknown>) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/relation-types/${id}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteRelationType: (kbId: string, id: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/relation-types/${id}`, {
      method: "DELETE",
    }),
  dismissMiss: (kbId: string, kind: string, key: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/ontology/misses/dismiss`, {
      method: "POST",
      body: JSON.stringify({ kind, key }),
    }),
  suggestOntology: (kbId: string) =>
    request<OntologyProposals>(`/api/v1/kbs/${kbId}/ontology/suggest`, { method: "POST" }),

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
  updateSource: (kbId: string, sourceId: string, body: Record<string, unknown>) =>
    request<{ source: SourceView }>(`/api/v1/kbs/${kbId}/sources/${sourceId}`, {
      method: "PATCH",
      body: JSON.stringify(body),
    }),
  deleteSource: (kbId: string, sourceId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/sources/${sourceId}`, { method: "DELETE" }),
  cleanupMissing: (kbId: string, sourceId: string) =>
    request<{ deleted: number }>(`/api/v1/kbs/${kbId}/sources/${sourceId}/missing/cleanup`, {
      method: "POST",
    }),
  syncSource: (kbId: string, sourceId: string) =>
    request<{ queued: boolean }>(`/api/v1/kbs/${kbId}/sources/${sourceId}/sync`, {
      method: "POST",
    }),
  sourceRuns: (kbId: string, sourceId: string) =>
    request<{ runs: SyncRun[] }>(`/api/v1/kbs/${kbId}/sources/${sourceId}/runs`),
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
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/facts/${factId}/confirm`, { method: "POST" }),
  rejectFact: (kbId: string, factId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/facts/${factId}/reject`, { method: "POST" }),
  revertMerge: (kbId: string, mergeId: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/merges/${mergeId}/revert`, { method: "POST" }),
  reviewHistory: (kbId: string, page: number, per = 20) =>
    request<{ events: ReviewHistoryEvent[]; total: number }>(
      `/api/v1/kbs/${kbId}/review/history?page=${page}&per=${per}`,
    ),

  members: (workspaceId: string) => request<Member[]>(`/api/v1/workspaces/${workspaceId}/members`),
  orgUsers: () => request<OrgUser[]>("/api/v1/users"),
  setMemberRole: (workspaceId: string, userId: string, role: string) =>
    request<{ ok: boolean }>(`/api/v1/workspaces/${workspaceId}/members/${userId}`, {
      method: "PUT",
      body: JSON.stringify({ role }),
    }),
  removeMember: (workspaceId: string, userId: string) =>
    request<{ ok: boolean }>(`/api/v1/workspaces/${workspaceId}/members/${userId}`, {
      method: "DELETE",
    }),

  testSettings: (workspaceId: string) =>
    request<{
      chat: { ok: boolean; reply?: string; error?: string };
      embed: { ok: boolean; dim?: number; error?: string };
    }>(`/api/v1/workspaces/${workspaceId}/settings/test`, { method: "POST" }),
};

/** RAG 对话：SSE 流式。返回中止函数。 */
export const conversationsApi = {
  list: (kbId: string) =>
    request<{ conversations: ConversationRow[] }>(`/api/v1/kbs/${kbId}/conversations`),
  detail: (kbId: string, id: string) =>
    request<{ messages: ConversationMessage[] }>(`/api/v1/kbs/${kbId}/conversations/${id}`),
  remove: (kbId: string, id: string) =>
    request<{ ok: boolean }>(`/api/v1/kbs/${kbId}/conversations/${id}`, { method: "DELETE" }),
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
          else if (event === "sources") handlers.onSources(JSON.parse(data || "[]"));
          else if (event === "step") handlers.onStep(JSON.parse(data) as ChatStep);
          else if (event === "delta") handlers.onDelta((JSON.parse(data) as { text: string }).text);
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
