// Types mirroring the OntoPilot backend responses.

export type Role = "owner" | "editor" | "viewer"

export interface User {
  id: number
  username: string
  display_name: string | null // optional nickname; falls back to username in the UI
  is_admin: boolean
  active: boolean
}

export interface Member {
  user_id: number
  username: string
  role: Role
}

export interface GrantableUser {
  id: number
  username: string
  is_admin: boolean
}

export interface MemberAccess {
  ks_id: number
  ks_name: string
  role: Role
}

export interface MemberActivity {
  ks_name: string
  action: string
  summary: string
  created_at: string
}

export interface MemberDetail {
  user: { id: number; username: string; is_admin: boolean; active: boolean }
  access: MemberAccess[]
  activity: MemberActivity[]
}

export interface AuditEvent {
  id: number
  actor_name: string
  action: string
  summary: string
  detail: Record<string, unknown>
  created_at: string
  can_rollback: boolean
}

export interface HistoryResponse {
  items: AuditEvent[]
  total: number
}

export interface DocumentMeta {
  id: number
  knowledge_system_id: number | null
  sha256: string
  original_filename: string
  folder: string
  ext: string
  mime: string | null
  size_bytes: number
  storage_path: string
  uploaded_at: string
  parse_status: "pending" | "parsed" | "failed"
  parser_backend: string | null
  parse_error: string | null
  text_char_count: number | null
  chunk_count: number
  tbox_extracted_at: string | null
  abox_extracted_at: string | null
}

export interface DocumentContribution {
  chunk_count: number
  axiom_count: number
  individual_count: number
}

export interface Chunk {
  id: number
  document_id: number
  idx: number
  text: string
  char_start: number
  char_end: number
  token_estimate: number
  created_at: string
}

export interface KnowledgeSystem {
  id: number
  name: string
  description: string
  owner_id: number | null
  graph_iri: string
  base_iri: string
  created_at: string
  updated_at: string
  class_count: number
  property_count: number
  axiom_count: number
  llm_model: string | null // per-KS model override; null -> system/.env default
  llm_provider_id: number | null
  embedding_provider_id: number | null
  embedding_model: string | null
  my_role: Role
}

export interface Provider {
  id: number
  name: string
  kind: "llm" | "embedding"
  base_url: string
  model: string
  has_api_key: boolean
  api_key_hint: string // masked, never the raw key
  last_test_ok: boolean | null // persisted result of the most recent connection test
  last_tested_at: string | null
}

export interface SystemSettings {
  llm_provider_id: number | null // default LLM model entry
  embedding_provider_id: number | null // default embedding model entry
  available_models: string[]
  temperature: number // read-only (.env-managed)
}

export interface TestResult {
  ok: boolean
  message: string
  latency_ms: number
}

export interface ModelCatalog {
  models: string[]
  default: string
}

export interface OntologyClass {
  iri: string
  local: string
  label: string
  comment: string
  superclasses: string[]
}

export interface OntologyProperty {
  iri: string
  local: string
  label: string
  comment: string
  domain: string | null
  domain_label: string | null
  range: string | null
  range_label: string | null
}

export interface OntologyView {
  classes: OntologyClass[]
  object_properties: OntologyProperty[]
  data_properties: OntologyProperty[]
  axioms: {
    subclass_of: { sub: string; super: string }[]
    disjoint_with: { a: string; b: string }[]
    equivalent_class: { a: string; b: string }[]
  }
  labels: Record<string, string>
  stats: { class_count: number; property_count: number; axiom_count: number }
  knowledge_system?: { id: number; name: string; base_iri: string }
}

export interface ExtractionJob {
  id: number
  knowledge_system_id: number
  kind: "tbox" | "abox" | "both"
  status: "pending" | "running" | "completed" | "failed"
  model: string
  chunk_ids: number[]
  created_at: string
  finished_at: string | null
  log: string
  error: string | null
  classes_added: number
  properties_added: number
  axioms_added: number
  total_chunks: number
  processed_chunks: number
  individuals_added: number
  assertions_added: number
  pending_added: number
  unknown_classes: Record<string, number>
}

export interface ExtractionResult {
  job: ExtractionJob
  classes_added: number
  properties_added: number
  axioms_added: number
  stats: { class_count: number; property_count: number; axiom_count: number }
  per_chunk: { chunk_id: number; status: string; axioms: number; error: string | null }[]
}

export interface ImpactAxiom {
  axiom_key: string
  description: string
}

export interface ImpactSystem {
  knowledge_system_id: number
  knowledge_system_name: string
  axioms: ImpactAxiom[]
}

export interface DocumentImpact {
  document_id: number
  systems: ImpactSystem[]
}

