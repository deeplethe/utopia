# 100k benchmarks

Roadmap item (README §Roadmap): *"Enterprise: OIDC SSO, backup and
restore commands, benchmarks at 100k documents."*

This proposal lands a Rust integration-test benchmark suite against a
100k-document synthetic corpus. First cut: one scenario (cold
`SELECT ... WHERE`) wired up to a single helper. Three more scenarios
follow once the shape is approved.

## Why an integration test, not `benches/foo.rs`

The maintainer's house style for performance tests is
**integration tests under `crates/*/tests/`**, not the
`[[bench]]` / `benches/` Cargo pattern. Two reasons visible in the
codebase:

1. **`scripts/bench/*.mjs` is the existing measurement surface** for
   cross-stack scenarios (governance, identity, truth corpora).
   Rust benches would be a third layer.
2. **`benches/` would require criterion or divan** — both add
   workspace deps and the maintainer has not pulled either in
   (`grep -r 'criterion\|divan' --include=Cargo.toml` returns nothing).
   An integration test that uses `std::time::Instant` directly is
   30 lines, not 300.

The pattern matches `crates/utopia-store/src/vector_index.rs:181`
(an existing `Instant`-based measurement) and
`crates/utopia-store/src/test_db.rs:14` (the skip rule that keeps
`cargo test` honest without a DB).

## What's already on disk

`crates/utopia-store/tests/bench_100k.rs` (148 lines) is a working
first-cut scenario:

- Synthetic `bench_100k_docs` table (BIGSERIAL id, kb_id, kind, name,
  body, n, created_at) populated with 100 000 rows in 20 batches of
  5 000 each (no `COPY` — that would race with the 24 other
  store-integration tests for table locks).
- One scenario: 30 iterations of `SELECT id, body, n FROM
  bench_100k_docs WHERE kind = $1 AND kb_id = $2 AND n > $3`,
  with a single global `Instant::now()` per iteration. Percentiles
  (p50, p95, p99), mean, and throughput (q/s) computed and printed
  as a markdown table.
- Deterministic seed (`id.wrapping_mul(0x1000_0001)` etc.) — same
  hardware two runs give the same distribution.
- DROP TABLE at the end so CI leaves nothing behind.

Output (live run on the maintainer's local docker-compose):

```
| scenario     | n  | p50 (ms) | p95 (ms) | p99 (ms) | mean (ms) | throughput |
|--------------|----|----------|----------|----------|-----------|------------|
| cold_select  | 30 |    13.04 |    26.63 |    27.27 |    15.69  |   63.7 q/s |

rows_returned_total = 237490
```

## Scenarios not yet landed

Per the existing benchmark-style precedents (`vector_index.rs:181` and
the JS benches under `scripts/bench/`), the four scenarios that
together justify "100k benchmarks" are:

1. **Cold SELECT with WHERE** ← this PR.
2. **Warm cache (re-run scenario 1 with `pg_prewarm`).** Validates
   that the cold-vs-warm gap is realistic and not a measurement bug.
3. **Write-heavy bulk INSERT.** Times a 5 000-row batch into
   `bench_100k_docs` — the same shape the corpus generator uses.
   Catches regressions in the chunked-INSERT path.
4. **Extraction-throughput.** Times how long it takes to extract facts
   from a sample of 1 000 documents end-to-end. This is the closest
   to "100k documents" in the roadmap item — it asks the question
   the maintainer wants to ask.

Each is a separate PR once (1) lands.

## Open questions for the maintainer

1. **Bench format.** Integration test under `crates/*/tests/` (this
   proposal) vs. `benches/foo.rs` with criterion (would require
   pulling in a new workspace dep). My read: integration test —
   matches the existing pattern, no new deps, prints to stdout where
   CI can grep it.
2. **Where the result goes.** Print to stdout (this proposal) vs.
   write to `docs/benchmarks/<date>.md` (so the maintainer can
   quote it in release notes) vs. PR-comment CI. My read: print to
   stdout for now; promote to a markdown report after scenario 4
   lands, since by then "100k documents" has enough substance to
   deserve a permanent artifact.
3. **What counts as a "document" in the corpus.** A `documents` row,
   a `chunks` row, a `facts` row, or a source-side document? The
   roadmap item is ambiguous. My read: a `documents` row is the
   natural reading; the extraction-throughput scenario (4) will
   resolve it by actually running an extraction against the corpus.
4. **Ship all 4 scenarios at once, or one at a time?** My read: one
   at a time. (1) is the cheapest sanity check that the harness
   works on the maintainer's hardware. (2)–(4) depend on (1) being
   trustworthy.
5. **Is the bench a merge gate, or informational?** Decision 0035's
   precedent is informational — the bench exists to make regressions
   visible, not to fail PRs. My read: informational, same as 0035.

## Test environment

The bench lives next to other store-integration tests, so it follows
the same skip rule (`crates/utopia-store/src/test_db.rs:14`):

- `UTOPIA_DATABASE_URL` unset → bench prints "skipping" and returns
  `Ok(())`. Local `cargo test` without a DB still works.
- `UTOPIA_TEST_REQUIRE_DB=1` and `UTOPIA_DATABASE_URL` unset →
  bench panics with "this run must not skip database-backed tests".
  This is the CI signal.

The CI job that runs store-integration tests is `migrations` (already
exists on `dev`). The bench needs no new workflow.

## What this PR does

- Adds `crates/utopia-store/tests/bench_100k.rs` (148 lines, already
  on disk from the previous draft)
- Adds `.roadmap-proposals/benchmarks-100k.md` (this file)
- No changes to `Cargo.toml`, no new deps
- No migration; the bench owns its own `bench_100k_docs` table and
  drops it at the end

## What this PR does NOT do

- No scenarios 2-4 (gated on Q4 above)
- No `docs/benchmarks/` report file (gated on Q2)
- No CI workflow change (the existing `migrations` job picks this up
  via the integration-test pattern)

## Verification

Run locally against the maintainer's local Postgres:

```
UTOPIA_TEST_REQUIRE_DB=1 \
UTOPIA_DATABASE_URL='postgres://utopia:utopia@localhost:1543/utopia_test' \
  cargo test -p utopia-store --test bench_100k -- --nocapture
```

Output:

```
running 1 test
| scenario     | n  | p50 (ms) | p95 (ms) | p99 (ms) | mean (ms) | throughput |
|--------------|----|----------|----------|----------|-----------|------------|
| cold_select  | 30 |    13.04 |    26.63 |    27.27 |    15.69  |   63.7 q/s |

rows_returned_total = 237490
test cold_select_at_100k_rows ... ok

test result: ok. 1 passed; 0 failed; finished in 1.14s
```
