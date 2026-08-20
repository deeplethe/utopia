// Thin typed API client. All calls go to /api which Vite proxies to the FastAPI backend.
import type {
  AboxClassList,
  AgentChatRequest,
  AgentConversation,
  AgentConversationDetail,
  AgentConversationList,
  AgentResponse,
  AgentStreamEvent,
  ApiToken,
  ApiTokenCreated,
  ApiTokenRevealed,
  ApiTokenScope,
  AssertionInput,
  Chunk,
  Conflict,
  ConflictContext,
  HistoryResponse,
  DocumentContribution,
  DocumentImpact,
  DocumentListResponse,
  DocumentMeta,
  EditOp,
  EditResult,
  ExtractionJob,
  ExportJob,
  ExportList,
  Individual,
  IndividualList,
  KnowledgePrompt,
  KnowledgePromptList,
  KnowledgeSystem,
  GrantableUser,
  Member,
  MemberDetail,
  ModelCatalog,
  OntologyView,
  OntologyChangeSetResult,
  OntologyImpactResponse,
  OntologySuggestion,
  OntologyRelease,
  ParseResponse,
  ParseBatchResponse,
  ReconciliationList,
  ResolutionDecisions,
  ResolutionQueue,
  ReviewCounts,
  Provider,
  ReleaseDiff,
  ReleaseLayer,
  ReleaseList,
  RdfImportFormat,
  RdfImportResult,
  RdfImportStrategy,
  RdfImportTarget,
  ResolveResult,
  Role,
  SourceDoc,
  SystemSettings,
  TestResult,
  User,
  ValidationDecisionList,
  ValidationResult,
  VocabularyConcept,
  VocabularyConceptList,
  VocabularyConceptInput,
  VocabularyScheme,
  VocabularySchemeList,
  VocabularyView,
  TermProposal,
  TermProposalList,
} from "./types"

// The AuthProvider registers a handler here so a 401 from any call (e.g. an expired
// session) drops the app back to the login screen instead of surfacing a raw error.
let onUnauthorized: (() => void) | null = null
export function setUnauthorizedHandler(fn: (() => void) | null) {
  onUnauthorized = fn
}

function errorMessage(detail: unknown) {
  if (typeof detail === "string") return detail
  if (detail && typeof detail === "object" && "message" in detail) {
    const message = (detail as { message?: unknown }).message
    if (typeof message === "string") return message
  }
  return JSON.stringify(detail)
}

export class ApiError extends Error {
  readonly status: number
  readonly detail: unknown

  constructor(status: number, detail: unknown) {
    // Keep the HTTP status as structured data.  User-facing callers should receive the backend's
    // explanation, not an Axios-style status wrapper that hides the actionable message.
    super(errorMessage(detail))
    this.name = "ApiError"
    this.status = status
    this.detail = detail
  }
}

const EXTRACTION_CONFLICT = "An extraction is in progress; try again after it finishes."

async function readErrorDetail(res: Response, path = ""): Promise<unknown> {
  const fallback = res.status === 409 && (
    path.includes("/documents/") || path.includes("/documents/parse-batch") || path.includes("/extract")
  ) ? EXTRACTION_CONFLICT : (res.statusText || `HTTP ${res.status}`)
  let raw = ""
  try {
    raw = await res.text()
  } catch {
    return fallback
  }
  if (!raw.trim()) return fallback
  try {
    const body = JSON.parse(raw) as unknown
    if (body && typeof body === "object" && "detail" in body) {
      return (body as { detail?: unknown }).detail ?? fallback
    }
    return body
  } catch {
    // Reverse proxies sometimes replace JSON with a plain-text explanation.  Preserve it.
    return raw.trim() || fallback
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "include", ...init })
  if (!res.ok) {
    if (res.status === 401 && onUnauthorized) onUnauthorized()
    const detail = await readErrorDetail(res, path)
    throw new ApiError(res.status, detail)
  }
  // Some endpoints (logout) return trivial JSON; a 204 would have no body.
  if (res.status === 204) return undefined as T
  return res.json() as Promise<T>
}

