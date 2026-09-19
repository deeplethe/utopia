# DRAFT: bind alignment decisions to the ontology snapshot they read

Status: PROPOSED / DESIGN_REQUIRED, not accepted, no ADR number reserved. Related: #795, #775, #792, #797, #798, #800 and ADR 0044. [CONTRIBUTING](../../../CONTRIBUTING.md) requires issue discussion and a landed ADR before changing the data model/ontology contract/public API. #795 has no discussion response at this audit.

## Invariant and scope

A response to ontology revision R may be accepted as current only while R remains current. A later response timestamp does not establish input freshness. Cover class/property definitions, candidate membership, parent edges, domain/range and other consumed semantic flags. Preserve human binding and entity decisions. Do not claim prompt/model versions or changed examples are solved. Endpoint class changes require their own generation or participation in the acceptance protocol; a final unprotected SELECT alone is insufficient.

## Proposed protocol

1. Add a per-KB monotonic ontology revision and nullable evaluated revision to automatic bindings. All semantic writers in alignment-input-writers.md must lock the same KB revision row and change semantic rows, advance revision and durably request work within ONE transaction. Store helpers must accept the transaction connection; route-only bumps are insufficient. No-op changes should not bump once semantic equality is established; conservative bumps are acceptable initially.
2. Read revision and all candidate definitions/hierarchy in one short REPEATABLE READ, READ ONLY transaction on one connection. PostgreSQL Read Committed alone is insufficient across multiple queries. Materialize immutable owned inputs, then close the transaction before embedding/model requests. Retrieval must not fetch a fresh unrelated definition after closing this snapshot; either derive retrieval from the captured data or validate its IDs against it.
3. At acceptance, use a short Read Committed write transaction, lock the current revision row before binding/entity rows, compare to captured revision, and check current signature membership under the agreed classification-generation protocol. On mismatch, persist a requeue/dirty marker but no old-result binding/application. On equality, reuse the existing human-precedence WHERE and type decision/application atomicity on that same connection.
4. Accepted automatic bindings retain evaluated revision. Stale queries compare it to current revision, including negative bindings. Historical null revisions remain stale rather than being filled with the deployment's current revision. Human decisions are preserved and never reinterpreted as automatic; changes requiring human review remain visible.
5. Work scheduling must close the final-check/job-completion gap: persist desired generation with edits and advance completed generation only for the consumed input. The worker/job completion transaction must leave runnable work whenever desired > completed. Reuse the existing jobs system with a reviewed generation extension; enqueue-after-commit by itself is insufficient. Conflict retries are debounced and capped per run, leaving persistent work for subsequent runs rather than spinning or repeatedly buying model calls.

## Projection boundary and lock order

Lock order proposal: KB revision -> binding -> affected entities/facts; never acquire revision after holding materialization/fact locks. Existing materialization serializes per KB and can be long. Do not silently nest revision locks inside it. For phrase materialization choose explicitly between (a) holding the revision guard during an entire recompute, with measured blocking cost, and (b) staging output by generation and atomically publishing only if still current. (b) is preferred for responsiveness but is a separate implementation slice requiring schema/design approval. Until selected and tested, do not claim end-to-end current typed projections. A type-only first cut may be accepted if titled/scoped accordingly, with the phrase limitation retained.

No connection/transaction/revision lock spans model calls. All helpers called under a transaction use that connection; no second checkout. Set bounded lock waiting in interactive paths with retryable conflict responses; tests must cover one-/two-connection pools, cancellation and rollback. Across KBs lock keys remain distinct.

## Migration, compatibility and rollback

Append a migration after checking dev/main/in-flight numbers; no number selected now. Add nullable evaluated revision without asserting historical freshness. Keep old versions readable during rolling deployment but do not enable revision-based guarantees while any old writer can bypass bumps. Roll out writers first behind an activation boundary, then readers/acceptance and a bounded reevaluation job. Do not delete old bindings or re-extract original documents. A rollback can stop new automatic acceptance while keeping human decisions and evidence; removing revision columns while new writers are live is unsafe.

## Alternatives and cost

Rejected: response wall clocks, more precise now(), candidate-then-revision outside one snapshot, check-then-write outside mutual exclusion, a worker-only advisory lock, all-KB global locks, long model transactions, or deleting caches. Content hashing can avoid irrelevant changes but requires complete canonical dependency coverage and atomic validation; not a shortcut. Coarse KB revision conservatively rejects unrelated edits within the same KB and can increase requests. Record conflict counts, reconsidered signatures, real batched request/token counts, queue age and lock waits; do not claim cost reduction without measurements.

## Acceptance and open decisions

Before code: decide type-only first cut versus type+phrase, projection publication policy, endpoint-class generation, which semantic fields advance revision, persistent scheduling contract, cosmetic-edit policy and historical backfill budget. Tests must deterministically edit between snapshot queries, between votes, and between validation/write; exercise imports/rollback, hierarchy/domain changes, humans, another KB, repeated edits, small pools and next-run recovery. Existing correct-behavior reproduction supplies only the first type-path failure. No new public endpoint is proposed yet.

## Database references checked for this draft

[PostgreSQL 16 transaction isolation](https://www.postgresql.org/docs/16/transaction-iso.html) confirms statement snapshots under Read Committed and stable transaction snapshots under Repeatable Read. [Explicit locking](https://www.postgresql.org/docs/16/explicit-locking.html) describes conflicting row locks and transaction lock lifetimes. These support the proposed protocol, not a claim that the repository already implements it.

This document is a discussion proposal, not an accepted ADR. No implementation, migration or public API is included. After the open decisions are resolved, the accepted decision can be numbered under docs/decisions.

## Reproduction evidence

The adjacent [test-only patch](alignment-input-version-regression.patch) applies to dev e894fa1d1f5d0bb823b61c252bbe9e8b8e61e066. In a separate checkout with a dedicated PostgreSQL database, apply it using `git apply docs/design/proposals/alignment-input-version-regression.patch`, then run `UTOPIA_TEST_REQUIRE_DB=1 cargo test --locked -p utopia-server input_contract_tests -- --nocapture` with UTOPIA_DATABASE_URL configured.

Linux Rust 1.98.1/PostgreSQL 16.15: one correct-behavior test failed with `old-definition replies were accepted as fresh after a normal ontology edit`; the unchanged-definition control passed. The local HTTP handler awaits the normal class edit before returning its first response; both captured votes see OLD rather than NEW. No timing sleep or paid model was used. Recovery assertions are not reached on the failing baseline. This patch is an explicitly expected-red reproduction artifact, not registered in the workspace by this documentation PR and not a bug fix.