export interface RetractGroup {
  knowledge_system_id: number
  axiom_keys: string[]
}

export interface SourceDoc {
  document_id: number
  filename: string
  folder: string | null
  exists: boolean
  chunk_count: number
  axiom_count: number
}

export interface ParseResponse {
  document_id: number
  parse_status: string
  parser_backend: string | null
  text_char_count: number | null
  chunk_count: number
  error: string | null
}

export interface ConflictEntity {
  iri: string
  label: string
}

export interface ConflictResolution {
  id: string
  label: string
  op: EditOp
}

export interface Conflict {
  id: number
  knowledge_system_id: number
  signature: string
  ctype: string
  severity: "error" | "warning"
  status: "open" | "resolved" | "dismissed"
  title: string
  detail: string
  payload: {
    entities: ConflictEntity[]
    resolutions: ConflictResolution[]
    recommendation?: { resolution_id: string; reason: string; confidence: number }
  }
  created_at: string
  resolved_at: string | null
  resolution: string | null
}

// An ontology edit operation. `op` selects the operation; the rest are its params.
export type EditOp = { op: string; [k: string]: unknown }

export interface EditResult {
  result: string
  view: OntologyView
  open_conflicts: Conflict[]
}

export interface ResolveResult {
  resolved: number
  open_conflicts: Conflict[]
  view: OntologyView
}

// ---- ABox (instances) ----
export interface AboxClass {
  iri: string
  label: string
  count: number
}

export interface AboxClassList {
  classes: AboxClass[]
  total: number
}

export interface TypeRef {
  iri: string
  label: string
}

export interface IndividualSummary {
  iri: string
  label: string
  types: TypeRef[]
}

export interface IndividualList {
  items: IndividualSummary[]
  total: number
}

/** Where an ABox fact came from: the source chunk (→ document) + a text snippet. */
export interface AboxSource {
  chunk_id: number
  document_id: number | null
  document: string | null
  snippet: string
}

export interface ObjectAssertion {
  prop: string
  prop_label: string
  target: string
  target_label: string
  sources?: AboxSource[]
}

export interface DataAssertion {
  prop: string
  prop_label: string
  value: string
  datatype: string | null
  sources?: AboxSource[]
}

export interface Individual {
  iri: string
  label: string
  types: TypeRef[]
  object_assertions: ObjectAssertion[]
  data_assertions: DataAssertion[]
  sources?: AboxSource[]
}

export interface AssertionInput {
  subject: string
  prop: string
  kind: "object" | "data"
  target?: string
  value?: string
  datatype?: string | null
}

// ---- Entity resolution ----
export interface ResolutionCandidate {
  iri: string
  label: string
  score: number
}

export interface ResolutionQueueItem {
  id: number
  surface_form: string
  class_iri: string | null
  class_label: string | null
  confidence: number | null
  candidates: ResolutionCandidate[]
  source_chunk_id: number | null
  created_at: string
}

export interface ResolutionDecision {
  id: number
  surface_form: string
  class_label: string | null
  status: "matched" | "new" | "distinct"
  individual_iri: string | null
  individual_label: string | null
  confidence: number | null
  reason: string | null
  resolved_by: string | null
  created_at: string
  resolved_at: string | null
}

export interface ResolutionQueue {
  items: ResolutionQueueItem[]
  total: number
}

export interface ResolutionDecisions {
  items: ResolutionDecision[]
  total: number
}

export interface Reconciliation {
  id: number
  slot: string // "domain" | "range"
  property_label: string
  property_iri: string | null
  candidates: string[]
  choice: string // "union" | "common_super" | "keep"
  chosen_label: string | null
  reason: string | null
  resolved_by: string | null
  created_at: string
}

export interface ReconciliationList {
  items: Reconciliation[]
  total: number
}

export interface ValidationDecision {
  id: number
  property_label: string
  property_iri: string | null
  xsd_type: string | null
  action: string // "relax" | "remove"
  reason: string | null
  resolved_by: string | null
  created_at: string
}

export interface ValidationDecisionList {
  items: ValidationDecision[]
  total: number
}

export interface ReviewCounts {
  conflicts: number
  resolution: number
  validation: number
  total: number
}

// ---- ABox validation ----
export interface ValidationFix {
  id: string
  label: string
  op: Record<string, unknown>
}

export interface Violation {
  id: string
  type: "disjoint" | "domain" | "range" | "datatype"
  severity: "error" | "warning"
  individual: { iri: string; label: string }
  summary: string
  fixes: ValidationFix[]
}

export interface ValidationResult {
  violations: Violation[]
  counts: { error: number; warning: number }
  truncated: boolean
}
