# Proposed delivery contract for human phrase decisions

Status: **isolated prototype; not a production #800 implementation**. No job kind
is registered and no HTTP/UI response changes. Refs #800. This branch exists to
review the protocol before introducing the public API/job contract required by
CONTRIBUTING. Do not deploy or merge the helpers as a standalone solution.

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

## Executable evidence

On the isolated branch, `phrase_bindings::decide_on` factors the existing single SQL
write so the test can compose it with `enqueue_with_max_attempts_tx`. The existing
pool caller remains. `materialize_in_tx` mechanically shares the original body;
`try_materialize` uses the exact existing lock key. No production caller uses the
new entry points. These are experiment support, pending the contract above.

Run against a **dedicated, otherwise idle test database** (the actual worker recovery
test consumes jobs; never point it at another session's database):

```sh
export UTOPIA_DATABASE_URL='postgres://.../dedicated_delivery_experiment'
export UTOPIA_TEST_REQUIRE_DB=1
cargo test --locked -p utopia-store --test phrase_delivery_prototype -- --ignored --skip crash_child --test-threads=1 --nocapture
```

Linux PostgreSQL 16: **two experiment tests passed**. The ignored `crash_child` entry
is an explicit subprocess probe; both parent tests are also opt-in to avoid consuming unrelated test jobs in a shared test database. The command above runs the parents explicitly; the parent invokes it three times with per-fixture
identifiers and kills/waits for those children. It is not a silently skipped crash
case. Evidence includes:

- The real enqueue helper rejects an invalid budget after the binding write; neither
  binding nor job commits. This is fault injection at the helper boundary, not a
  simulated disk failure during COMMIT.
- A held real advisory lock does not stall acceptance; the same job is Deferred and
  the two-connection pool remains available through repeated busy attempts.
- A decision after an old projection's final read has its own job; reverse-order and
  duplicate processing preserve final none and the open statement.
- Actual `run_worker` startup reclaims a running job and idempotently recomputes; no
  model path is present in the prototype handler.
- Actual OS subprocess termination before commit rolls back both rows; termination
  after accept preserves the queued job; termination after projection commit before
  ack preserves a running job and recomputation creates no duplicate typed fact.
- Exhausted deferral/failure budget becomes visible failed and explicit scoped
  requeue can recover. No sleep guesses which SQL lock is held.

Three runtime mutations failed their specific assertions: writing the binding via
an independent pool transaction left an orphan decision; treating Busy as success
marked the job done; adding a model-config prerequisite prevented pure recomputation.
Restoring the prototype passes both experiments again.

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
