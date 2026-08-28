<div align="center">

<img src="assets/banner.webp" alt="Utopia" width="100%">

**The world's first open-source enterprise world model.**

[Philosophy](#philosophy) · [Quick start](#quick-start) · [Features](#features) · [Roadmap](#roadmap)

[![Official site](https://img.shields.io/badge/OFFICIAL-UTOPIA.BI-8A6D1F?style=flat-square&labelColor=161B22)](https://utopia.bi)
[![License](https://img.shields.io/badge/LICENSE-APACHE%202.0-1F4B3F?style=flat-square&labelColor=161B22)](LICENSE)
[![Rust](https://img.shields.io/badge/BUILT%20WITH-RUST-6B3524?style=flat-square&labelColor=161B22&logo=rust&logoColor=C9D1D9)](https://www.rust-lang.org)
[![Container](https://img.shields.io/badge/GHCR-DEEPLETHE%2FUTOPIA-1E3A5F?style=flat-square&labelColor=161B22&logo=docker&logoColor=C9D1D9)](https://github.com/deeplethe/utopia/pkgs/container/utopia)

[![Discussions](https://img.shields.io/badge/DISCUSSIONS-3B2A52?style=flat-square&labelColor=161B22&logo=github&logoColor=C9D1D9)](https://github.com/deeplethe/utopia/discussions)
[![Built by DeepLethe](https://img.shields.io/badge/BUILT%20BY-DEEPLETHE-2D333B?style=flat-square&labelColor=161B22)](https://github.com/deeplethe)
[![中文](https://img.shields.io/badge/LANG-%E4%B8%AD%E6%96%87-5C2A2A?style=flat-square&labelColor=161B22)](README.zh-CN.md)

</div>

---

<!-- Video: drop an mp4 into any issue/PR comment box, GitHub returns a
     https://github.com/user-attachments/assets/xxx link,
     paste that link on its own line here and it renders as a player. -->

<div align="center">

https://github.com/user-attachments/assets/PLACEHOLDER

</div>

---

## Philosophy

> **We strongly recommend reading this chapter.** About 5 minutes.

**An agent memory? Another Graph RAG? Or one more DAG?** It can be any of those, and it doesn't stop there.

What we set out to build is a world model that belongs to your company: it understands the people, events, rules and relations inside it, and equally how they change over time, why they hold, and when they cease to. Memory, knowledge, rules, reasoning and action are each only a part of it.

So we gave it a somewhat romantic name: **Utopia**. Put in engineering terms, it is a passively evolving enterprise world model — **a hub for knowledge, a foundation for decisions, a gate for controlled execution, a proving ground for reasoning about what comes next.**

But you find out on maintaining it that building a world is not the hard part; keeping it running for years, steadily and credibly, is. An ideal state needs great institutions. Hence the laws that follow.

| Law | |
|---|---|
| **Zeus's Law** | Treat every piece of knowledge that arrives well |
| **Law of History** | Knowledge is not a static table of facts but a history still unfolding |
| **Law of Deduction** | From understanding the present toward reasoning about the future |
| **The Iron Gate** | Let rules hold up intelligence; let logic bound action |

### Zeus's Law

> **Treat every piece of knowledge that arrives well.**

In ancient Greece, Zeus guarded strangers and travellers. Wherever they came from, they were to be received before they were known. Utopia hopes to treat every piece of knowledge entering this world the same way.

It may come from a document, or from a database, a warehouse, a web page, a running subscription; it may be fully structured, or it may be nothing more than a new way of putting something. We will not turn it away merely because it does not yet fit an existing shape.

The system tries to recognise entities, facts and relations within it, forming its own ontology through the cold-start phase. New predicates may appear; old ones are not casually discarded; and when a fact changes, the trace of what it once was is kept along with the chain of that change.

Treating knowledge well does not mean understanding everything at any cost. We aim for a four-way balance between accuracy, extraction throughput, cost, and human involvement.

This is Utopia's Law of Zeus: treat well every piece of knowledge that comes into this world.

### Law of History

> **Knowledge is not a static table of facts but a history still unfolding.**

Knowledge graphs have tended to record settled facts, forever pursuing which knowledge is *correct*. Yet whether something is correct is at times a question only time can answer.

Ptolemy's geocentric system was long held to be a reasonable account of cosmic order; then Copernicus proposed the heliocentric one, Kepler corrected the model of planetary motion, Galileo brought new observational evidence, and Newton reinterpreted celestial movement under a unified mechanics. Today we of course know that geocentrism is no longer the correct model of the solar system.

And yet we remember it.

We remember not only that geocentrism existed, but why it was believed, in what age it was accepted, and how observation, theory and evidence revised and displaced it step by step. Because a truthful world needs to record more than *what is correct*; it needs to record what we once believed, and how we got from there to here.

So in Utopia a piece of knowledge is not simply overwritten or deleted because a new fact appeared. The system records when it was ingested, when it changed, the span over which it held in the real world, and the moment we learned of that change. We can therefore ask not only "what is true now?" but also:

```
"What did we believe was true at the time?"
"When did this begin to hold?"
"And when did we learn that it had changed?"
```

Only by preserving these changes can knowledge take real part in the unfolding of history. In engineering terms we call it a **bitemporal knowledge graph**.

### Law of Deduction

> **From understanding the present toward reasoning about the future.**

Laplace imagined an intellect vast enough to know, at a single moment, the state of every particle in the universe together with all the laws governing their motion; in principle it could then derive the entire past and future. Which is to say it could predict not only the courses of the stars, but what the one writing this README is thinking, and what thought will arise in you reading it a second from now.

In Utopia we try to approach that ideal by engineering: forward chaining drives facts to keep deriving, a symbolic system expresses rules, states and causal relations, and a language model handles the reasoning that resists full formalisation. Given enough facts, rules and causes, perhaps the system can move from understanding the present toward reasoning about the future.

### The Iron Gate

> **Let rules hold up intelligence; let logic bound action.**

An ideal state does not mean doing as one pleases. Quite the opposite: here every action should run within law, rule and boundary. What we least want to see is a model's hallucination, a human oversight or a flaw in reasoning turning into a logical error, an overreach of authority, or worse, an irreversible action.

So in Utopia an agent's judgement is not by itself an action. Every call to a downstream service must, before it actually runs, pass a joint check by ontology rules, constraints and symbolic logic:

```
Does it agree with the facts?     Are its preconditions met?
Does it hold the authority?       Does it cross a set boundary?
        Is its outcome consistent with the current state of the world?
```

Only through these rules can reasoning become action. A model may think boldly, but execution must be restrained; an agent may explore the unknown, but it may not cross the iron law of this world.

> Go on, agent.
> Catch every task steadily, and hold every boundary just as steadily.

## Features

The system is a single binary plus a Postgres service. By bringing in pgvector and a queue-table design, we cut the weight of the stack and its service dependencies — deployment is over in a blink.

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
