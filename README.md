<div align="center">

<img src="assets/banner.webp" alt="Utopia" width="820">

</div>

# Utopia

<div align="center">

[Philosophy](#philosophy) · [Quick start](#quick-start) · [Features](#features) · [Roadmap](#roadmap)

[![Stars](https://img.shields.io/github/stars/deeplethe/utopia?style=flat-square&label=STARS&labelColor=161B22&color=FFC220&logo=github&logoColor=FFFFFF)](https://github.com/deeplethe/utopia/stargazers)
[![License](https://img.shields.io/badge/LICENSE-APACHE%202.0-3FB950?style=flat-square&labelColor=161B22)](LICENSE)
[![Rust](https://img.shields.io/badge/BUILT%20WITH-RUST-F74C00?style=flat-square&labelColor=161B22&logo=rust&logoColor=FFFFFF)](https://www.rust-lang.org)

[![Official site](https://img.shields.io/badge/OFFICIAL-UTOPIA.BI-FFFFFF?style=flat-square&labelColor=161B22&logo=safari&logoColor=FFFFFF)](https://utopia.bi)
[![Container](https://img.shields.io/badge/GHCR-DEEPLETHE%2FUTOPIA-2496ED?style=flat-square&labelColor=161B22&logo=docker&logoColor=FFFFFF)](https://github.com/deeplethe/utopia/pkgs/container/utopia)
[![Discussions](https://img.shields.io/badge/DISCUSSIONS-8957E5?style=flat-square&labelColor=161B22&logo=data:image/svg%2Bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxNiIgaGVpZ2h0PSIxNiIgZmlsbD0iI0ZGRkZGRiIgY2xhc3M9ImJpIGJpLWNoYXQtZG90cy1maWxsIiB2aWV3Qm94PSIwIDAgMTYgMTYiPgogIDxwYXRoIGQ9Ik0xNiA4YzAgMy44NjYtMy41ODIgNy04IDdhOSA5IDAgMCAxLTIuMzQ3LS4zMDZjLS41ODQuMjk2LTEuOTI1Ljg2NC00LjE4MSAxLjIzNC0uMi4wMzItLjM1Mi0uMTc2LS4yNzMtLjM2Mi4zNTQtLjgzNi42NzQtMS45NS43Ny0yLjk2NkMuNzQ0IDExLjM3IDAgOS43NiAwIDhjMC0zLjg2NiAzLjU4Mi03IDgtN3M4IDMuMTM0IDggN001IDhhMSAxIDAgMSAwLTIgMCAxIDEgMCAwIDAgMiAwbTQgMGExIDEgMCAxIDAtMiAwIDEgMSAwIDAgMCAyIDBtMyAxYTEgMSAwIDEgMCAwLTIgMSAxIDAgMCAwIDAgMiIvPgo8L3N2Zz4%3D)](https://github.com/deeplethe/utopia/discussions)
[![Built by DeepLethe](https://img.shields.io/badge/BUILT%20BY-DEEPLETHE-2D333B?style=flat-square&labelColor=161B22)](https://github.com/deeplethe)
[![中文](https://img.shields.io/badge/LANG-%E4%B8%AD%E6%96%87-DA3633?style=flat-square&labelColor=161B22)](README.zh-CN.md)

</div>

**The enterprise world model built by [DeepLethe](https://deeplethe.com).** It's the world's first substrate for knowledge engineering that learns passively and governs itself — it puts time and ontology in the base layer, revises that ontology as new material arrives, remembers how its own understanding changed, settles conflicts by axiom, and replays the world as it was understood at any past moment. Run it on your corporate network, on a cloud server, or on the laptop in front of you — as a company's knowledge foundation, as the decision core your agents can trust, or as a chronicle of your own.

---

<!-- Video: drop an mp4 into any issue/PR comment box, GitHub returns a
     https://github.com/user-attachments/assets/xxx link,
     paste that link on its own line here and it renders as a player. -->

<div align="center">

https://github.com/user-attachments/assets/PLACEHOLDER

</div>

---

## Philosophy

We gave it a somewhat romantic name — **Utopia**. Ptolemy's geocentric model was long held to be a reasonable account of cosmic order, then revised and displaced step by step by Copernicus, Kepler, Galileo and Newton. Notice that what we keep is not merely "ah, heliocentrism is the right one"; it is the whole arc along which understanding moved, a history in full. This is one of the things that set Utopia apart from a conventional knowledge graph: facts are organised into a replayable chain of revisions — engineered as a **bitemporal knowledge graph**. Review an action later and the full course and grounds of the decision come back with it. To make it hold up in practice, we have iterated at length against public corpora spanning enterprise records, education, finance, law and research. Temporality is only one facet — for how knowledge is taken in, how the future is reasoned about, and how logic bounds action, see [utopia.bi/philosophy](https://utopia.bi/philosophy).

## Features

The whole system is a Rust binary and a Postgres service. By bringing in pgvector and a queue-table design, we cut the weight of the stack and its service dependencies — deployment is over in a blink.

| | |
|---|---|
| **Knowledge ingest** | PDF, DOCX, PPTX, XLSX/XLS/ODS, CSV/TSV, Markdown, HTML and plain text, with encoding detection for Chinese sources. Web pages and RSS sync on a cron; documents can also be pushed from anywhere with a per-source ingest token. Failed parses reprocess in place without re-uploading; a whole source or a whole knowledge base can be re-extracted in bulk. |
| **Search and chat** | Hybrid retrieval over Tantivy full-text and pgvector, fused with RRF; Chinese full-text uses jieba tokenisation. Answers stream with inline citations that jump straight to the source passage. Any OpenAI-compatible endpoint works — DeepSeek, Qwen, GLM, Ollama, vLLM — so the whole system can run on an isolated network. |
| **Ontology and cold start** | Every knowledge base ships with a built-in ontology (person, organization, project, product, event, concept, location, and the relations among them), so ingest can begin without designing a model first. Types and predicates encountered outside the ontology are recorded and counted; frequent ones can be proposed by the model and merged in on confirmation. The ontology grows with the corpus, instead of asking you to define the world completely on day one. |
| **Bitemporal graph** | LLM extraction against the editable ontology produces entities and facts. Every fact carries a validity interval and its evidence rows. Correcting a fact does not overwrite the old version: it closes the old one and links the new one to it. Graph and neighbourhood queries can be read back at any point in history. The entity panel shows two timelines at once: when something held in the world, and when the system formed — then changed — that judgement. |
| **Entity resolution and review** | Three-stage entity resolution; every merge is logged and can be undone. Low-confidence extractions, merge candidates and cardinality conflicts go to a review queue rather than interrupting anyone. Confirming, rejecting or closing a fact by hand leaves a record. |
| **Reasoning and derivation** | Rules are expressed in temporal Datalog and driven by forward chaining. Derived facts carry validity time and provenance just as extracted ones do, and their full derivation path can be expanded — every conclusion can be asked "why", all the way back to the original text. Ontology axioms (type inheritance, relation hierarchy, transitivity, symmetry, inverses, disjointness, cardinality) compile into rules and take part in reasoning; constraint violations go to the conflict queue. |
| **Ontology-driven querying** | Register a Postgres connection once at the system level, mount it on a knowledge base, and chat can query documents and the database together. The method behind it ([Ontology2SQL](https://github.com/deeplethe/ontology2sql)) scores 70.20 on SQLite and 65.80 on PostgreSQL on BIRD Mini-Dev — state of the art on both, ahead of second place by 12.2 and 9.0 ([leaderboard submission](https://github.com/bird-bench/bird-bench.github.io/pull/218)). |
| **Multi-user and permissions** | Permissions are scoped per knowledge base, each with its own members and roles. Open bases are readable by everyone in the deployment; restricted ones only by invited users. A public space readable by all (General Knowledge Base) is created on deployment. |
| **Decision ledger** | Confirming and rejecting facts, merging and reverting entities, rebuilding the graph — all leave a record with the operator, the time, and a snapshot of the object as it then stood. The record remains queryable after the object is invalidated or rebuilt. |

## Quick start

Requirements: Docker (local development also needs Rust 1.85+, Node 20+, pnpm).

Start from the prebuilt image:

```bash
git clone https://github.com/deeplethe/utopia.git
cd utopia
docker compose --profile app up -d
```

Open http://localhost:1516 and register — the first account automatically becomes the administrator, and a public knowledge base readable by everyone is created at the same time. Before ingesting documents, configure the model endpoints (chat and embedding) under system settings.

Or build from source:

```bash
docker compose -f docker-compose.yml -f docker-compose.build.yml --profile app up -d --build
```

### Local development

```bash
# 1. Postgres with pgvector
docker compose up -d db

# 2. Backend on :1516 — runs migrations on startup
cargo run -p utopia-server

# 3. Frontend on :5173, proxying /api to the backend
cd web && pnpm install && pnpm dev
```

## Roadmap

- [ ] **Simulation engine**: scenario overlays that never touch the ledger, computing both the diff and the constraints it would violate
- [ ] **Execution gate**: every downstream call an agent makes passes ontology rules and symbolic logic first; what fails does not land
- [ ] **Lakehouse**: Iceberg / Delta Lake, plus Databricks, Snowflake and MaxCompute
- [ ] **More sources**: MySQL, ClickHouse and Doris drivers; S3, WebDAV, Notion and Feishu connectors
- [ ] **Agent memory over MCP**: episode writes, the retrieve endpoint, and the MCP server
- [ ] **Enterprise**: OIDC SSO, backup and restore commands, benchmarks at 100k documents

## Status

Utopia is still at **v0.1**. The database schema evolves between versions and migrations only roll forward, with no rollback — pin a specific version with `UTOPIA_IMAGE` in production, and back up the database along with the `data` directory before upgrading.

Please read [SECURITY.md](SECURITY.md) before exposing it to the public internet.

## Community

- 💬 [Discussions](https://github.com/deeplethe/utopia/discussions) — ask questions, talk design, tell us what you built with it
- 🐛 [Issues](https://github.com/deeplethe/utopia/issues) — report bugs, request features
- 🔌 [Ontology2SQL](https://github.com/deeplethe/ontology2sql) — ontology-driven text-to-SQL, the method behind ontology-driven querying

## License

[Apache-2.0](LICENSE)
