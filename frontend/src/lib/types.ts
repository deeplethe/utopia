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

export interface DocumentListResponse {
  items: DocumentMeta[]
  total: number
  folders: string[]
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
  public_id: string
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

export type ApiTokenScope = "ontology:read" | "vocabulary:read" | "instances:read" | "query:read" | "provenance:read"

export interface ApiToken {
  id: number
  name: string
  token_prefix: string
  scopes: ApiTokenScope[]
  status: "active" | "expired" | "revoked"
  created_at: string
  expires_at: string | null
  last_used_at: string | null
  revoked_at: string | null
  can_reveal: boolean
}

export interface ApiTokenCreated extends ApiToken {
  token: string
}

export interface ApiTokenRevealed {
  token: string
}

export interface Provider {
  id: number
  name: string
  kind: "llm" | "embedding"
  base_url: string
  model: string
  concurrency_limit: number
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
  domain_members?: string[]
  range: string | null
  range_label: string | null
  range_members?: string[]
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

// ---- Controlled terminology (SKOS) ----
export interface VocabularyLabel {
  value: string
  language: string
}

export type VocabularyOrigin = "manual" | "extraction" | "agent"

export interface VocabularyScheme {
  iri: string
  title: string
  titles: VocabularyLabel[]
  description: string
  descriptions: VocabularyLabel[]
  default_language: string
  origin: VocabularyOrigin
  created_at: string
  modified_at: string
  concept_count: number
}

export interface VocabularyConcept {
  iri: string
  scheme_iri: string
  pref_labels: VocabularyLabel[]
  alt_labels: VocabularyLabel[]
  hidden_labels: VocabularyLabel[]
  display_label: string
  description: string
  notation: string
  broader: string[]
  broader_labels: string[]
  related: string[]
  related_labels: string[]
  mapped_entity_iri: string | null
  status: "active" | "deprecated"
  origin: VocabularyOrigin
  created_at: string
  modified_at: string
}

export interface VocabularyStats {
  scheme_count: number
  concept_count: number
  label_count: number
  mapped_count: number
  unmapped_count: number
}

export interface VocabularyView {
  schemes: VocabularyScheme[]
  concepts: VocabularyConcept[]
  stats: VocabularyStats
}

export interface VocabularySchemeList {
  items: VocabularyScheme[]
  total: number
  stats: VocabularyStats
}

export interface VocabularyConceptList {
  items: VocabularyConcept[]
  total: number
}

export interface VocabularyConceptInput {
  scheme_iri: string
  pref_labels: VocabularyLabel[]
  alt_labels: VocabularyLabel[]
  hidden_labels: VocabularyLabel[]
  description: string
  notation: string
  broader: string[]
  related: string[]
  mapped_entity_iri: string | null
  status: "active" | "deprecated"
}

export interface TermProposalEvidence {
  chunk_id: number
  document_id: number | null
  document: string | null
  snippet: string
}

export interface TermProposal {
  id: number
  action: "create" | "add_alias" | "update"
  term: string
  target_iri: string | null
  target_label: string | null
  status: "pending" | "accepted" | "rejected"
  payload: Record<string, unknown>
  confidence: number | null
  reason: string | null
  evidence: TermProposalEvidence[]
  source_chunk_ids: number[]
  extraction_job_id: number | null
  proposed_by: string
  resolved_by: string | null
  resolution_note: string | null
  created_at: string
  resolved_at: string | null
}

export interface TermProposalList {
  items: TermProposal[]
  total: number
}

export type RdfImportTarget = "auto" | "tbox" | "abox"
export type RdfImportStrategy = "merge" | "replace"
export type RdfImportFormat = "auto" | "turtle" | "rdfxml" | "ntriples" | "jsonld"

export interface RdfImportResult {
  filename: string
  sha256: string
  format: Exclude<RdfImportFormat, "auto">
  target: RdfImportTarget
  strategy: RdfImportStrategy
  base_iri: string
  parsed_triples: number
  tbox_triples: number
  abox_triples: number
  tbox_added: number
  tbox_removed: number
  abox_added: number
  abox_removed: number
  view: OntologyView
  open_conflicts: Conflict[]
  validation: {
    counts: { error: number; warning: number }
    truncated: boolean
  }
  terminology: {
    terms_added: number
    terms_mapped: number
    terminology_error: string | null
  }
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
  phase: "" | "tbox" | "role_recovery" | "hierarchy" | "reconciling" | "conflicts" |
    "structure" | "abox" | "terminology" | "finalizing" | "completed" | "failed"
  terms_added: number
  terms_mapped: number
  terminology_proposals: number
  terminology_error: string | null
}

export interface ExtractionResult {
  job: ExtractionJob
  classes_added: number
  properties_added: number
  axioms_added: number
  stats: { class_count: number; property_count: number; axiom_count: number }
  per_chunk: { chunk_id: number; status: string; axioms: number; error: string | null }[]
}

// ---- Immutable releases and streaming exports ----
export type ReleaseStatus = "draft" | "reviewed" | "published" | "deleted"
export type ReleaseLayer = "tbox" | "vocabulary" | "abox" | "bundle"

export interface ReleaseDeployment {
  id: number
  status: "provisioning" | "active" | "stopping" | "stopped" | "failed"
  statement_count: number
  provenance_count: number
  error: string | null
  activated_at: string | null
  stopped_at: string | null
}

export interface ArtifactFile {
  name: string
  layer?: string
  statements?: number
  records?: number
  bytes: number
  sha256: string
}

export interface ReleaseQualityGate {
  open_conflict_errors: number
  unresolved_entities: number
  pending_terminology: number
  validation_errors: number
  blocking: number
}

export interface ReleaseManifest {
  capture_status: "pending" | "running" | "ready" | "failed" | "deleted"
  error?: string
  compression?: "none"
  quality_gate?: ReleaseQualityGate
  layers?: Record<Exclude<ReleaseLayer, "bundle">, {
    graph_iri: string
    statements: number
    files: ArtifactFile[]
  }>
  provenance?: ArtifactFile[]
}

export interface OntologyRelease {
  id: number
  knowledge_system_id: number
  version: string
  status: ReleaseStatus
  title: string
  notes: string
  manifest: ReleaseManifest
  created_by: string
  reviewed_by: string | null
  published_by: string | null
  created_at: string
  reviewed_at: string | null
  published_at: string | null
  deployment: ReleaseDeployment | null
  service_url: string | null
}

export interface ReleaseList {
  items: OntologyRelease[]
  total: number
}

export interface ReleaseDiffLayer {
  added: number
  removed: number
  added_sample: string[]
  removed_sample: string[]
}

export interface ReleaseDiff {
  from: { id: number; version: string }
  to: { id: number; version: string }
  layers: Record<Exclude<ReleaseLayer, "bundle">, ReleaseDiffLayer>
}

export interface ExportJob {
  id: number
  knowledge_system_id: number
  release_id: number | null
  layer: ReleaseLayer
  format: "nquads"
  status: "pending" | "running" | "completed" | "failed"
  shard_size: number
  processed_statements: number
  total_statements: number
  files: ArtifactFile[]
  error: string | null
  created_by: string
  created_at: string
  started_at: string | null
  finished_at: string | null
}

export interface ExportList {
  items: ExportJob[]
  total: number
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

export interface ParseBatchResponse {
  items: ParseResponse[]
  total: number
  parsed: number
  failed: number
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

export interface ConflictEvidenceSource {
  chunk_id: number
  chunk_index: number
  document_id: number | null
  document: string | null
  folder: string | null
  job_id: number | null
  snippet: string
}

export interface ConflictEvidenceAxiom {
  axiom_key: string
  description: string
  source_count: number
  sources: ConflictEvidenceSource[]
}

export interface ConflictContext {
  conflict: Conflict
  evidence: ConflictEvidenceAxiom[]
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
  chunk_id: number | null
  document_id: number | null
  document: string | null
  snippet: string
  job_id?: number | null
  model?: string | null
  prompt_snapshot?: Record<string, unknown> | null
  method?: "extraction" | "manual" | "review"
  actor?: string | null
  review?: Record<string, unknown> | null
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
  individual_deleted: boolean
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
  terminology: number
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
  type: "placeholder" | "type_count" | "role" | "disjoint" | "domain" | "range" | "datatype"
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

// ---- Per-knowledge-system prompts ----
export type PromptCategory = "extraction" | "review" | "governance" | "validation"

export interface KnowledgePrompt {
  key: string
  category: PromptCategory
  title: string
  description: string
  default_content: string
  effective_content: string
  variables: string[]
  is_overridden: boolean
  updated_at: string | null
  updated_by: string | null
}

export interface KnowledgePromptList {
  items: KnowledgePrompt[]
  total_overrides: number
}
