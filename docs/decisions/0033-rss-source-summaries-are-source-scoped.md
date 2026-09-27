# 0033 · RSS summaries are scoped to the source being listed

- **Status**: Implemented (#462, #417)
- **Written**: 2026-09-06 (conventions in the [README](README.md))
- **Related**: [0023](0023-rss-observations-are-not-documents.md) established RSS observations as a separate responsibility from documents; #417 changes the public `SourceView` contract while fixing the scope of its RSS summary.

> A Library page lists ten sources. The database holds several thousand RSS observations, most belonging to other sources or older activations. The list query should count the rows belonging to each source and its current generation, not project the whole RSS observation table before it starts returning sources.

## What the source list does today

`sources::list` wraps the complete `ENTRY_SELECT` projection in one global CTE and then runs six correlated counts over that projection for every source. `ENTRY_SELECT` is also the canonical place where observation and job state becomes `pending`, `queued`, `hydrating`, `retry_wait`, `complete`, `terminal`, `deleted` or `superseded`.

The CTE is referenced six times. PostgreSQL 12 and later inline a non-recursive, side-effect-free CTE by default only when it is referenced once; this CTE is therefore materialized. Because `ENTRY_SELECT` has no `WHERE`, materialization evaluates its three joins, deletion `EXISTS` and nine-branch state `CASE` for every RSS observation in every knowledge base and generation. The materialized result has no index, so each correlated count scans it in full for each source. The resulting shape is `O(sources × all entries × 6)` rather than `O(sources × own entries)`.

The projection also exposes implementation state — `generation` and `baseline_count` — as part of the public source-list response.

## Decisions

### 1. Aggregate only the source and generation being listed

The source list will use a `LEFT JOIN LATERAL` aggregate per source. The aggregate is parameterized by the outer source's `id` and current `rss_generation`, and its filters are pushed into `rss_full_content_entries` through the implicit btree named `rss_full_content_entries_source_id_activation_generation_ex_key`, which backs `UNIQUE (source_id, activation_generation, external_key)`. The partial `rss_full_content_entries_pending_idx` cannot serve this aggregate. The outer `sources` table must not use the `s` alias because `ENTRY_SELECT` has its own `sources s` join; reusing it would silently bind the source filter to the inner row and count entries from other sources. The `listed_source.kind = 'rss'` predicate appears both inside the lateral subquery and on the join. The join predicate is what excludes non-RSS sources from the lateral result; the inner predicate is retained as a defensive guard. The required execution plan will show how PostgreSQL applies both predicates rather than relying on an assumed one-time short circuit.

The aggregate obtains `pending`, `queued`, `retrying`, `complete` and `terminal` with five `count(*) FILTER` expressions in one pass over the source's own entries. The first implementation cut also computes the existing internal `baseline_count` in that pass so it can preserve the flat API unchanged. It reuses the canonical `ENTRY_SELECT` projection and does not duplicate its state `CASE` in `sources.rs`. `queued` remains the union of `queued` and `hydrating`; `terminal` remains the union of `terminal`, `deleted` and `superseded`. Only rows whose `activation_generation` equals the source's current `rss_generation` are counted.

`baseline` rows are intentionally excluded from all five counts. They represent pre-activation feed stock, not outstanding hydration work. A source containing only baseline observations therefore reports zero for all five work counts even though its observation ledger is not empty; the summary describes hydration work, not ledger cardinality. `baseline_count` remains available only to internal activation state where it is needed.

The query continues to preserve source ordering, document and missing counts, credential removal, the `SOURCE_SECRET_KEYS` bind, and `config - $2::text[]`. Observation, job and document responsibilities remain unchanged, as does `rss_full_content::counts()` and the diagnostic list.

### 2. Make the public summary an explicit nested type

`SourceView` will replace its flat RSS fields with a nullable `rss_full_content` object:

```json
{
  "rss_full_content": {
    "state": "active",
    "pending": 1,
    "queued": 2,
    "retrying": 3,
    "complete": 4,
    "terminal": 5
  }
}
```

The Rust API will expose a strongly typed `RssFullContentSummary`, not an arbitrary JSON value. A non-RSS source returns `"rss_full_content": null`. An RSS source always returns an object, including when full-content hydration is disabled. `queued` and `terminal` retain the state unions described above. `generation` and `baseline_count` remain internal state and are removed from the source-list API.

The store will deserialize a private flat `SourceListRow`, then explicitly convert it to `SourceView`. Missing SQL fields or a row that cannot form the required RSS object are storage errors; the conversion will not use `unwrap`, `expect` or silent defaults to hide them.

> **Revised 2026-09-12, in the cut that implemented this.** The mechanism is the one thing here that did not survive contact. The query builds the object with `jsonb_build_object` inside the existing `CASE WHEN kind = 'rss'`, and `SourceView` reads it through `#[sqlx(json(nullable))]`; there is no second flat row struct. What that paragraph was protecting is intact and arguably better served: the type is still `RssFullContentSummary` and not an arbitrary JSON value, a block that cannot be formed is a deserialization error from the driver rather than a default, and there is no `unwrap` or `expect` on the path. What it cost, had it been followed literally, was a private struct restating all fourteen non-RSS columns so that six of them could be moved — a second place to forget a column, to buy a guarantee the typed column already gives. The shape of the contract, which is what this decision is actually about, is unchanged.

The TypeScript contract and Library consumer will use the nested object and first check it for null. No compatibility double-write of the old fields is planned. No UI redesign, new component, visual-style change or new copy is part of this decision.

### 3. Do not add schema or runtime dependencies

This change adds no migration and no new Rust or npm dependency. It changes the read query, the API model, the TypeScript contract and their tests only. SQL values continue to use binds; any dynamic SQL is assembled only from repository-owned constants.

### 4. Land the decisions independently

The query optimization and public contract will be implemented in two PRs against this record. The first PR changes only the store query and preserves the existing flat API fields, making the performance change independently measurable and revertible. The second PR replaces those fields with the nested Rust and TypeScript contract and updates the Library consumer. A problem in either change can then be reverted without taking back the other.

## Alternatives rejected

- **Keep the global CTE.** It is simple to reuse, but its work grows with the entire deployment rather than with the source being listed. A larger observation table makes every Library load more expensive.
- **Run one query per count.** Five round trips and repeated scans add latency and work for the same source summary.
- **Fetch every entry and count in Rust.** It transfers and retains data that the endpoint only needs as five numbers, making network and memory costs unacceptable.
- **Add a cache table for the summary.** A cache introduces consistency and invalidation problems. The current read path is sufficient once its scope is source- and generation-bound.

## Performance evidence required by the query implementation

The query implementation PR will include before/after `EXPLAIN (ANALYZE, BUFFERS, VERBOSE)` results from a dedicated PostgreSQL database with one knowledge base, at least one full-content RSS source, several thousand current-generation observations, and observations for other sources or knowledge bases. The comparison will record actual entry scan rows, use of the `source_id`/`activation_generation` index, lateral loops, shared buffer hits/reads, execution time, and whether non-RSS sources avoid scanning entries. Temporary SQL, plans and generated data will stay out of the repository.

## Open questions

- **A transition period for the flat fields.** Not included. The nested object is the public contract approved by this record; a compatibility period would need a separate API decision.
- **A summary cache.** Deferred until measured source-scoped aggregation is insufficient for a real workload; the invalidation boundary would need to be designed with it.
