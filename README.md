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

**The enterprise world model built by [DeepLethe](https://deeplethe.com).** It is the first open substrate for knowledge engineering that learns passively and governs itself. Where a knowledge graph or a vector store works to hold present knowledge, Utopia puts time awareness and ontology in the base layer: the knowledge system evolves as material arrives, and conflict detection, reasoning and decision making all run against that ontology. It deploys offline, so a company can stand up a knowledge foundation, a decision core its agents can trust, and a compliance audit trail on hardware it controls.

> Please note: we would rather this project were not framed as an open-source take on Palantir. It is **a different route to enterprise intelligence, built bottom up from knowledge governance to trustworthy decisions and simulation**.

---

<!-- Video: drop an mp4 into any issue/PR comment box, GitHub returns a
     https://github.com/user-attachments/assets/xxx link,
     paste that link on its own line here and it renders as a player. -->

<div align="center">

https://github.com/user-attachments/assets/aa226443-75de-437e-bd80-88e592ed8457

</div>

---

## Philosophy

We gave it a somewhat romantic name, **Utopia**. Ptolemy's geocentric model was taken for truth for a very long time, then falsified step by step by Copernicus, Kepler, Galileo and Newton. Looking back, what we keep is not only that heliocentrism turned out to be right; it is how that history unfolded.

Where existing vector stores and knowledge graphs work to get present knowledge right, one of Utopia's founding aims is to record the whole course of changing understanding. Engineered, that becomes a **bitemporal knowledge graph**. When a decision is reviewed later, the system can produce the full course it took and the grounds it rested on. To make this hold up in practice we have iterated at length against public corpora spanning enterprise records, education, finance, law and research. Temporality is only one facet; for how knowledge is taken in, how the future is reasoned about, and how logic bounds action, see [utopia.bi/philosophy](https://utopia.bi/philosophy).

## Features

One Rust binary and one Postgres. Full-text search is embedded in the binary, vectors go in pgvector, and the job queue is a table: nothing else to run.

| | |
|---|---|
| **A complete application** | A system console, a graph browser and an ontology workbench in one web UI. A product, not a library: install it and it works. |
| **Knowledge ingest** | Upload PDF, DOCX, PPTX, XLSX, XLS, ODS, CSV, TSV, Markdown, HTML or plain text, with legacy encodings detected on the way in. Web pages, RSS, GitHub, Jira and S3-compatible buckets sync on a schedule; everything else comes in through the API. |
| **Search and chat** | Full-text on Tantivy, vectors on pgvector, fused with RRF. Answers stream with inline citations that open the passage they came from. Any OpenAI-compatible endpoint works (DeepSeek, Qwen, GLM, Ollama, vLLM), so the whole system can run air-gapped. |
| **Agent harness and agentic RAG** | The whole system can be driven through conversation. The built-in agent searches documents, walks the graph (an entity's facts as of any date, or what changed in a period) and queries a mounted database. The same read-only tools are exposed over MCP. |
| **Ontology and cold start** | A new knowledge base has no vocabulary of its own; it starts from the packs you pick at creation. Five ship inside the binary: schema.org, W3C Org, PROV-O, FOAF and IOF Core ([ask for your industry](https://github.com/deeplethe/utopia/issues/new?labels=enhancement&title=Ontology%20pack%20request)). Terms outside the packs are counted as they appear; confirm the common ones and they join the ontology. |
| **Bitemporal graph** | Extraction turns documents into entities and facts, following an ontology you can edit. Every fact carries when it held and where it came from. Correcting a fact closes the old version and links the new one to it rather than overwriting, so the graph keeps two timelines: when something was true in the world, and when the system came to believe it. |
| **Entity resolution and review** | Duplicates are resolved in three stages: exact name or alias, embedding similarity, then a model's call on the doubtful pairs. Every merge can be undone. Uncertain cases go to a review queue: low-confidence extractions, suspected duplicates and cardinality conflicts. |
| **Reasoning and derivation** | Ontology axioms compile into rules: transitivity, symmetry, inverses and relation hierarchy derive new facts by forward chaining. Derivation is off by default, since a wrong axiom derives wrong facts. A derived fact is marked as such on the graph, carries validity and confidence like any other, and shows what it was derived from. When it contradicts an asserted fact, the asserted one stands. |
| **Conflict detection** | Three kinds of conflict, three sets of choices. A new fact that clashes with an older one: close the old, keep both, or reject the new. Data that breaks an axiom (self-loop, asymmetry, transitive cycle, cardinality): retract the fact, relax the axiom, or accept both. The ontology itself is checked first, because violations of a self-contradictory ontology are noise. |
| **Ontology-driven querying** | Mount a database on a base (Postgres, Trino for Iceberg / Delta Lake / Hive, Databricks, Snowflake) and chat can query it alongside the documents. The agent proposes how its tables map onto the ontology, and you confirm. The method behind it, [Ontology2SQL](https://github.com/deeplethe/ontology2sql), is state of the art on BIRD Mini-Dev for SQLite and PostgreSQL ([submission](https://github.com/bird-bench/bird-bench.github.io/pull/218)). |
| **Multi-user and permissions** | Each knowledge base has its own members and roles: owner, admin, editor and viewer. Open bases are readable by everyone in the deployment, restricted ones only by invitation. The first account registered becomes the system administrator. |
| **Decision ledger** | Confirming or rejecting a fact, merging or reverting an entity, rebuilding the graph: each leaves a record of who, when, and what the object looked like at the time. The ledger is append-only, and a record outlives its object, even the base it belonged to. |
| **[Decision intelligence (in development)](#roadmap)** | Record a decision, replay both what was understood and the course it took, and reason over overlaid scenarios. |

## Quick start

Requirements: Docker (local development also needs Rust 1.85+, Node 20+, pnpm).

Start from the prebuilt image:

```bash
git clone https://github.com/deeplethe/utopia.git
cd utopia
docker compose --profile app up -d
```

Open http://localhost:1516 and register. The first account automatically becomes the administrator, and a public knowledge base readable by everyone is created at the same time. Before extracting business documents, configure the model endpoints (chat and embedding) under system settings.

Or build from source:

```bash
docker compose -f docker-compose.yml -f docker-compose.build.yml --profile app up -d --build
```

### Local development

```bash
# 1. Postgres with pgvector
docker compose up -d db

# 2. Backend on :1516, runs migrations on startup
cargo run -p utopia-server

# 3. Frontend on :5173, proxying /api to the backend
cd web && pnpm install && pnpm dev
```

## Roadmap

- [ ] **Decision reasoning**: constraint computation, and replaying a decision after the fact
- [ ] **Execution gate**: checking an agent's calls against ontology rules and symbolic logic
- [ ] **MaxCompute**: mapping exploration and Ontology2SQL over Alibaba Cloud MaxCompute (Iceberg / Delta Lake via Trino, Databricks and Snowflake are in, awaiting a run against a real cluster)
- [ ] **More sources**: MySQL, ClickHouse and Doris drivers; S3, WebDAV, Notion and Feishu connectors
- [ ] **Time to the moment**: an `instant` precision beside year / month / day, for sources that carry a real timestamp. Today a connector rounds it to a UTC day, which can shift an event across midnight by one day
- [ ] **Agent memory over MCP**: episode writes, the retrieve endpoint, and the MCP server
- [ ] **Enterprise**: OIDC SSO, backup and restore commands, benchmarks at 100k documents

## Status

Utopia is still at **v0.1**. The database schema evolves between versions and migrations only roll forward, with no rollback. Pin a specific version with `UTOPIA_IMAGE` in production, and back up the database along with the `data` directory before upgrading.

Please read [SECURITY.md](SECURITY.md) before exposing it to the public internet.

## Star History

<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/deeplethe/utopia/assets/assets/star-history.svg">
  <img src="https://raw.githubusercontent.com/deeplethe/utopia/assets/assets/star-history-light.svg" alt="Star History" width="820">
</picture>

</div>

## Community

- 💬 [Discussions](https://github.com/deeplethe/utopia/discussions): discuss the project, share your experience, and leave feedback
- 🐛 [Issues](https://github.com/deeplethe/utopia/issues): report bugs, ask design questions, and submit feature requests
- 🤝 [Contributing](CONTRIBUTING.md): development setup, pre-push checks, and DCO sign-off
- 🔌 [Ontology2SQL](https://github.com/deeplethe/ontology2sql): the ontology-driven text-to-SQL method referenced above

## License

[Apache-2.0](LICENSE)
