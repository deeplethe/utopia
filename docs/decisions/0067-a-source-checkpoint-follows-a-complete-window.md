# A source checkpoint follows a complete window

Proposed 2026-10-13 (#1212); open: maintainer discussion, continuation storage and implementation.

Discussion: [bounded continuation proposal and database reproduction](https://github.com/deeplethe/utopia/issues/1212#issuecomment-6137283600).

## Context

[Issue #1212](https://github.com/deeplethe/utopia/issues/1212) remains present on
`dev` at `259931ac702623351c2ed4e6487474790e57a6b0`. Custom, GitHub and Jira
consume at most 200 records and nevertheless finish the run as `ok`.
`last_successful_sync_start` then treats that run's start as a covered lower
bound. The unconsumed records can disappear from subsequent incremental reads.

[PR #794](https://github.com/deeplethe/utopia/pull/794) made failed attempts and
updates during a run safe; it explicitly left the batch limit unresolved.
[PR #1061](https://github.com/deeplethe/utopia/pull/1061) requires each selected
GitHub issue to retain its complete discussion when its document is replaced.

Keeping the old lower bound alone does not make progress: the next attempt
selects the same first 200 records. Charging only changed documents against
the limit helps custom responses but still cannot reach beyond GitHub's ten
100-record pages or Jira's ten 50-record pages. It also makes GitHub replay
unbounded: checking content requires reading the previous issues' discussions.

## Proposed decision, pending discussion

Add one private `sources.sync_state JSONB` column. Do not add it to the
serialized `Source` model or place it in user-editable source configuration.
It contains a completed `checkpoint` and an optional active window:

- The fixed `since` used to open that window.
- The first batch's database start time, `window_started_at`.
- The provider's next page or `startAt`, the fetched page awaiting processing,
  its consumption position, and whether the listing is exhausted.
- The source configuration identity, so a changed endpoint, repository or
  project invalidates continuation and starts a safe new scan.

Custom has no pagination or source-update timestamp in its existing protocol.
Preserve the first response's `items` and `deleted` snapshot and consume that
snapshot in batches. `doc_time` remains document time; it never selects a
continuation position or advances the checkpoint. A custom endpoint must still
return the complete requested window under its existing contract.

GitHub and Jira fetch and persist one listing page at a time. Keep the existing
200-record processing budget, the ten-listing-page budget and the existing
per-issue discussion budget. Persist a fetched page before ingesting its items.
Save consumption progress at batch boundaries; a crash may replay the current
batch. Reuse `ingest_item` and the existing transactional document-version and
processing-job writes to make that replay harmless.

Use GitHub's `sort=created&direction=asc` and Jira's
`ORDER BY created ASC, key ASC`. Sorting by update time moves already visited
members behind later offsets when they change, potentially skipping an older
unvisited member. Fixed creation order avoids that movement for ordinary
updates. A record entering the incremental set during the window may appear
before the saved offset, but its update belongs to the next window as well.

An unfinished batch must retain the previous successful checkpoint and report
its committed counts. The smallest compatible status option uses the existing
`failed` status with an explicit batch-limit message, without raising an error
alert or making the queue retry a completed batch. A separate `partial` status
would express this better but changes the public status contract, database
checks, web types and translations. This choice requires discussion before
implementation.

Only after the snapshot and listing are exhausted does a transaction set
`checkpoint = window_started_at` and clear the active window. The checkpoint
uses the first batch's start, not the last batch's start or finish. Updates
during any of the intervening batches therefore remain eligible next time.
Sync-run audit rows retain their actual per-batch start times. The successful
lower bound no longer depends on the retention of the latest 50 audit rows.

## Pagination limitations to resolve

An offset alone is insufficient when an upstream deletion, project transfer
or permission change removes a member before the saved page. Keep a page
boundary identity and re-read the boundary when resuming; if it has drifted,
restart from the fixed old `since` and replay through existing deduplication.
This must be tested before claiming coverage. It is not an upstream atomic
snapshot. GitHub also does not expose a second sorting key for equal creation
times; the resumed boundary must be checked rather than assuming stable ties.

The existing GitHub discussion reader silently stops at ten full pages. Source
listing continuation must not turn a truncated discussion into a replacement
document or a complete success. Explicitly detect this condition and retain
the item for retry; supporting continuation inside unusually large individual
discussions needs its own bounded state, rather than removing that budget.

Custom snapshots may be large. Saving progress once per batch avoids rewriting
the entire JSON response per item, but storage size and configuration access
checks still need review. There is no claim that the current custom protocol
can recover records the endpoint itself silently omits.

## Upgrade and recovery

Old `ok` rows cannot distinguish a complete window from a truncated one.
Initialize the new state for these three source kinds without a successful
lower bound, causing one bounded, resumable full recovery scan after upgrade.
Do not delete documents, versions, jobs or audit history. Existing identity and
content deduplication preserves unchanged documents during recovery. After that
scan, resume normal incremental reads; this is not permanent full polling.

Before this change ships, an operator can rewind the successful run history for
an affected source only while its scheduler and worker are idle. That merely
re-exposes the lost window: the old binary still stops at the first 200 records,
so rewinding alone is not a complete recovery procedure.

## Rejected approaches

- Marking a capped attempt successful: advances over unread content.
- Keeping `since` but repeatedly taking the same prefix: does not finish.
- Counting only writes while retaining the fixed first ten pages: stalls on a
  larger result set and increases GitHub detail requests without a bound.
- Advancing from maximum `doc_time`: custom has no source-update clock, and ties
  can span a batch or page boundary.
- Raising or removing the limits, or disabling incremental sync permanently:
  changes the resource contract instead of implementing continuation.
- Hiding progress in `config`, error strings or job payload history: exposes or
  overloads unrelated state, and makes retention and configuration changes
  determine correctness.

## Acceptance

Reuse the existing wiremock and database fixtures with required PostgreSQL and
pgvector. Cover 201 records, several batches, more than each reader's ten-page
limit, a previously imported prefix, equal update timestamps, failure after
committed items, and changes during multiple batches. Assert actual `since`,
JQL and page parameters, completed checkpoints, resource budgets, stable
document identities, version counts and processing-job counts. Exercise every
changed provider branch, including boundary drift and crash replay.

This record is a proposal. No continuation schema or production implementation
has been written or represented as accepted.
