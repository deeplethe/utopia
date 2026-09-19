# 100k cold-SELECT benchmark — first cut, design

This is the rework of the first cut on PR #713. The previous cut
measured Postgres filtering of 100k synthetic rows in a synthetic
table, which does not tell us anything about Utopia. This cut
populates the real `documents`, `chunks` and `facts` tables for one
base and times a real product read path against them.

## What "document" means

The maintainer's answer to question 3:

> A "document" is a `documents` row together with its `chunks` and
> `facts` rows; the corpus should look like the product's ledger,
> not a table of its own.

So a 100k-document corpus is **100 000 `documents` rows** plus their
**chunks** (a handful per document, say ~3–8) plus their **facts**
(a handful per chunk, ~2–6). That works out to roughly:

| table | rows |
|---|---|
| `documents` | 100 000 |
| `chunks` | ~500 000 |
| `derived_facts` (and friends) | ~2 000 000 |

Worth knowing up front: the facts table is the one that gets large.
The bench does not need to exercise every column on every table —
populate what `graph::facts_hold_at` and `search_entities` actually
read.

## The corpus shape

The previous cut's critique called out three things a real corpus
needs:

1. **Realistic distributions.** A few hub entities with very many
   facts (which is what the wide-table TPC-H case in #501/#520 is
   about), and merges/corrections in their history. Without hubs,
   `facts_hold_at` looks uniform; with hubs, it touches the temporal
   merge paths that matter.

2. **Realistic write path.** "Writing rows directly is fine (no
   model calls), but the shapes and distributions should be real."
   Concretely: use `documents::upsert_source_document_tx` +
   `documents::replace_chunks` + `documents::set_ready` +
   `graph::insert_fact`. Not raw `INSERT INTO ...`. That way the
   bench measures the same code path the server measures, not a
   short-cut.

3. **A fixture generator.** A single `tests/bench_100k.rs` fixture
   builder that, given a `PgPool` and a `kb_id`, populates one base.
   Runs once per benchmark process, shared across scenarios.

## Scenarios — first cut is one

Maintainer's answer to question 4: "One scenario at a time: yes."

The first scenario is **`graph::facts_hold_at` with `at` and
`as_of`** — the read path a base opens with. Specifically:

- Pick one entity id out of the corpus (one of the hubs).
- Run `facts_hold_at(pool, kb_id, entity_id, at=T, as_of=A)` for
  `(T, A)` ∈ { (now, now), (T-30d, now), (T-1y, now),
  (now, T-30d) }.
- Time each call. Discard the first iteration (warmup).
- Print p50 / p95 / p99 / mean / throughput, identical layout to
  the previous cut's markdown table.

Why this one specifically:

- It is the read path a base opens with. If it is slow, **every**
  base is slow.
- It exercises the temporal join the previous cut missed.
- It runs against the real schema, not a synthetic one.

Scenarios queued for follow-up PRs (not in this cut): entity
detail, hybrid retrieval (BM25 + vector), `temporal::reconcile_new_fact`
on a hub, paging a review queue, deleting a document cited across
many timelines. Each becomes its own PR with its own fixture config
because each needs a different corpus shape (entity detail needs
chunks of varying size; reconcile_new_fact needs a hub with merge
history).

## How it runs

Maintainer's answers to 1, 2, 5:

1. **Integration test, no new dependencies.**
2. **Write to `docs/benchmarks/<date>.md`.** The bench reads its own
   git `HEAD` date? No — `chrono::Utc::now().date()` at the moment
   the bench runs, formatted `YYYY-MM-DD`. The previous run stays as
   a separate file. Comparison is by reading two files.
5. **Informational, not a merge gate.** The test is `#[ignore]`-able
   by default; CI does not run it. The maintainer signs off as 0035.

Mechanically:

- The test sits in `crates/utopia-store/tests/bench_100k.rs`.
- A small env var `UTOPIA_BENCH=1` (or similar) gates the run; without
  it, `cargo test -p utopia-store` builds and skips silently. With
  it, the test populates, runs, and writes the report.
- The report file path: `docs/benchmarks/<utc-date>-100k.md`. The
  bench fails loud if `docs/benchmarks/` does not exist or is not
  writable.
- Teardown drops the created rows (transactional; the test owns its
  own base id and rolls back if anything goes wrong).

## What the bench does NOT do (this cut)

- No CI integration. The `migrations` job in CI does **not** load
  100k rows. The previous cut's "1.8s in the log, numbers nobody
  reads" was a real cost without a real benefit.
- No new dependencies. The previous cut pulled in `std::time::Instant`
  — that stays.
- No streaming benchmark, no concurrent benchmark, no model call.
- No migration-count or schema-version skew — populating 100k rows
  is via the write functions, not raw SQL, so it goes through the
  same migration-tested write paths the server uses.

## Honest scope

A 100k-document corpus with chunks and facts is a real piece of
work — at ~5 chunks and ~5 facts per document, the fixture is
populating ~2.5 million rows. Even at 10 000 rows/sec on a local
Postgres that's several minutes of setup, which is why this lives
behind `UTOPIA_BENCH=1` and not in CI.

The previous cut's `bench_100k_docs` table had 100 000 rows and
took "1.8 s in the log" — that table had 7 columns and no
constraints. The realistic corpus has more rows, more columns, and
constraints/indexes that exercise the planner. The fixture will be
slower than the previous cut's, but every row that lands is in the
shape the production schema requires.

## Review questions for the maintainer

If anything is wrong with this plan, the part worth flagging is:

- **The hub-entity distribution.** A corpus with 2 hubs of 50 000
  facts each and 100 000 documents with 4 facts each is what I'd
  default to. If the bench needs a different shape (e.g. one giant
  hub and 99 999 documents with 0 facts, to isolate the hub path),
  the generator takes a `BenchCorpus` struct.
- **The scenario choice.** `facts_hold_at` is the maintainer's
  suggestion. If a different read path is more representative of
  the 100k-document product question, the scenario is one line
  away.

## Status

Design only. Code cut is queued for after #781 (the backup+restore
PR) merges, so review bandwidth is on one thing at a time.