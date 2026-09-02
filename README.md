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

The system is a Rust binary and a Postgres service. pgvector and a queue-table design keep the stack and its service dependencies light.

| Feature | Highlights |
| --- | --- |
| **Complete application** | System console · Graph browser · Browser-based ontology workbench · Ready to use out of the box |
| **Document ingestion** | Multiple document formats, including PDF, Markdown, HTML, PowerPoint, Word, and Excel · Custom subscription-based updates and scheduled synchronization *(Jira and Feishu support in progress)* |
| **Hybrid retrieval** | Tantivy · pgvector embeddings · RRF fusion · Chunk-level provenance |
| **Bitemporal knowledge graph** | Valid time and transaction time · Query the graph as of any point in time · Knowledge change history |
| **Agent harness and agentic RAG** | The application itself serves as an agent harness, providing conversational access to the full system · The built-in agent includes multiple tools and supports multi-turn tool use and dialogue |
| **Built-in ontology packs** | schema.org · W3C Org · PROV-O · FOAF · IOF Core · Continuously expanding · [Request support for your industry](https://github.com/deeplethe/utopia/issues/new?labels=enhancement&title=Ontology%20pack%20request) |
| **Semantic extraction** | Entity, relation, and temporal normalization · Every fact carries a supporting evidence excerpt · Automatic disambiguation through vector and ontology retrieval with LLM adjudication; traceable and reversible · Ontology revision proposals generated from source text |
| **Knowledge derivation and reasoning** | Temporal Datalog · Forward chaining · Ontology axiom compilation · Traceable derivation paths · Lightweight reasoning engine built in Rust |
| **Conflict detection** | Temporal conflicts · Self-loop, asymmetry, transitive-cycle, and cardinality violations · Ontology defects · Retract facts, revise axioms, or accept coexistence |
| **Human review and audit trail** | Low-confidence extractions and merge candidates automatically enter the review queue · Every operation records the user, timestamp, and change snapshot for compliance auditing |
| **Intelligent mapping and data querying** | Select a database and knowledge base, then let the agent explore and establish mappings automatically · Data querying powered by Ontology2SQL · [State-of-the-art result on BIRD Mini-Dev](https://github.com/bird-bench/bird-bench.github.io/pull/218) |
| **Model integration** | Any OpenAI-compatible API endpoint · Support for locally deployed models |
| **Multi-user, multi-knowledge-base** | Roles and permissions scoped to each knowledge base · System administrator and user roles · Separate administration, editing, and access levels for knowledge bases |
| **[Decision intelligence *(in development)*](#roadmap)** | Decision records · Replay of how understanding evolved and how decisions were made · Reasoning across layered scenarios |

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
- [ ] **Lakehouse for mapping and querying**: mapping exploration and Ontology2SQL over Iceberg / Delta Lake, Databricks, Snowflake and MaxCompute
- [ ] **More sources**: MySQL, ClickHouse and Doris drivers; S3, WebDAV, Notion and Feishu connectors
- [ ] **Time to the moment**: an `instant` precision beside year / month / day, for sources that carry a real timestamp. Today a connector rounds it to a UTC day, which can shift an event across midnight by one day
- [ ] **Agent memory over MCP**: episode writes, the retrieve endpoint, and the MCP server
- [ ] **Enterprise**: OIDC SSO, backup and restore commands, benchmarks at 100k documents

## Status

Utopia is still at **v0.1**. The database schema evolves between versions and migrations only roll forward, with no rollback. Pin a specific version with `UTOPIA_IMAGE` in production, and back up the database along with the `data` directory before upgrading.

Please read [SECURITY.md](SECURITY.md) before exposing it to the public internet.

## Community

- 💬 [Discussions](https://github.com/deeplethe/utopia/discussions): discussion, experience reports and reviews
- 🐛 [Issues](https://github.com/deeplethe/utopia/issues): any bug, design question or request
- 🤝 [Contributing](CONTRIBUTING.md): dev setup, the checks to run before pushing, DCO sign-off
- 🔌 [Ontology2SQL](https://github.com/deeplethe/ontology2sql): the ontology-driven text-to-SQL method referenced above

## License

[Apache-2.0](LICENSE)
