// Thin typed API client. All calls go to /api which Vite proxies to the FastAPI backend.
import type {
  AboxClassList,
  AssertionInput,
  Chunk,
  Conflict,
  HistoryResponse,
  DocumentContribution,
  DocumentImpact,
  DocumentMeta,
  EditOp,
  EditResult,
  ExtractionJob,
  Individual,
  IndividualList,
  KnowledgeSystem,
  GrantableUser,
  Member,
  MemberDetail,
  ModelCatalog,
  OntologyView,
  ParseResponse,
  ReconciliationList,
  ResolutionDecisions,
  ResolutionQueue,
  ReviewCounts,
  Provider,
  ResolveResult,
  Role,
  SourceDoc,
  SystemSettings,
  TestResult,
  User,
  ValidationDecisionList,
  ValidationResult,
} from "./types"

// The AuthProvider registers a handler here so a 401 from any call (e.g. an expired
// session) drops the app back to the login screen instead of surfacing a raw error.
let onUnauthorized: (() => void) | null = null
export function setUnauthorizedHandler(fn: (() => void) | null) {
  onUnauthorized = fn
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "include", ...init })
  if (!res.ok) {
    if (res.status === 401 && onUnauthorized) onUnauthorized()
    let detail = res.statusText
    try {
      const body = await res.json()
      detail = body.detail ?? JSON.stringify(body)
    } catch {
      /* ignore */
    }
    throw new Error(`${res.status}: ${detail}`)
  }
  // Some endpoints (logout) return trivial JSON; a 204 would have no body.
  if (res.status === 204) return undefined as T
  return res.json() as Promise<T>
}

const json = (body: unknown): RequestInit => ({
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
})

const patch = (body: unknown): RequestInit => ({
  method: "PATCH",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
})

