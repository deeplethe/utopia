# Use decays; the ledger does not

- **Status**: Proposed · 2026-10-16 · #1277 · open: all of it
- **Written**: 2026-10-16 (conventions in the [README](README.md))
- **Related**: [0014](0014-identity-from-the-person-scope-from-the-token.md) (who may read, write, and through which token); [0019](0019-the-second-clock-can-be-rewound.md) (the record axis, which this leaves alone); [0022](0022-an-unknown-date-is-not-an-open-one.md) (the world axis an expiry closes on); [0052](0052-document-content-is-a-read-contract.md) (what a reader is promised about a document); [0054](0054-a-source-may-push-statements-in-the-open-contract.md) (pushed observations, the fastest-growing case); [0011](0011-a-mapping-is-not-a-fact.md) (the one time the ledger was updated in place); [#1277](https://github.com/deeplethe/utopia/issues/1277), #1271, #1273

> An agent's memory store keeps everything with the same weight forever. A fact read every day and a fact nobody has asked about in a year rank the same, a note meant to hold for a week still holds in a year, and a source that pushes observations (0054) grows without bound. Utopia can say what was believed and when; it cannot say what is still in use.

## Problem

Three things are missing, and they share one shape.

- **Ranking by use.** `retrieval::hybrid` fuses BM25 and vector recall with `utopia_search::rrf_fuse` and truncates to `top_k`. Nothing it reads says whether a chunk has been useful before.
- **An end someone asked for.** A person or an agent that writes "valid until the end of the sprint" has no way to say so except to come back and delete the document by hand.
- **A bound.** A source has no ceiling; an `api` or `statements` source pushing every few seconds grows the base until someone notices.

The ledger cannot hold any of this. A fact is a row that is never updated in place (the ledger design file, 0001 P0); typed rows are a view that `materialize` retires and rebuilds whenever bindings move, linked back to the open statement that first produced them by `facts.from_statement_id`. A counter on a fact row would be written on every read, lost on every rebuild, and would turn the one table that never changes into the one that changes most.

## Decisions

**1. Salience is a side table, keyed by what lasts.**

```
salience (kb_id, target_kind fact|document|chunk, target_id,
          retrieval_count, last_retrieved_at, anchor_at,
          half_life_days 1..365, importance 0..1,
          expires_at, expired_at)
```

A row is created on the first signal about a target; a target without a row reads as the base's defaults, so a base where nobody reads or sets anything stores nothing. For a fact the key is `COALESCE(from_statement_id, id)`: the open statement, which survives rematerialization, rather than the typed row, which does not. A typed row built from several statements carries the salience of the first, because that is the link the row already keeps.

The defaults are columns on `knowledge_bases`: a fact's half-life 30 days, a document's 60, stale after 30 days without a retrieval, `salience_weight` 0. A chunk with no row reads its document's row before the base's defaults. A new row starts at the default half-life, importance 0.5 (nobody said, so setting it can move either way), and `anchor_at` the target's own creation time.

**2. The arithmetic is pure and computed at read time.**

- `strength = 0.5 ^ (age_days / half_life_days)`, with age measured from `anchor_at`.
- A retrieval adds 2 days to the half-life (cap 365), sets `anchor_at` and `last_retrieved_at` to now, and adds one to the count.
- A boost adds 5 days (cap 365). A penalty takes 3 away, never below 7, and never raises a half-life someone set lower by hand.
- `score = strength × (0.5 + 0.5 × importance)`, in 0..1.
- `stale` means no retrieval within the base's stale-after days, counted from `last_retrieved_at`, or from `anchor_at` for a target never read.

Neither the strength, the score nor `stale` is stored: strength changes every second, and a stored value would be wrong by the next read or need a job to keep it right. They are computed in SQL from the row and the base's defaults, and returned beside the item as `salience: { score, stale, retrieval_count, last_retrieved_at }`. That `stale` is not `stale_facts`' word, which means "the evidence sits on an older version of its document"; the two answer different questions and stay in different fields.

`confidence` is not touched. It says how sure the source and its extraction were, set once at insertion and capped by the chunk's origin (0040); how often a fact is read says nothing about whether it is true.

**3. A read records what it returned.**

The REST search route and the `search_chunks`, `entity_facts` and `timeline` tools record a retrieval for each item they hand back, after they hand it back. The tools live once in `tools.rs` and serve chat and MCP alike, so a chat turn's tool call counts the same as an MCP client's. The record is one batched upsert per read, spawned and not awaited; a failure is logged and never fails the read. What RRF fused and dropped, what `chunks_by_ids` filtered out, and the candidate lists of a lookup are never counted: an item the caller never saw has not been used.

A caller that is measuring rather than using (a probe, an eval, a health check) passes `record: false` on the route or the tool and leaves no trace.

An explicit API covers what a read cannot know:

- `POST /api/v1/kbs/{kb_id}/salience/retrievals` with a list of `{kind, id}`: an agent that used something it fetched another way. Viewer, and allowed on a read-only token (0014), because it says only that the caller read what it could already read.
- `POST /api/v1/kbs/{kb_id}/salience/feedback` with `{kind, id, signal: boost|penalize}`. Editor, and a token ticked `write`.
- `PATCH /api/v1/kbs/{kb_id}/salience/{kind}/{id}` with any of `importance`, `expires_at`, `half_life_days`. Editor, and a token ticked `write`.

Each call is one audit row (`salience.retrieval`, `salience.feedback`, `salience.set`) through `audit::record` with `via_token`, so an administrator can see which token taught the base what.

**4. Ranking multiplies, before truncation, and weight 0 is today.**

`final = rrf × (1 + salience_weight × score)`. It is applied in `hybrid` after `chunks_by_ids` has filtered the candidates and before `truncate(top_k)`, so a salient chunk can climb into the window rather than only reorder inside it. `rrf_fuse` returns ids today; #1273 makes it return its scores too.

Multiplicative because an RRF score means nothing outside its own query: it is a sum of `1 / (60 + rank)` terms whose scale depends on how many channels answered. Adding a salience term would let it swamp a one-channel query and vanish in a two-channel one. A factor in `[1, 1 + weight]` keeps the comparison relative within the list it reorders. With `salience_weight` 0 the factor is 1 and the order is exactly today's, which is the default; a base opts in.

Facts are not re-ranked in this record. `entity_facts` and `timeline` return them in the order they do now, with the `salience` field beside each.

**5. An hourly sweep acts on what was asked, and only once.**

Like the governance loop in `main.rs`, an hourly tick lists the bases that have work (a row whose `expires_at` has passed and `expired_at` is null, or a source over its cap) and queues one `salience_sweep` job per base with `jobs::enqueue_unless_queued`, so a slow sweep is never queued twice.

- **A document or chunk past `expires_at`** is deleted the way a person deletes it: `documents::delete` with no actor, which tombstones the document, supersedes its chunks, invalidates only the facts whose every source is gone, and writes a `document_deletions` row that `documents::restore` reads back. A single chunk takes a new, smaller path that sets its `superseded_at` as `replace_chunks` does and records it so it can be restored too. Expiry is reversible because a deadline someone set last month can be wrong.
- **A fact past `expires_at`** is closed on the world axis at `expires_at` through `temporal::close_superseded`, the path a later observation's close takes. Expiry means "it stopped holding then", which is a statement about the world. It is not a retraction: the fact stays on the record axis, `as_of` still finds it, and a reader asking about last week still gets it. A row whose end is already known keeps its end.
- **A source with `max_documents`** (a new, nullable column on `sources`) evicts its lowest-score live documents down to the cap, by the same `documents::delete`.

The sweep sets `expired_at` when it acts. A person who restores an expired document keeps it; the sweep does not act on that row again. Every action is an audit row with no actor (`audit::record_opt`): `salience.expire`, `salience.evict`.

## Dead ends

- **Columns on `facts` and `documents`.** The ledger is append-only, and the one in-place update it ever had (0011's `UPDATE facts SET confidence = 1.0`) was removed for that reason. A counter would also make every read a write on the hottest rows, under the same locks `rewrite_end_tx` and `add_evidence` take, and would be dropped on every typed rebuild.
- **Lowering `confidence` for stale items.** It would answer "how often is this read" in the column that answers "how sure was the source", and the review queue reads that column: an unread but well-sourced fact would land in front of a person as doubtful.
- **Storing strength.** It decays continuously. A stored value is stale by the next read, or needs a job writing every row every hour to keep it close.
- **Counting candidates.** Every query recalls up to 24 chunks per channel and returns six to ten. Counting the recall would credit whatever matches common words, and the feedback loop below would start at the first query instead of the thousandth.

## Risks

- **Popularity feeds itself.** What ranks higher is returned more and so ranks higher still. Counting only what was returned, the 365-day cap, the bounded factor, `penalize`, and a default weight of 0 each limit it; none removes it. A base that turns the weight on should watch whether its top results stop changing.
- **Write volume.** A busy base writes one upsert per read. It is one statement for all the items of that read, off the request path; if it is still too much, the next step is a small in-process buffer flushed every few seconds, not a change of design.

## Open questions

- **Re-ranking facts.** `entity_facts` and `timeline` order by time today, and a time-ordered answer reordered by use stops being a timeline. Whether some fact read should use the score is left until the fields have been seen in use.
- **A re-parsed document's chunks.** `replace_chunks` supersedes every chunk and writes new ids, so chunk rows are lost on re-parse and the chunk falls back to its document's row. Carrying them by content hash is possible and not designed.
- **A corrected statement.** A correction inserts a new row with a new id; whether the statement's salience follows `supersedes` is undecided.
- **A closed statement and its typed rows.** Whether `materialize` carries a closed statement's end onto the typed rows built from it, or the sweep must close those rows itself, is checked in the first cut, not assumed here.
- **Feedback over MCP.** The tools stay read-only; whether an MCP client should boost or penalize through a tool, behind the nod gate of 0015, is not decided.
