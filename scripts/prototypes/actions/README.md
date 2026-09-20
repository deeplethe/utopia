# Proposed amendment to 0034: an action attempt keeps its identity

Status: **proposed, executable protocol experiment only**. This does not supersede
0034, add production tables/routes, or enable action sending. Refs #530.

0034's synchronous “run, then one row” leaves two facts indistinguishable: a call
that was never sent, and a call whose remote effect happened but whose response
was lost. A client retry then risks repeating a non-idempotent action. The example
is not hypothetical: the loopback endpoint in this experiment increments its
side-effect counter, waits on a barrier, and drops the connection before replying.
The observed result is one remote effect and an unknown local outcome. Replaying
the same execution request does not increase the counter.

## Decision requested

Approve a stable execution request ID, revision-bound preview, and a durable
single-attempt gate before implementing the registry's sender. Prefer **no automatic
retry, no redirects, and no recovery takeover** in this first manual cut. This
narrows the redirect behavior mentioned in 0034 and must be accepted explicitly.

The ID belongs to one explicit user operation, scoped to actor, action and KB (or
registry-test scope). Equal inputs with a new ID are a new operation; an existing
ID with different inputs/revision is a conflict. Replays reauthorize before reading
back a run. PostgreSQL's ordinary UNIQUE with a nullable scope is insufficient:
the experiment uses UNIQUE NULLS NOT DISTINCT, supported by the project's PG16.
The model has one action; production uniqueness must also include action identity.

Every definition edit and credential rotation increments revision in the same
transaction. Preview returns revision and a non-secret rendered request without a
run or network request. The execution service renders from the same revision and
validated arguments, never a client-supplied URL/body/header set.

## State and dispatch authority

- `prepared`: intent persisted, no dispatch grant yet; replays only read it.
- `dispatching`: a unique token and authorization gate committed. Only that original
  flow can call send after commit confirmation. A lost commit acknowledgement means
  do not send, even if a later read finds dispatching.
- `not_sent`: an atomic prepared expiration/cancellation won before dispatch.
- `response_received`: observed HTTP status, independently of 2xx and body capture.
- `outcome_unknown`: dispatch may have happened; no durable observation establishes
  a response. Recovery never changes this to a sendable state.

After a short authorization/revision check and CAS transaction, no database locks
remain held across the network. Changes to permissions before this gate reject;
a revocation after it cannot recall a request. Row-lock order for action, grants,
parameters and run must be specified in the implementation. The experiment locks
one simplified definition row, **not** Utopia's production permission tables.

Keep HTTP status, capture state (complete/truncated/read_error/absent), safe excerpt,
original dispatch token, dispatch/observation/finish timestamps and immutable safe
request snapshot separately. A late response can update unknown using the original
token; it never grants another send. The model exercises state/token/capture, not
the complete proposed audit schema or its timestamps.

HTTP 202 is a response, not proof of business completion. A 500 may follow a remote
side effect too. A response body failure must not erase already observed headers.
A final database-write failure may leave dispatching; retry only persisting the
observation if it remains available, never the external call.

## What was executed

Run from `scripts/prototypes/actions` with an **isolated PostgreSQL 16 database**:

```sh
python -m venv .venv
.venv/bin/pip install -r requirements.txt
export UTOPIA_DATABASE_URL='postgres://.../isolated_test_database'
.venv/bin/python -m unittest -v test_protocol
```

The model creates/drops only a random `action_model_*` schema. The HTTP server binds
loopback and the sender's target is hard-coded loopback; it cannot be repurposed as
a production managed sender. The test dependency is isolated, not a Utopia runtime
dependency. Each test shuts down its server and drops its schema.

Linux Python 3.13 / PostgreSQL 16: **19 tests passed**. They cover concurrent duplicate
registry submissions, nullable uniqueness, KB scope, input conflict, authorization
and revision changes, insert failure, expiry/CAS competition, lost commit ack,
remote-effect/drop, 200/202/400/500, body timeout/cap, failed observation persistence,
late token observation, no redirect follow, and proxy environment isolation of this
loopback client. A separate test kills actual Python subprocesses after prepared,
after dispatch, and after the remote effect, then retries the same operation.

Two mutations were rejected: ordinary nullable UNIQUE produces multiple dispatches;
allowing a replay to take over prepared sends an operation whose creator was lost.
Both fail their named tests, rather than merely failing to compile.

## Not established by this model

No production RBAC, seal/auth handling, templating, DNS pinning, direct-client policy,
production 15-second/1-MB/4-KB limits, UI, deletion retention or production migrations
were implemented or tested here. Model caps are deliberately tiny to trigger faults.
The ordinary worker has no model registration; do not copy its running-job replay
policy into action recovery. An unknown action is not a failed internal recompute.

After approval, implement dedicated action tables and revision/grants/preview first,
then a managed sender with no retries/redirects and audited proxy policy, then UI and
real-backend authorization/E2E. Do not reuse `client_for().post()` without examining
its redirect behavior. Secrets belong in a sealed auth block; both request snapshots
and echoed response excerpts need redaction. Logs retain unknown outcomes. Rollback
first disables new dispatch, preserves runs, and cannot reverse remote effects.

The experiment supports an **at-most-once application dispatch attempt**, not external
exactly-once execution, packet-level guarantees or arbitrary remote business semantics.