export const api = {
  health: () => request<{ status: string; extract_model: string; has_llm_key: boolean }>("/api/health"),

  // System settings + model catalog
  getModels: () => request<ModelCatalog>("/api/models"),
  getSettings: () => request<SystemSettings>("/api/settings"),
  updateSettings: (body: { llm_provider_id?: number; embedding_provider_id?: number }) =>
    request<SystemSettings>("/api/settings", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  // Model entries (a flat list of endpoint + key + model + kind)
  listProviders: () => request<Provider[]>("/api/providers"),
  createProvider: (body: { name: string; kind: "llm" | "embedding"; base_url: string; api_key: string; model: string }) =>
    request<Provider>("/api/providers", json(body)),
  updateProvider: (id: number, body: { name?: string; kind?: "llm" | "embedding"; base_url?: string; api_key?: string; model?: string }) =>
    request<Provider>(`/api/providers/${id}`, patch(body)),
  deleteProvider: (id: number) =>
    request<{ deleted: number }>(`/api/providers/${id}`, { method: "DELETE" }),
  testProvider: (body: {
    provider_id?: number
    base_url?: string
    api_key?: string
    model?: string
    kind?: "llm" | "embedding"
  }) => request<TestResult>("/api/providers/test", json(body)),

  // Auth
  login: (username: string, password: string) =>
    request<User>("/api/auth/login", json({ username, password })),
  logout: () => request<{ ok: boolean }>("/api/auth/logout", { method: "POST" }),
  me: () => request<User>("/api/auth/me"),
  // Self-service profile: set/clear nickname, or change password (needs current password).
  updateMe: (body: { display_name?: string; current_password?: string; new_password?: string }) =>
    request<User>("/api/auth/me", patch(body)),

  // User management (admin)
  listUsers: () => request<User[]>("/api/auth/users"),
  createUser: (username: string, password: string, is_admin = false) =>
    request<User>("/api/auth/users", json({ username, password, is_admin })),
  updateUser: (uid: number, patch: { password?: string; is_admin?: boolean; active?: boolean }) =>
    request<User>(`/api/auth/users/${uid}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    }),
  deleteUser: (uid: number) =>
    request<{ deleted: number }>(`/api/auth/users/${uid}`, { method: "DELETE" }),

  // Documents (scoped to a knowledge system)
  listDocuments: (ksId: number) => request<DocumentMeta[]>(`/api/knowledge/${ksId}/documents`),
  uploadDocument: (ksId: number, file: File, folder = "/") => {
    const fd = new FormData()
    fd.append("file", file)
    fd.append("folder", folder)
    return request<DocumentMeta>(`/api/knowledge/${ksId}/documents/upload`, { method: "POST", body: fd })
  },
  parseDocument: (ksId: number, id: number) =>
    request<ParseResponse>(`/api/knowledge/${ksId}/documents/${id}/parse`, { method: "POST" }),
  getChunks: (ksId: number, id: number) =>
    request<Chunk[]>(`/api/knowledge/${ksId}/documents/${id}/chunks`),
  getContribution: (ksId: number, id: number) =>
    request<DocumentContribution>(`/api/knowledge/${ksId}/documents/${id}/contribution`),
  moveDocument: (ksId: number, id: number, folder?: string, original_filename?: string) =>
    request<DocumentMeta>(`/api/knowledge/${ksId}/documents/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ folder, original_filename }),
    }),
  getImpact: (ksId: number, id: number) =>
    request<DocumentImpact>(`/api/knowledge/${ksId}/documents/${id}/impact`),
  deleteDocument: (ksId: number, id: number) =>
    request<{ deleted: number }>(`/api/knowledge/${ksId}/documents/${id}/delete`, { method: "POST" }),

  // Knowledge systems
  listKS: () => request<KnowledgeSystem[]>("/api/knowledge"),
  createKS: (
    name: string,
    description: string,
    opts?: { llm_provider_id?: number; embedding_provider_id?: number; llm_model?: string | null },
  ) => request<KnowledgeSystem>("/api/knowledge", json({ name, description, ...opts })),
  updateKS: (id: number, body: {
    name?: string
    description?: string
    llm_model?: string | null
    llm_provider_id?: number | null
    embedding_provider_id?: number | null
    embedding_model?: string | null
  }) => request<KnowledgeSystem>(`/api/knowledge/${id}`, patch(body)),
  getKS: (id: number) => request<KnowledgeSystem>(`/api/knowledge/${id}`),
  reviewCounts: (ksId: number) => request<ReviewCounts>(`/api/knowledge/${ksId}/review/counts`),
  deleteKS: (id: number) => request<{ deleted: number }>(`/api/knowledge/${id}`, { method: "DELETE" }),

  // Membership (owner-managed)
  listMembers: (ksId: number) => request<Member[]>(`/api/knowledge/${ksId}/members`),
  grantableUsers: (ksId: number, q?: string) =>
    request<GrantableUser[]>(`/api/knowledge/${ksId}/members/candidates${q ? `?q=${encodeURIComponent(q)}` : ""}`),
  addMember: (ksId: number, username: string, role: Role) =>
    request<Member[]>(`/api/knowledge/${ksId}/members`, json({ username, role })),
  removeMember: (ksId: number, userId: number) =>
    request<{ removed: number }>(`/api/knowledge/${ksId}/members/${userId}`, { method: "DELETE" }),
  getMemberDetail: (ksId: number, userId: number) =>
    request<MemberDetail>(`/api/knowledge/${ksId}/members/${userId}/detail`),

  // Ontology
  getOntology: (ksId: number) => request<OntologyView>(`/api/knowledge/${ksId}/ontology`),
  exportOntology: async (ksId: number, fmt: string): Promise<string> => {
    const res = await fetch(`/api/knowledge/${ksId}/ontology/export?fmt=${fmt}`, { credentials: "include" })
    if (!res.ok) {
      if (res.status === 401 && onUnauthorized) onUnauthorized()
      throw new Error(`${res.status}: ${res.statusText}`)
    }
    return res.text()
  },

  // Extraction (starts a background job; poll it for progress)
  runExtraction: (ksId: number, chunkIds: number[], model?: string) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/extract`, json({ chunk_ids: chunkIds, model })),
  listJobs: (ksId: number) => request<ExtractionJob[]>(`/api/knowledge/${ksId}/jobs`),
  getJob: (ksId: number, jobId: number) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/jobs/${jobId}`),
  getSources: (ksId: number) => request<SourceDoc[]>(`/api/knowledge/${ksId}/sources`),

  // Change history / audit log
  getHistory: (
    ksId: number,
    params: { category?: string; q?: string; limit?: number; offset?: number } = {},
  ) => {
    const qs = new URLSearchParams()
    if (params.category) qs.set("category", params.category)
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 20))
    qs.set("offset", String(params.offset ?? 0))
    return request<HistoryResponse>(`/api/knowledge/${ksId}/history?${qs.toString()}`)
  },
  rollbackHistory: (ksId: number, eventId: number) =>
    request<{ undone: number; view: OntologyView; open_conflicts: Conflict[] }>(
      `/api/knowledge/${ksId}/history/${eventId}/rollback`,
      { method: "POST" },
    ),

  // Manual editing
  editOntology: (ksId: number, op: EditOp) =>
    request<EditResult>(`/api/knowledge/${ksId}/ontology/edit`, json(op)),

  // Conflicts
  detectConflicts: (ksId: number) =>
    request<Conflict[]>(`/api/knowledge/${ksId}/conflicts/detect`, { method: "POST" }),
  listConflicts: (ksId: number, status = "open", ctype?: string) =>
    request<Conflict[]>(`/api/knowledge/${ksId}/conflicts?status=${status}${ctype ? `&ctype=${ctype}` : ""}`),
  resolveConflict: (ksId: number, cid: number, resolutionId: string) =>
    request<ResolveResult>(
      `/api/knowledge/${ksId}/conflicts/${cid}/resolve`,
      json({ resolution_id: resolutionId }),
    ),
  dismissConflict: (ksId: number, cid: number) =>
    request<Conflict>(`/api/knowledge/${ksId}/conflicts/${cid}/dismiss`, { method: "POST" }),
  reopenConflict: (ksId: number, cid: number) =>
    request<Conflict>(`/api/knowledge/${ksId}/conflicts/${cid}/reopen`, { method: "POST" }),

  // Learned reconciliation memory (TBox domain/range decisions the reconcile agent consults)
  listReconciliations: (ksId: number, params: { q?: string; limit?: number; offset?: number } = {}) => {
    const qs = new URLSearchParams()
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 50))
    qs.set("offset", String(params.offset ?? 0))
    return request<ReconciliationList>(`/api/knowledge/${ksId}/reconciliations?${qs.toString()}`)
  },
  revokeReconciliation: (ksId: number, id: number) =>
    request<{ revoked: number }>(`/api/knowledge/${ksId}/reconciliations/${id}`, { method: "DELETE" }),
  editReconciliationReason: (ksId: number, id: number, reason: string) =>
    request<{ id: number; reason: string }>(`/api/knowledge/${ksId}/reconciliations/${id}`, patch({ reason })),
  revokeResolutionDecision: (ksId: number, id: number) =>
    request<{ revoked: number }>(`/api/knowledge/${ksId}/resolution/decisions/${id}`, { method: "DELETE" }),
  editResolutionReason: (ksId: number, id: number, reason: string) =>
    request<{ id: number; reason: string }>(`/api/knowledge/${ksId}/resolution/decisions/${id}`, patch({ reason })),

  // ABox (instances)
  aboxClasses: (ksId: number) => request<AboxClassList>(`/api/knowledge/${ksId}/abox/classes`),
  aboxIndividuals: (
    ksId: number,
    params: { class_iri?: string; q?: string; limit?: number; offset?: number } = {},
  ) => {
    const qs = new URLSearchParams()
    if (params.class_iri) qs.set("class_iri", params.class_iri)
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 20))
    qs.set("offset", String(params.offset ?? 0))
    return request<IndividualList>(`/api/knowledge/${ksId}/abox/individuals?${qs.toString()}`)
  },
  getIndividual: (ksId: number, iri: string) =>
    request<Individual>(`/api/knowledge/${ksId}/abox/individual?iri=${encodeURIComponent(iri)}`),
  createIndividual: (ksId: number, label: string, classIri: string) =>
    request<Individual>(`/api/knowledge/${ksId}/abox/individuals`, json({ label, class_iri: classIri })),
  deleteIndividual: (ksId: number, iri: string) =>
    request<{ removed: number }>(`/api/knowledge/${ksId}/abox/individuals/delete`, json({ iri })),
  addAssertion: (ksId: number, a: AssertionInput) =>
    request<Individual>(`/api/knowledge/${ksId}/abox/assertions`, json(a)),
  removeAssertion: (ksId: number, a: AssertionInput) =>
    request<Individual>(`/api/knowledge/${ksId}/abox/assertions/delete`, json(a)),

  // ABox instance extraction (background job; poll it like TBox extraction)
  extractInstances: (ksId: number, chunkIds: number[], model?: string) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/extract-instances`, json({ chunk_ids: chunkIds, model })),
  // One-click schema + instances (TBox then ABox in a single job)
  extractAll: (ksId: number, chunkIds: number[], model?: string) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/extract-all`, json({ chunk_ids: chunkIds, model })),

  // Entity resolution: manual queue + learned decision log
  getResolutionQueue: (ksId: number, params: { q?: string; limit?: number; offset?: number } = {}) => {
    const qs = new URLSearchParams()
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 50))
    qs.set("offset", String(params.offset ?? 0))
    return request<ResolutionQueue>(`/api/knowledge/${ksId}/resolution/queue?${qs.toString()}`)
  },
  getResolutionDecisions: (ksId: number, params: { q?: string; limit?: number; offset?: number } = {}) => {
    const qs = new URLSearchParams()
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 50))
    qs.set("offset", String(params.offset ?? 0))
    return request<ResolutionDecisions>(`/api/knowledge/${ksId}/resolution/decisions?${qs.toString()}`)
  },
  resolveQueueItem: (ksId: number, resId: number, action: "match" | "new", individualIri?: string) =>
    request<{ id: number; status: string; individual_iri: string | null; summary: string }>(
      `/api/knowledge/${ksId}/resolution/${resId}/resolve`,
      json({ action, individual_iri: individualIri }),
    ),

  // ABox validation (lint individuals against the TBox)
  validateAbox: (ksId: number) => request<ValidationResult>(`/api/knowledge/${ksId}/abox/validate`),
  fixViolation: (ksId: number, op: Record<string, unknown>, summary: string) =>
    request<ValidationResult>(`/api/knowledge/${ksId}/abox/validate/fix`, json({ op, summary })),
  listValidationDecisions: (ksId: number, params: { q?: string; limit?: number; offset?: number } = {}) => {
    const qs = new URLSearchParams()
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 50))
    qs.set("offset", String(params.offset ?? 0))
    return request<ValidationDecisionList>(`/api/knowledge/${ksId}/validation/decisions?${qs.toString()}`)
  },
  revokeValidationDecision: (ksId: number, id: number) =>
    request<{ revoked: number }>(`/api/knowledge/${ksId}/validation/decisions/${id}`, { method: "DELETE" }),
}
