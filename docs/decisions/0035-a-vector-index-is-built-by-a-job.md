# 0035 · A vector index is built by a job

- **Status**: Implemented (#512, #514)
- **Written**: 2026-09-09 (conventions in the [README](README.md))
- **Related**: the ingest migration said P1 scans sequentially and an HNSW index comes "at volume"; this is that note coming due. [0019](0019-the-second-clock-can-be-rewound.md) is why the record-axis filter stays on the query and the index accommodates it. [0016](0016-close-the-open-seams-before-cutting-new-ones.md) C2 is the loop that scanned the entity table once per subject.

> `chunks.embedding` had no index of any kind, so every hybrid query computed cosine distance against every embedded chunk in the base and sorted the lot to take ten. `entities.profile_embedding` had none either, and type resolution asked it once per subject, sixty subjects a round, ten rounds a job. The column is `vector` with no `(N)`: the dimension follows the workspace's embedding model, which no migration knows.

## Measured before deciding

Sixty thousand chunks of 1024 dimensions in one table across three bases (50,000 / 10,000 / 20), random vectors, pgvector 0.8.6.

| | |
|---|---|
| Today's query, base of 50k | seq scan, 195 ms |
| Today's query, base of 10k | `chunks_kb_idx` + sort, 39 ms |
| Expression HNSW index, serial build over 60k rows | 87 s, 469 MB |
| Rewritten query, base of 50k | 4.5 ms |
| HNSW forced on the 10k base, `iterative_scan = off` | 3 rows of the 24 asked for |
| same, `relaxed_order` | 24 rows, 7.9 ms |

So the cost was about 4 ms per thousand chunks in the base being searched, per query. Nobody feels it at hundreds of chunks; a base of 100k pays 0.4 s per question on this leg, and type resolution paid it sixty times a round.

Then again with the code in this record, on a copy of a bench base (531 real chunks and 3,269 real entity profiles at 1024 dims) with a synthetic base of 50,000 chunks written beside them:

| | |
|---|---|
| The rewritten `vector_search` on the 50k base, no index | seq scan and top-N sort, 378.6 ms |
| same, index built | index scan, 6.0 ms |
| Serial build over 50,531 rows at the default `maintenance_work_mem` of 64 MB | 4 min 20 s; "graph no longer fits after 13,930 tuples" |
| same at 512 MB | 60.6 s, 394 MB |
| 200 real entity queries on a base of 1,415 profiles, HNSW forced, recall@10 against the exact top ten | 0.997 by distance; 0.917 by id, the gap being ties at the tenth place |
| 522 real chunk queries, each base about 1% of the index, HNSW forced, `scan_mem_multiplier` 1 | recall@10 0.705 with `relaxed_order`, 154 lists short, 32 ms a query; 0.075 without `relaxed_order` |
| same, `max_scan_tuples` 100,000 | unchanged: 0.705, 154 short |
| same, `scan_mem_multiplier` 4 | recall@10 1.0, no list short, 74 ms a query; 16 is the same |
| A real base of 229 chunks, HNSW forced, the query far from the base | 0 rows after 19,345 tuples, 105 ms |
| The same base with `chunks_kb_idx` back and the planner choosing | `chunks_kb_idx` and a sort, 1.5 ms; the 50k base beside it still takes the HNSW path, 3.8 ms |

The forced lines are the planner's choice taken away (the `kb_id` index dropped on the copy): they are the floor, and the reason the scan settings below are not the defaults.

## Decisions

**A job, not a migration.** The dimension is only known when vectors are written, and `CREATE INDEX CONCURRENTLY` cannot run inside the transaction sqlx wraps migrations in, while a plain `CREATE INDEX` holds `ACCESS EXCLUSIVE` on `chunks` for the length of the build. So the first write of a dimension asks for an index (`vector_index::request`), which queues one `build_vector_index` job for that table and dimension unless one is already queued, and the job builds it `CONCURRENTLY IF NOT EXISTS` on a plain connection. An index that a failed build left invalid is dropped and rebuilt, because `IF NOT EXISTS` would otherwise take it for finished. Once the index is seen the process remembers it, so a write costs a set lookup; before that it costs one catalog query and one insert-unless-queued, for the minute or two the build takes. The build's session sets `maintenance_work_mem` to 512 MB and `max_parallel_maintenance_workers` to 0: at the default 64 MB the graph stops fitting after some fourteen thousand rows of 1024 dimensions and every row after that goes through disk (four minutes and twenty seconds for 50k rows against one minute), and a parallel build wants shared memory that Docker's default 64 MB `/dev/shm` does not have.

**A partial expression index per dimension.** `USING hnsw ((embedding::vector(N)) vector_cosine_ops) WHERE vector_dims(embedding) = N`. The cast gives pgvector the dimension the column type lacks; the predicate keeps rows of another dimension out, so a workspace that changed its embedding model has two indexes and two populations rather than one build that fails. `vector_cosine_ops` matches the `<=>` the queries use.

**The dimension is a literal in the SQL.** With `vector_dims(embedding) = $2` bound as a parameter the custom plan uses the partial index and the generic plan falls back to a sequential scan. sqlx's prepared statements switch to a generic plan after five executions, so the index would have stopped being used on the sixth query, silently. `same_dims` and `distance` format the integer in, and the `ORDER BY` expression is character for character the indexed one.

**`hnsw.iterative_scan = relaxed_order` on every nearest-neighbour read.** HNSW takes `ef_search` candidates and applies the `WHERE` afterwards. On a table shared by tenants a small base holds few of those candidates, and `LIMIT 10` comes back with three rows or none: measured above, and the ordinary case rather than a corner. Iterative scan keeps walking until the limit is met, and it stops early on two conditions, both loosened here. The one that bound in measurement is memory: `hnsw.scan_mem_multiplier` caps the scan at that multiple of `work_mem`, and at the default of 1 (4 MB) the forced scan over real bases that are each a hundredth of the index came back short on 154 of 522 queries, recall 0.705, at 32 ms a query; at 4 every list filled, recall 1.0, at 74 ms a query, and 16 changed nothing more. The other is `hnsw.max_scan_tuples`, raised from 20,000 to 100,000; it did not bind on the copy (the same 0.705 at either value), and it is raised because of what each failure looks like: a short list is silent, a slow query is visible, and a hundred thousand tuples bound the slow case at about half a second. The planner keeps a base that small on the exact path when it has the choice (229 real chunks beside the 50k: `chunks_kb_idx` and a sort, 1.5 ms), so the forced figures are the floor rather than the expectation. All three settings are `SET LOCAL`, so each read runs in a transaction. A pgvector older than 0.8 has no such setting; the process probes once and leaves it unset, and the query is still correct.

**The planner chooses the path.** With `chunks_kb_idx` present it picked index-plus-sort for the 20-row and 10k-row bases on its own and HNSW only for the 50k one. No application-side threshold: a threshold is a guess about the planner, and a wrong guess is slow on both sides.

**Entities take the same mechanism.** `nearest_typed_entities` reads the subject's vector first so the dimension can be written into the SQL, then runs the same shaped query. It gained a dimension guard it never had: a base with two dimensions of profiles used to error on `<=>`.

**The loop gathers before it reasons** (#514). The sixty neighbour queries of a batch are independent; they were serial because the loop was. `nearest_typed_for_each` runs them eight at a time with `buffered`, which yields in input order, and the per-subject reasoning stays in that order because adjudication downstream reads it. Eight is well under the pool of 32, which `db.rs` sizes for concurrent short queries. `descendants_of` is keyed on the coarse class, drawn from a small vocabulary, so a batch asks the same recursive query over and over; `DescendantsMemo` answers once per class per batch. A subject with no class has no descendants axis (0009) and stays out of the memo rather than sharing a key with a real class.

**Above 2000 dimensions there is no index.** That is the HNSW limit for `vector`; `text-embedding-3-large` is 3072. Such a dimension is never requested and the query stays exact. `halfvec` reaches 4000 at another precision and waits for someone to need it.

## Dead ends

- **ivfflat.** Needs a representative sample at build time to pick its lists, and a corpus that grows past the sample loses recall quietly. For a system whose claim is traceable evidence, silent recall loss is the wrong failure mode.
- **A numbered migration.** Two branches each adding the next number merge cleanly and then neither runs; and see the transaction above.
- **Dropping the record-axis filter so the index applies cleanly.** Replay is the product (0019). The index accommodates the filter.
- **Sizing the pool to the batch.** Sixty concurrent scans would fit a pool of 64 and move the load onto Postgres; `db.rs` already argues against sizing the pool to the worker count.
- **Caching neighbours across subjects.** Every subject has a different query vector; there is nothing to share.

## What the tests pin

Every question is asked twice, without the index and with it, and the answers must agree: the nearest chunk, a small base beside a large one filling its `LIMIT`, tenancy, a second dimension in one table, a superseded chunk, a moment on the record axis. For entities: the batch comes back in input order at concurrency 1 and 8, with and without the index; a two-connection pool completes sixty subjects; the memo returns what the query returns and does not notice a class added mid-batch. A test asserts on answers only; a plan change stays quiet.

## Open questions

- **Recall on a corpus the planner sends to the index.** The entity figure above (0.997) is on real profiles with the index forced; the chunk figures are on real chunks in an adversarial table, a synthetic tenant whose vectors sit closer to every query than the query's own base does, and they reach 1.0 only with the scan memory raised. A real deployment with two large tenants of different subject matter is the case still unmeasured, and `hnsw.ef_search` (40 here; 200 cost 10 ms in the synthetic run and changed nothing on the copy) is the first knob if it disappoints.
- **A dimension that leaves.** When a workspace changes model, the old dimension's index stays until someone drops it. It is small harm and no mechanism yet.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> implemented · a partial HNSW index per dimension on `chunks.embedding` and `entities.profile_embedding`, requested by the first write of that dimension and built by a `build_vector_index` job outside any transaction · `vector_search` and `nearest_typed_entities` write the dimension as a literal, cast both sides and set `hnsw.iterative_scan = relaxed_order` · type resolution gathers a batch's neighbours eight at a time, in order, and remembers descendant sets per batch (#512, #514) · dimensions above 2000 stay on the exact path
