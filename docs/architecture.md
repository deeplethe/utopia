# OntoPilot Architecture

## System Context

```mermaid
flowchart TB
    HUMAN["Ontology engineer / domain reviewer"] --> UI["React workspace"]
    CLIENT["Downstream application"] --> EXT["Scoped external API"]
    UI --> API["FastAPI governance API"]
    EXT --> API
    API --> PG["PostgreSQL"]
    API --> OXI["Oxigraph"]
    API --> ART["Document and artifact storage"]
    API --> LLM["OpenAI-compatible LLM"]
    API --> EMB["Embedding endpoint"]
```

## Storage Boundaries

| Store | Responsibility |
| --- | --- |
| PostgreSQL | Users, roles, documents, chunks, jobs, prompt snapshots, review queues, provenance, audit events, releases, and export jobs |
| Oxigraph | Mutable RDF named graphs for TBox, SKOS terminology, and ABox |
| Serving Oxigraph | Read-only release projections in dedicated version-scoped named graphs |
| Artifact storage | Source blobs, immutable release snapshots, manifests, provenance JSONL, and export shards |

SQLite remains a local-development fallback. It is not the recommended shared-deployment database.

## Knowledge-System Graphs

```mermaid
flowchart LR
    DOC["Document chunks"] --> PROPOSE["Candidate extractors"]
    PROPOSE --> CRITIC["Independent role critics"]
    CRITIC -->|"reusable concepts"| TBOX["TBox named graph"]
    CRITIC -->|"concrete identities"| RESOLVE["Entity resolution"]
    RESOLVE --> ABOX["ABox named graph"]
    TBOX --> SYNC["Deterministic SKOS synchronization"]
    SYNC --> TERMS["Vocabulary named graph"]
    CRITIC -->|"uncertain"| REVIEW["Human review queues"]
    REVIEW --> TBOX
    REVIEW --> TERMS
    REVIEW --> ABOX
```

The layers are intentionally separate:

- TBox is a reusable conceptual schema.
- SKOS terminology governs lexical forms and mappings.
- ABox contains identities and assertions.

## Extraction and Provenance

```mermaid
sequenceDiagram
    participant U as User
    participant API as FastAPI
    participant J as Extraction job
    participant M as Model endpoint
    participant G as Oxigraph
    participant P as PostgreSQL

    U->>API: Select chunks and start extraction
    API->>P: Freeze model and effective prompt snapshot
    API-->>U: Job ID
    J->>M: Grounded chunk + ontology context
    M-->>J: Candidate TBox/ABox delta
    J->>M: Independent role verification
    J->>G: Merge accepted statements
    J->>P: Statement → chunk/job provenance
    J->>P: Review queues and audit event
```

The prompt snapshot stores exact contents and SHA-256 hashes. Editing a project prompt affects future jobs only.

## Release State Machine

```mermaid
stateDiagram-v2
    [*] --> Draft: Capture immutable snapshot
    Draft --> Reviewed: Quality gate passes
    Reviewed --> Published: Authorized publish
    Draft --> Restored: Restore snapshot
    Reviewed --> Restored: Restore snapshot
    Published --> Restored: Restore snapshot
```

The quality gate blocks review while unresolved error conflicts, entity-resolution items, terminology proposals, or ABox validation errors remain.

## Export Design

ABox export never materializes the complete graph in Python memory. Oxigraph quads are streamed into fixed-statement-count `.nq` shards. Each shard is uncompressed and independently checksummed.

```mermaid
flowchart LR
    OXI["Oxigraph quad iterator"] --> WRITER["Constant-memory shard writer"]
    WRITER --> NQ1["abox-00001.nq"]
    WRITER --> NQ2["abox-00002.nq"]
    WRITER --> NQN["abox-xxxxx.nq"]
    NQ1 --> MANIFEST["manifest.json + SHA-256"]
    NQ2 --> MANIFEST
    NQN --> MANIFEST
```

Uncompressed shards support line-oriented processing, HTTP range requests, CDN/object-storage replication, and independent retry. A reverse proxy may apply transport compression without changing the artifact format.

## Published Service Boundary

Publishing verifies the immutable artifacts, streams them into a separate serving Oxigraph database, and indexes the captured provenance by release and statement key in PostgreSQL. Public fixed-version REST and SPARQL routes only use those projections. Deployment state is independent from release state, so a service may be stopped and rebuilt without changing the release. Terminal release deletion clears the projection and artifacts but retains a tombstone and audit evidence.

## Trust Boundaries

- Browser sessions and machine API tokens are separate credentials.
- External SPARQL is read-only and bounded.
- Per-knowledge-system roles gate governance operations.
- Model endpoints receive selected chunks and bounded ontology context, not unrestricted filesystem or database access.
- Graph mutations produce audit events; release artifacts are immutable after capture.