async function responseError(res: Response, path = "") {
  if (res.status === 401 && onUnauthorized) onUnauthorized()
  const detail = await readErrorDetail(res, path)
  return new ApiError(res.status, detail)
}

function agentTraceStep(value: unknown): AgentResponse["trace"][number] | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  const candidate = value as Record<string, unknown>
  if (typeof candidate.tool !== "string") return null
  return {
    tool: candidate.tool,
    arguments: candidate.arguments && typeof candidate.arguments === "object" && !Array.isArray(candidate.arguments)
      ? candidate.arguments as Record<string, unknown>
      : {},
    summary: typeof candidate.summary === "string" ? candidate.summary : "MCP tool completed",
    reason: typeof candidate.reason === "string" ? candidate.reason : undefined,
  }
}

function agentProposal(value: unknown): AgentResponse["proposal"] {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  const candidate = value as Record<string, unknown>
  return Array.isArray(candidate.operations) ? candidate as unknown as AgentResponse["proposal"] : null
}

function agentConversation(value: unknown, fallbackId?: unknown): AgentConversation | null {
  const candidate = value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
  const rawId = candidate.id ?? fallbackId
  const id = typeof rawId === "number" ? rawId : Number(rawId)
  if (!Number.isInteger(id) || id <= 0) return null
  return {
    id,
    knowledge_system_id: typeof candidate.knowledge_system_id === "number"
      ? candidate.knowledge_system_id
      : undefined,
    title: typeof candidate.title === "string" ? candidate.title : null,
    first_user_message: typeof candidate.first_user_message === "string"
      ? candidate.first_user_message
      : undefined,
    turn_count: typeof candidate.turn_count === "number" ? candidate.turn_count : undefined,
    created_at: typeof candidate.created_at === "string" ? candidate.created_at : undefined,
    updated_at: typeof candidate.updated_at === "string" ? candidate.updated_at : undefined,
  }
}

function parseAgentStreamEvent(eventName: string, data: string): AgentStreamEvent | null {
  if (!data || data === "[DONE]") return null

  let payload: Record<string, unknown>
  try {
    const parsed = JSON.parse(data) as unknown
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null
    payload = parsed as Record<string, unknown>
  } catch {
    throw new Error("The agent returned a malformed stream event")
  }

  const type = eventName === "message" && typeof payload.type === "string"
    ? payload.type
    : eventName
  if (type === "turn_started") {
    const conversation = agentConversation(payload.conversation, payload.conversation_id)
    if (!conversation) return null
    return {
      type,
      conversation,
      conversation_id: conversation.id,
      user_turn_id: typeof payload.user_turn_id === "number" ? payload.user_turn_id : undefined,
      assistant_turn_id: typeof payload.assistant_turn_id === "number" ? payload.assistant_turn_id : undefined,
    }
  }
  if (type === "progress") {
    return {
      type,
      phase: typeof payload.phase === "string" ? payload.phase : undefined,
      title: typeof payload.title === "string" ? payload.title : undefined,
      detail: typeof payload.detail === "string" ? payload.detail : undefined,
      message: typeof payload.message === "string" ? payload.message : undefined,
    }
  }
  if (type === "commentary") {
    return typeof payload.text === "string" ? { type, text: payload.text } : null
  }
  if (type === "trace") {
    const trace = agentTraceStep(payload.trace ?? payload)
    return trace ? { type, trace } : null
  }
  if (type === "answer_reset") return { type }
  if (type === "delta") {
    return typeof payload.delta === "string" ? { type, delta: payload.delta } : null
  }
  if (type === "proposal") {
    return { type, proposal: agentProposal(payload.proposal) }
  }
  if (type === "error") {
    return {
      type,
      code: typeof payload.code === "string" ? payload.code : undefined,
      message: typeof payload.message === "string" ? payload.message : "The agent stream failed",
    }
  }
  if (type === "done") {
    return {
      type,
      answer: typeof payload.answer === "string" ? payload.answer : "",
      trace: Array.isArray(payload.trace)
        ? payload.trace.map(agentTraceStep).filter((step): step is AgentResponse["trace"][number] => Boolean(step))
        : [],
      proposal: agentProposal(payload.proposal),
      conversation: agentConversation(payload.conversation, payload.conversation_id) ?? undefined,
    }
  }
  return null
}

