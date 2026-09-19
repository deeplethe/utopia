# DRAFT — Sourced criteria are reviewable proposals before executable rules

**Status: DRAFT / DESIGN_REQUIRED. Not approved or implemented.** Related: issue 507; ADR 0015, 0021, 0029, 0030, 0032. This proposes a narrow amendment to ADR 0021's prohibition on model-proposed rules; its prohibition on model-invented or self-approved executable criteria remains intact. No new ADR number is claimed.

## Proposed first slice

A model may report that a specific accessible document passage states a general criterion. Persisting or displaying that report must not insert into `attribute_rules`, enable a rule, run materialization, or lift the mood exclusion on open statements.

Limit initial candidates to one existing subject class, existing same-entity numeric attribute(s), constant comparison(s), and one existing typing or constant-attribute conclusion. Start with the single condition `scratch_length > 2 mm` for Product, concluding NeedsReinspection. Preserve exact `>` semantics. OR, computed expressions, cross-entity traversal and aggregation need not be included merely because portions of the engine support them. No arbitrary SQL, scripts, regex programs or execution of source instructions.

One-off events are facts, not criteria. Exceptions, negation scope and nested conditions that cannot be represented losslessly remain unresolved for manual handling with the full original passage; never discard “unless”. Missing/unknown property, conclusion class or units remain unresolved, with no auto-created ontology elements. Unit handling should initially require an explicit compatible canonical unit; any conversion policy requires its own stated validation rules.

## Storage and identity — proposed, not an existing API

Prefer a separate rule-proposal store with explicit nonexecuting states over overloading `pending_facts` or putting disabled drafts into the live rule table. Reuse pending-fact permission, rejection and audit patterns. A dedicated table/migration is a proposal requiring maintainer approval, not a settled implementation requirement.

Record KB, proposed structured interpretation, unresolved terms/reasons, extraction provenance, document ID, reviewed document version, chunk ID, exact quote and server-verified span/content identity. Snapshot the reviewed version because chunk reuse can update `doc_version`. Store proposer identity separately from adopter identity. Names alone are not identity; an idempotency key should bind source identity/version plus normalized interpretation, with a documented policy for repeated extraction and materially changed source text. Rejection must not be undone by re-extraction of the same candidate.

Do not overload `fact_derivations`: those are the instance readings that make an application of a rule true. Rule-to-criterion evidence is a separate provenance edge. The UI should show both “who/what authorized this criterion” and “which instance readings satisfied it”.

## Adoption and permissions — proposed

Require an authorized human Editor for adoption, matching current rule-authoring permissions. Recheck KB membership, source access and current source state at display and adoption time; stored quote snapshots must not bypass access revocation. Model tools can propose, never approve themselves.

Adoption must atomically validate the reviewed proposal revision and vocabulary/unit mapping, create the ordinary executable rule through the same validation semantics as `business_rules::create`, link its provenance and reviewer, and mark the proposal adopted. Duplicate adoption returns the prior result. Concurrent source/ontology edits must cause an explicit stale/review-required result, not silently adopt an unseen revision. Current `business_rules::create` owns its transaction, so an implementation may need a scoped transaction-capable internal helper; do not bolt on non-atomic dual writes.

The review UI shows original passage, exact structured interpretation, unresolved items, applicable subject scope, and estimated effects. `rule_routes::run_now` is not a preview: it materializes real data. Build any nonpersisting preview around the existing evaluator without saving an executable rule; report cap/interval/proof limitations and make clear that preview is not approval. After adoption, use the ordinary inference schedule/run path, keeping asserted facts/types untouched.

## Source authority after adoption — decision required

Two coherent policies exist; this draft does **not** select one:

| Policy | Meaning and consequence |
|---|---|
| Revocable source projection | Approved rule remains dependent on live source support. Source change/deletion suspends it or requires reapproval; recomputation retires consequences. Must explain authority when multiple sources support one rule. |
| Human-assumed authored rule | Approval creates a human-authorized rule whose origin is the passage. Source change/deletion flags review but does not automatically rescind the human decision. Access restrictions still redact unavailable evidence. |

Maintainers must choose behavior for pending source update, pending source deletion/access loss, approved source update/deletion, human rule edits and rule withdrawal. Pending stale proposals should not be silently adopted; immutable reviewed history and present-day access checks serve different purposes. Human edits must create distinguishable revision history rather than implying the original passage said the edited rule.

Prefer disable-and-recompute for withdrawal when retaining rule history is required; the existing delete path cascades direct derived rows and is not archival retirement. Reuse the existing reasoner for withdrawing no-longer-wanted results, including chained proofs and temporal intervals. Do not delete asserted facts or rewrite entities' asserted types. Decide how and when dependent chains are recomputed rather than assuming FK cascade covers every downstream proof.

## Acceptance gates before implementation can be called complete

1. Approved ADR resolving authority, identity, unit policy, state transitions, permissions and adoption transaction.
2. Fixture with Product, NeedsReinspection and a known mm attribute; a general criterion proposes, a one-off narrative does not, and an exception stays unresolved.
3. No derived effect before approval; exact threshold boundaries and missing-reading controls after adoption.
4. Atomic/idempotent adoption, durable rejection, source-version races and same-KB evidence validation.
5. Source edits/deletion/access changes follow the chosen policy; manual edits preserve history; withdrawal uses existing temporal/proof lifecycle.
6. Regression coverage for ordinary authored rules, chaining, computed rules, asserted precedence and mood exclusion. Report existing test runs separately from new proposal/extraction evaluation.

Until those decisions are approved, deliver the audit, synthetic examples/evaluation plan and existing-engine validation only. Do not claim issue 507 is implemented or that sourced rules are authorized to execute automatically.

This document is a discussion proposal, not an accepted ADR. No implementation, migration or public API is included. After the open decisions are resolved, the accepted decision can be numbered under docs/decisions.
