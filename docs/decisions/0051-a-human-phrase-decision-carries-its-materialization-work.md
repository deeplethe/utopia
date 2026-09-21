# 0051 · A human phrase decision carries its materialization work

- **Status**: proposed; domain contract pending review. Shared store refactors and real regressions only; no production job kind or HTTP/UI change.
- **Written**: 2026-09-21
- **Related**: [0044](0044-the-ontology-is-a-view-over-what-documents-say.md); [PR #841](https://github.com/deeplethe/utopia/pull/841).

## Problem

A human binding may commit after a running aligner's final read. Depending on that aligner's late recheck can therefore leave the accepted decision without a projection. The proposed delivery unit is the accepted decision and its own durable work, rather than a guess that another running job will cover it.

## Decision requested

Prefer one durable materialization-only job for every accepted human phrase
binding, committed in the **same transaction** as that binding. The job reads the
current bindings; it never replays the old decision. Its payload needs KB identity,
not an old property/status. Do not suppress delivery because another job is running.

Return an honest saved/accepted response with job ID (prefer HTTP 202), rather than
inventing `typed: {added:0,...}`. Before production wiring, agree a minimal authorized
KB/job-kind status read and UI completion/failure behavior. Returning a job ID alone
does not provide those surfaces. The present route still materializes synchronously.

A job acquires the existing `typed_materialize` transaction advisory lock using
try-lock, then executes the original materialization body on that same connection.
Busy rolls back and enters the existing Deferred path; it is not a successful job.
Commit projection before acknowledging the job. Use normal finite failure budgets
and the existing bounded deferral window/requeue surface; no infinite hidden retry.

## Why a late decision is not lost

For every committed decision D there is a durable J_D committed with it. J_D is only
visible after D commits. Reading after acquiring the materialization lock observes
previously committed bindings under READ COMMITTED. If an older worker already did
its final read, J_D remains independently queued. If jobs run out of order, both
read the latest bindings instead of restoring old payload values. Once a finite
sequence of decisions stops, a successfully processed follow-up converges to the
last binding, assuming database/worker availability and eventual lock acquisition.

This does not promise a separate historical projection for every intermediate
click, a single snapshot across the whole multi-statement recompute, or progress
through permanent failures. Existing human priority and statement/evidence/temporal
semantics belong to the reused materializer, not the queue.

## Reusable code and regression evidence

The production `phrase_bindings::decide` calls `decide_on`; production `materialize` calls private `materialize_in_tx`. They share the existing SQL and transaction body with tests. No unused public try-lock entry point is exported. Busy orchestration belongs in module-local `cfg(test)` code, while normal recomputation uses the existing public materializer.

The retained integration target is [`human_phrase_materialization_delivery`](../../crates/utopia-store/tests/human_phrase_materialization_delivery.rs). Its module header documents opt-in execution on a dedicated, otherwise idle database, including real worker startup and OS subprocess termination. Busy coverage is in `materialize`'s module-local tests. These test adapters do not register a production handler.

Historical evidence at `e83f015f9a3949e53b1ae849b8d6dad0e2c4546e` on Linux / PostgreSQL 16.15 comprised two explicit parent tests and three actual killed subprocesses: before decision/job commit, after acceptance commit, and after projection commit before ack. Enqueue-helper failure rolled back both rows; Busy deferred the same job and released a two-connection pool; late arrivals, reverse processing and duplicates converged; actual worker startup reclaimed running work; exhausted deferral became visible failed and scoped requeue recovered it. The enqueue failure is helper-boundary injection, not a disk failure at COMMIT.

Three historical runtime mutations were rejected: splitting decision and enqueue transactions left an orphan; acknowledging Busy marked unfinished work done; adding a model prerequisite stopped pure recomputation. These results concern real store behavior but do not establish an asynchronous production route. The renamed tests preserve those assertions; new validation must be reported against its own head rather than reusing these counts.

## Alternatives

A late recheck cannot cover a decision committed after that check. Skipping enqueue when a worker is running loses this independent delivery obligation. Replaying a decision's old property payload can overwrite a later human choice. Blocking on the materialization lock retains scarce connections; a bounded Deferred outcome preserves work without claiming completion. A global lease/recovery redesign would expand the present single-process queue contract and is outside this proposal.

## Measured cost, not a throughput claim

One Linux run on a 100-open-statement graph, two request-pool connections:

| Decisions | Jobs/recomputations | Total accept ms | Max accept µs | Total convergence ms |
|---|---|---|---|---|
| 1 | 1 | 0 | 985 | 142 |
| 10 | 10 | 10 | 1770 | 58 |
| 100 | 100 | 75 | 1036 | 593 |

The first pass creates projections; later passes are largely no-ops. Times are
observations, not a percentile benchmark or a maximum latency guarantee. Production
large graphs and concurrent ingestion were not measured. The cost can be N full
recomputations for N decisions. Optimize only with a separately tested finite set of
covered job IDs, never with “one is running, so skip enqueue.”

## Remaining production acceptance and recovery limits

The current queue assumes **one server process**: startup requeues all running jobs.
This experiment does not establish safe multi-instance ownership. A mark_done write
failure can leave running until restart; this is not live lease recovery. An actual
kill inside a partly written materialization transaction, failed ack persistence,
notification-loss polling, old-aligner/new-handler overlap, and production route
permissions/status reads/UI E2E remain explicit acceptance work. Existing temporal
and evidence tests must pass after any extraction; the experiment is not a substitute.

After contract approval, wire the route's existing authorization and binding lookup
to a same-transaction decision+job function; register the pure handler in main;
add job status authorization and completion/failure events; update Review and both
languages. Keep #828's kind-word lock timeout isolated. Do not reuse align_phrases,
whose model dependency, busy guard and late recheck are a different contract.

Rollback first stops accepting new jobs of this kind, drains or explicitly retains
outstanding jobs, then returns to the old binary. Old workers cannot silently drop
an unknown kind. Keep failed work visible; never mark outstanding jobs done just to
make rollback clean. External actions (#530) must not use this retry/recovery path.