async function streamAgentChat(
  path: string,
  body: AgentChatRequest,
  onEvent: (event: AgentStreamEvent) => void,
  signal?: AbortSignal,
): Promise<AgentResponse> {
  const res = await fetch(path, {
    ...json(body),
    credentials: "include",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream, application/json" },
    signal,
  })
  if (!res.ok) throw await responseError(res)

  const contentType = res.headers.get("content-type")?.toLowerCase() ?? ""
  if (contentType.includes("application/json")) {
    const event = parseAgentStreamEvent("done", await res.text())
    if (!event || event.type !== "done") throw new Error("The agent returned an invalid final response")
    onEvent(event)
    return {
      answer: event.answer,
      trace: event.trace,
      proposal: event.proposal,
      conversation: event.conversation,
    }
  }
  if (!res.body) throw new Error("The browser could not read the agent stream")

  const reader = res.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  let accumulatedAnswer = ""
  let latestTrace: AgentResponse["trace"] = []
  let latestProposal: AgentResponse["proposal"] = null
  let finalResult: AgentResponse | null = null
  let receivedDone = false
  let latestConversation: AgentConversation | undefined
  let pendingDelta = ""
  let deltaFlushTimer: number | null = null
  const presentationIntervalMs = 32
  const presentationChunkSize = 10

  // Provider fragments can arrive several at a time in one network read. Updating React and
  // re-rendering Markdown for every tiny fragment makes a real stream look like a burst (and is
  // expensive for long answers). Keep the wire stream authoritative while presenting adjacent
  // text at a steady ~25 fps, matching the event-consumption pattern used by Neurocircuits.
  const clearDeltaTimer = () => {
    if (deltaFlushTimer != null) {
      window.clearTimeout(deltaFlushTimer)
      deltaFlushTimer = null
    }
  }

  const takePendingDelta = (maxCharacters: number) => {
    if (!pendingDelta) return
    const characters = Array.from(pendingDelta)
    const delta = characters.slice(0, maxCharacters).join("")
    pendingDelta = characters.slice(maxCharacters).join("")
    onEvent({ type: "delta", delta })
  }

  const flushPendingDelta = () => {
    clearDeltaTimer()
    if (!pendingDelta) return
    const delta = pendingDelta
    pendingDelta = ""
    onEvent({ type: "delta", delta })
  }

  const scheduleDeltaFlush = () => {
    if (deltaFlushTimer != null) return
    deltaFlushTimer = window.setTimeout(() => {
      deltaFlushTimer = null
      if (!pendingDelta) return
      takePendingDelta(presentationChunkSize)
      if (pendingDelta) scheduleDeltaFlush()
    }, presentationIntervalMs)
  }

  const drainPendingDelta = async () => {
    clearDeltaTimer()
    if (!pendingDelta) return
    // A validated answer may arrive as one buffered burst. Reveal it over a bounded number of
    // frames so the user can follow the response without stretching long answers indefinitely.
    const chunkSize = Math.max(
      presentationChunkSize,
      Math.ceil(Array.from(pendingDelta).length / 90),
    )
    while (pendingDelta) {
      signal?.throwIfAborted()
      takePendingDelta(chunkSize)
      if (pendingDelta) {
        await new Promise<void>((resolve) => window.setTimeout(resolve, presentationIntervalMs))
      }
    }
  }

  const discardPendingDelta = () => {
    clearDeltaTimer()
    pendingDelta = ""
  }

  const consumeFrame = async (frame: string) => {
    signal?.throwIfAborted()
    let eventName = "message"
    const dataLines: string[] = []
    for (const line of frame.split(/\r?\n/)) {
      if (!line || line.startsWith(":")) continue
      const separator = line.indexOf(":")
      const field = separator === -1 ? line : line.slice(0, separator)
      let value = separator === -1 ? "" : line.slice(separator + 1)
      if (value.startsWith(" ")) value = value.slice(1)
      if (field === "event") eventName = value
      if (field === "data") dataLines.push(value)
    }
    if (!dataLines.length) return
    const event = parseAgentStreamEvent(eventName, dataLines.join("\n"))
    if (!event) return
    if (event.type === "turn_started") latestConversation = event.conversation
    if (event.type === "answer_reset") {
      accumulatedAnswer = ""
      // Text that has not reached the screen belongs to the discarded candidate and should
      // never flash immediately before its reset event.
      discardPendingDelta()
    } else if (event.type === "delta") {
      accumulatedAnswer += event.delta
      pendingDelta += event.delta
      scheduleDeltaFlush()
      return
    } else if (event.type === "done") {
      await drainPendingDelta()
    } else {
      // Preserve chronological text → tool/proposal/done ordering.
      flushPendingDelta()
    }
    if (event.type === "trace") latestTrace = [...latestTrace, event.trace]
    if (event.type === "proposal") latestProposal = event.proposal
    if (event.type === "error") {
      // Deliver the terminal event before rejecting so the in-flight message can retain
      // its streamed text and show the server-provided failure in the transcript.
      onEvent(event)
      throw new Error(event.message)
    }
    if (event.type === "done") {
      receivedDone = true
      finalResult = {
        // Native deltas are the authoritative transcript. A server-provided final answer
        // is only a fallback for JSON/non-streaming implementations.
        answer: accumulatedAnswer || event.answer,
        trace: event.trace.length ? event.trace : latestTrace,
        proposal: event.proposal ?? latestProposal,
        conversation: event.conversation ?? latestConversation,
      }
    }
    onEvent(event)
  }

  try {
    while (true) {
      const { done, value } = await reader.read()
      buffer += decoder.decode(value, { stream: !done })
      let boundary = buffer.search(/\r?\n\r?\n/)
      while (boundary !== -1) {
        const match = buffer.slice(boundary).match(/^\r?\n\r?\n/)?.[0] ?? "\n\n"
        await consumeFrame(buffer.slice(0, boundary))
        buffer = buffer.slice(boundary + match.length)
        boundary = buffer.search(/\r?\n\r?\n/)
      }
      if (done) break
    }
    if (buffer.trim()) await consumeFrame(buffer)
    await drainPendingDelta()
  } catch (error) {
    flushPendingDelta()
    await reader.cancel().catch(() => undefined)
    throw error
  } finally {
    if (deltaFlushTimer != null) window.clearTimeout(deltaFlushTimer)
    reader.releaseLock()
  }
  if (!receivedDone || !finalResult) {
    throw new Error("The agent stream ended before the final response arrived")
  }

  return finalResult
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

const put = (body: unknown): RequestInit => ({
  method: "PUT",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
})

export const api = {
  health: () => request<{ status: string; extract_model: string; has_llm_key: boolean }>("/api/health"),

  // Immutable ontology releases + asynchronous uncompressed N-Quads exports
  listReleases: (ksId: number) => request<ReleaseList>(`/api/knowledge/${ksId}/releases`),
  createRelease: (ksId: number, body: { version?: string; title?: string; notes?: string; shard_size?: number } = {}) =>
    request<OntologyRelease>(`/api/knowledge/${ksId}/releases`, json(body)),
  reviewRelease: (ksId: number, releaseId: number, note = "") =>
    request<OntologyRelease>(`/api/knowledge/${ksId}/releases/${releaseId}/review`, json({ note })),
  publishRelease: (ksId: number, releaseId: number, note = "") =>
    request<OntologyRelease>(`/api/knowledge/${ksId}/releases/${releaseId}/publish`, json({ note })),
  deployRelease: (ksId: number, releaseId: number) =>
    request<OntologyRelease>(`/api/knowledge/${ksId}/releases/${releaseId}/deployment`, json({})),
  stopReleaseService: (ksId: number, releaseId: number) =>
    request<OntologyRelease>(`/api/knowledge/${ksId}/releases/${releaseId}/deployment`, { method: "DELETE" }),
  deleteRelease: (ksId: number, releaseId: number) =>
    request<OntologyRelease>(`/api/knowledge/${ksId}/releases/${releaseId}`, { method: "DELETE" }),
  rollbackRelease: (ksId: number, releaseId: number) =>
    request<{ restored: number; version: string }>(`/api/knowledge/${ksId}/releases/${releaseId}/rollback`, json({})),
  diffReleases: (ksId: number, fromId: number, toId: number) =>
    request<ReleaseDiff>(`/api/knowledge/${ksId}/releases/diff?from_id=${fromId}&to_id=${toId}`),
  listExports: (ksId: number) => request<ExportList>(`/api/knowledge/${ksId}/exports`),
  createExport: (ksId: number, layer: ReleaseLayer, releaseId?: number, shardSize = 100_000) =>
    request<ExportJob>(`/api/knowledge/${ksId}/exports`, json({ layer, release_id: releaseId, shard_size: shardSize })),
  getExport: (ksId: number, jobId: number) => request<ExportJob>(`/api/knowledge/${ksId}/exports/${jobId}`),
  exportFileUrl: (ksId: number, jobId: number, filename: string) =>
    `/api/knowledge/${ksId}/exports/${jobId}/files/${encodeURIComponent(filename)}`,

  // System settings + model catalog
  getModels: () => request<ModelCatalog>("/api/models"),
  getSettings: () => request<SystemSettings>("/api/settings"),
  updateSettings: (body: {
    llm_provider_id?: number
    embedding_provider_id?: number
  }) =>
    request<SystemSettings>("/api/settings", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),

  // Model entries (a flat list of endpoint + key + model + kind)
  listProviders: () => request<Provider[]>("/api/providers"),
  createProvider: (body: { name: string; kind: "llm" | "embedding"; base_url: string; api_key: string; model: string; concurrency_limit: number }) =>
    request<Provider>("/api/providers", json(body)),
  updateProvider: (id: number, body: { name?: string; kind?: "llm" | "embedding"; base_url?: string; api_key?: string; model?: string; concurrency_limit?: number }) =>
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
  getDocument: (ksId: number, id: number) =>
    request<DocumentMeta>(`/api/knowledge/${ksId}/documents/${id}`),
  listDocumentsPage: (
    ksId: number,
    params: {
      folder?: string
      q?: string
      status?: "pending" | "parsed" | "failed"
      limit?: number
      offset?: number
    } = {},
  ) => {
    const qs = new URLSearchParams()
    if (params.folder !== undefined) qs.set("folder", params.folder)
    if (params.q) qs.set("q", params.q)
    if (params.status) qs.set("status", params.status)
    qs.set("limit", String(params.limit ?? 20))
    qs.set("offset", String(params.offset ?? 0))
    return request<DocumentListResponse>(`/api/knowledge/${ksId}/documents/page?${qs.toString()}`)
  },
  uploadDocument: (ksId: number, file: File, folder = "/") => {
    const fd = new FormData()
    fd.append("file", file)
    fd.append("folder", folder)
    return request<DocumentMeta>(`/api/knowledge/${ksId}/documents/upload`, { method: "POST", body: fd })
  },
  parseDocument: (ksId: number, id: number) =>
    request<ParseResponse>(`/api/knowledge/${ksId}/documents/${id}/parse`, { method: "POST" }),
  parseDocuments: (
    ksId: number,
    body: { document_ids?: number[]; folders?: string[]; recursive?: boolean },
  ) => request<ParseBatchResponse>(
    `/api/knowledge/${ksId}/documents/parse-batch`,
    json(body),
  ),
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

  // External API access (owner-managed)
  listApiTokens: (ksId: number) => request<ApiToken[]>(`/api/knowledge/${ksId}/tokens`),
  createApiToken: (
    ksId: number,
    body: { name: string; scopes: ApiTokenScope[]; expires_in_days: number | null },
  ) => request<ApiTokenCreated>(`/api/knowledge/${ksId}/tokens`, json(body)),
  revealApiToken: (ksId: number, tokenId: number) =>
    request<ApiTokenRevealed>(`/api/knowledge/${ksId}/tokens/${tokenId}/reveal`, { method: "POST" }),
  revokeApiToken: (ksId: number, tokenId: number) =>
    request<ApiToken>(`/api/knowledge/${ksId}/tokens/${tokenId}`, { method: "DELETE" }),

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
  importRdf: (
    ksId: number,
    file: File,
    options: {
      target: RdfImportTarget
      strategy: RdfImportStrategy
      format: RdfImportFormat
      baseIri?: string
    },
  ) => {
    const fd = new FormData()
    fd.append("file", file)
    fd.append("target", options.target)
    fd.append("strategy", options.strategy)
    fd.append("format", options.format)
    if (options.baseIri?.trim()) fd.append("base_iri", options.baseIri.trim())
    return request<RdfImportResult>(`/api/knowledge/${ksId}/rdf/import`, { method: "POST", body: fd })
  },

  // Controlled terminology (SKOS vocabulary + human-reviewed agent proposals)
  getVocabulary: (ksId: number) => request<VocabularyView>(`/api/knowledge/${ksId}/vocabulary`),
  listVocabularySchemes: (ksId: number) =>
    request<VocabularySchemeList>(`/api/knowledge/${ksId}/vocabulary/schemes`),
  listVocabularyConcepts: (
    ksId: number,
    params: {
      scheme_iri?: string
      q?: string
      status?: "active" | "deprecated"
      mapping?: "mapped" | "standalone"
      origin?: "manual" | "extraction" | "agent"
      start_date?: string
      end_date?: string
      limit?: number
      offset?: number
    } = {},
  ) => {
    const qs = new URLSearchParams()
    if (params.scheme_iri) qs.set("scheme_iri", params.scheme_iri)
    if (params.q) qs.set("q", params.q)
    if (params.status) qs.set("status", params.status)
    if (params.mapping) qs.set("mapping", params.mapping)
    if (params.origin) qs.set("origin", params.origin)
    if (params.start_date) qs.set("start_date", params.start_date)
    if (params.end_date) qs.set("end_date", params.end_date)
    qs.set("limit", String(params.limit ?? 20))
    qs.set("offset", String(params.offset ?? 0))
    return request<VocabularyConceptList>(`/api/knowledge/${ksId}/vocabulary/concepts?${qs.toString()}`)
  },
  syncVocabulary: (ksId: number) => request<{
    scheme_iri: string | null
    terms_added: number
    terms_mapped: number
    aliases_added: number
    broader_added: number
    stale_mappings_removed: number
    mapping_conflicts: number
    view: VocabularyView
  }>(`/api/knowledge/${ksId}/vocabulary/sync`, { method: "POST" }),
  createVocabularyScheme: (
    ksId: number,
    body: { title: string; description: string; default_language: string },
  ) => request<VocabularyScheme>(`/api/knowledge/${ksId}/vocabulary/schemes`, json(body)),
  updateVocabularyScheme: (
    ksId: number,
    iri: string,
    body: { title: string; description: string; default_language: string },
  ) => request<VocabularyScheme>(
    `/api/knowledge/${ksId}/vocabulary/schemes?iri=${encodeURIComponent(iri)}`,
    patch(body),
  ),
  deleteVocabularyScheme: (ksId: number, iri: string) =>
    request<{ deleted: string; removed_triples: number }>(
      `/api/knowledge/${ksId}/vocabulary/schemes?iri=${encodeURIComponent(iri)}`,
      { method: "DELETE" },
    ),
  createVocabularyConcept: (ksId: number, body: VocabularyConceptInput) =>
    request<VocabularyConcept>(`/api/knowledge/${ksId}/vocabulary/concepts`, json(body)),
  updateVocabularyConcept: (ksId: number, iri: string, body: VocabularyConceptInput) =>
    request<VocabularyConcept>(
      `/api/knowledge/${ksId}/vocabulary/concepts?iri=${encodeURIComponent(iri)}`,
      patch(body),
    ),
  deleteVocabularyConcept: (ksId: number, iri: string) =>
    request<{ deleted: string; removed_triples: number }>(
      `/api/knowledge/${ksId}/vocabulary/concepts?iri=${encodeURIComponent(iri)}`,
      { method: "DELETE" },
    ),
  suggestVocabulary: (ksId: number, schemeIri: string) =>
    request<TermProposalList>(
      `/api/knowledge/${ksId}/vocabulary/suggest`,
      json({ scheme_iri: schemeIri }),
    ),
  listTermProposals: (
    ksId: number,
    params: { status?: string; q?: string; limit?: number; offset?: number } = {},
  ) => {
    const qs = new URLSearchParams()
    qs.set("status", params.status ?? "all")
    if (params.q) qs.set("q", params.q)
    qs.set("limit", String(params.limit ?? 100))
    qs.set("offset", String(params.offset ?? 0))
    return request<TermProposalList>(`/api/knowledge/${ksId}/vocabulary/proposals?${qs.toString()}`)
  },
  acceptTermProposal: (ksId: number, proposalId: number, payload?: Record<string, unknown>, note = "") =>
    request<{ proposal: TermProposal; concept: VocabularyConcept }>(
      `/api/knowledge/${ksId}/vocabulary/proposals/${proposalId}/accept`,
      json({ payload, note }),
    ),
  rejectTermProposal: (ksId: number, proposalId: number, note = "") =>
    request<TermProposal>(
      `/api/knowledge/${ksId}/vocabulary/proposals/${proposalId}/reject`,
      json({ note }),
    ),
  exportVocabulary: async (ksId: number, fmt = "turtle"): Promise<string> => {
    const res = await fetch(`/api/knowledge/${ksId}/vocabulary/export?fmt=${fmt}`, { credentials: "include" })
    if (!res.ok) throw new Error(`${res.status}: ${res.statusText}`)
    return res.text()
  },

  // Extraction (starts a background job; poll it for progress)
  runExtraction: (ksId: number, chunkIds: number[], model?: string, replaceExisting = true) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/extract`, json({
      chunk_ids: chunkIds, model, replace_existing: replaceExisting,
    })),
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

  // Per-knowledge-system model prompts
  listPrompts: (ksId: number) =>
    request<KnowledgePromptList>(`/api/knowledge/${ksId}/prompts`),
  updatePrompt: (ksId: number, promptKey: string, content: string) =>
    request<KnowledgePrompt>(
      `/api/knowledge/${ksId}/prompts/${encodeURIComponent(promptKey)}`,
      put({ content }),
    ),
  restorePrompt: (ksId: number, promptKey: string) =>
    request<KnowledgePrompt>(
      `/api/knowledge/${ksId}/prompts/${encodeURIComponent(promptKey)}`,
      { method: "DELETE" },
    ),
  restoreAllPrompts: (ksId: number) =>
    request<void>(`/api/knowledge/${ksId}/prompts/restore-all`, { method: "POST" }),

  // Manual editing
  editOntology: (ksId: number, op: EditOp) =>
    request<EditResult>(`/api/knowledge/${ksId}/ontology/edit`, json(op)),
  ontologyImpact: (ksId: number, iri: string, kind: "class" | "property") => {
    const qs = new URLSearchParams({ iri, kind })
    return request<OntologyImpactResponse>(`/api/knowledge/${ksId}/ontology/impact?${qs.toString()}`)
  },
  previewOntologyChanges: (ksId: number, operations: EditOp[], expectedRevision?: string) =>
    request<OntologyChangeSetResult>(
      `/api/knowledge/${ksId}/ontology/changes`,
      json({
        operations,
        expected_revision: expectedRevision,
        dry_run: true,
        include_rdf_diff: false,
      }),
    ),
  commitOntologyChanges: (
    ksId: number,
    operations: EditOp[],
    expectedRevision?: string,
    reason?: string,
  ) => request<OntologyChangeSetResult>(
    `/api/knowledge/${ksId}/ontology/changes`,
    json({
      operations,
      expected_revision: expectedRevision,
      reason: reason ?? "",
      confirm_destructive: operations.some((op) => op.op.startsWith("delete_") || op.op.startsWith("merge_")),
      include_rdf_diff: false,
    }),
  ),
  listAgentConversations: (ksId: number, deleted = false) =>
    request<AgentConversationList>(
      `/api/knowledge/${ksId}/agent/conversations${deleted ? "?deleted=true" : ""}`,
    ),
  createAgentConversation: (ksId: number, title?: string) =>
    request<AgentConversation>(
      `/api/knowledge/${ksId}/agent/conversations`,
      json(title?.trim() ? { title: title.trim() } : {}),
    ),
  getAgentConversation: (ksId: number, conversationId: number) =>
    request<AgentConversationDetail>(
      `/api/knowledge/${ksId}/agent/conversations/${conversationId}`,
    ),
  renameAgentConversation: (ksId: number, conversationId: number, title: string) =>
    request<AgentConversation>(
      `/api/knowledge/${ksId}/agent/conversations/${conversationId}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
      },
    ),
  deleteAgentConversation: (ksId: number, conversationId: number) =>
    request<{ deleted: boolean }>(
      `/api/knowledge/${ksId}/agent/conversations/${conversationId}`,
      { method: "DELETE" },
    ),
  restoreAgentConversation: (ksId: number, conversationId: number) =>
    request<AgentConversation>(
      `/api/knowledge/${ksId}/agent/conversations/${conversationId}/restore`,
      json({}),
    ),
  chatWithAgent: (ksId: number, body: AgentChatRequest) =>
    request<AgentResponse>(`/api/knowledge/${ksId}/agent/chat`, json(body)),
  chatWithAgentStream: (
    ksId: number,
    body: AgentChatRequest,
    onEvent: (event: AgentStreamEvent) => void,
    signal?: AbortSignal,
  ) => streamAgentChat(`/api/knowledge/${ksId}/agent/chat/stream`, body, onEvent, signal),
  suggestOntologyChanges: (ksId: number, instruction: string, expectedRevision?: string) =>
    request<OntologySuggestion>(
      `/api/knowledge/${ksId}/ontology/suggest`,
      json({ instruction, expected_revision: expectedRevision }),
    ),

  // Conflicts
  detectConflicts: (ksId: number) =>
    request<Conflict[]>(`/api/knowledge/${ksId}/conflicts/detect`, { method: "POST" }),
  listConflicts: (ksId: number, status = "open", ctype?: string) =>
    request<Conflict[]>(`/api/knowledge/${ksId}/conflicts?status=${status}${ctype ? `&ctype=${ctype}` : ""}`),
  getConflictContext: (ksId: number, cid: number) =>
    request<ConflictContext>(`/api/knowledge/${ksId}/conflicts/${cid}`),
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
  extractInstances: (ksId: number, chunkIds: number[], model?: string, replaceExisting = true) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/extract-instances`, json({
      chunk_ids: chunkIds, model, replace_existing: replaceExisting,
    })),
  // One-click schema + instances (TBox then ABox in a single job)
  extractAll: (ksId: number, chunkIds: number[], model?: string, replaceExisting = true) =>
    request<ExtractionJob>(`/api/knowledge/${ksId}/extract-all`, json({
      chunk_ids: chunkIds, model, replace_existing: replaceExisting,
    })),

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
  resolveQueueItem: (
    ksId: number,
    resId: number,
    decision: {
      action: "match" | "new" | "reject" | "defer"
      individual_iri?: string
      reason?: string
      review_after?: string
      expected_updated_at?: string
    },
  ) =>
    request<{ id: number; status: string; individual_iri: string | null; summary: string; idempotent: boolean }>(
      `/api/knowledge/${ksId}/resolution/${resId}/resolve`,
      json(decision),
    ),
  mergeIndividuals: (
    ksId: number,
    sourceIri: string,
    canonicalIri: string,
    reason: string,
    resolutionId?: number,
    expectedUpdatedAt?: string,
  ) =>
    request<{ source_iri: string; canonical_iri: string; idempotent: boolean }>(
      `/api/knowledge/${ksId}/resolution/merge`,
      json({
        source_iri: sourceIri,
        canonical_iri: canonicalIri,
        reason,
        resolution_id: resolutionId,
        expected_updated_at: expectedUpdatedAt,
      }),
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
