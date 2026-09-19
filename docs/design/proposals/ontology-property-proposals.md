# DRAFT: evidence-backed relationship proposals from open signatures

Status: PROPOSED / DESIGN_REQUIRED. No migration, endpoint or approved behavior is implied. Related to ADR 0044 and #725.

Invariant: an unapproved proposal changes no ontology, binding, entity classification or typed fact. Evidence is scoped, attributable, current at adoption and separated from interpretation. Start only with known endpoint classes and simple relation signatures; no new classes, unit conversion or rules.

## Slices and reuse

A. Correct deterministic statement counts first (independent bug fix). Then design an internal gap view that combines signatures, bindings, current candidate compatibility and failure/rejection state. Enumerate missing-binding signatures too. Return distinct statement/document counts, stable statement IDs and source snapshots (document/chunk/version/character span). Do not expose unscoped raw excerpts through a new public endpoint by default.

B. Add an explicitly requested bounded model job using the existing client/retry/budget system. Allowed outcomes: reuse an existing property, propose one relation, insufficient evidence, or no ontology gap. Validate every referenced class/property/statement/source against the job's KB and permission snapshot. Recompute quotes from trusted source snapshots; treat source text as data, reject fabricated IDs/spans and do not execute arbitrary model tools/URLs/code. Candidate absence in retrieved top-k is not ontology absence. Keep per-item parsing failures visible and separate from no-proposal results.

C. Extend the existing persisted proposal/rejection lifecycle with a versioned evidence payload and reviewed direction per signature. Require a human Editor to adopt. In one reviewed transaction, lock the proposal, recheck source permission/version, ontology revision and uniqueness, invoke normal ontology writers, write audit/adoption linkage and persist recomputation work. Concurrent or repeated adoption returns the same result or an explicit stale/conflict result; do not swallow uniqueness errors as success. Another person's same-meaning property requires review/reuse, not automatic duplicate creation.

## Decisions to land before implementation

Agree on proposal identity beyond section/key (signature set + input revision + source snapshot), explicit gap reasons, source access policy, rejected-proposal suppression and when changed evidence may reopen it. Choose an ontology writer transaction API shared with R3, and lifecycle for proposal edit/withdrawal. Existing `decide_proposal` status update alone is not an atomic adoption transaction. Do not promise full undo where ontology deletion is blocked by use.

Use migration additions only, preserve existing rejection rows and do not mark historical proposals as reviewed under current inputs. Existing unversioned proposals may remain readable but require revalidation. Record approved / ontology written / reevaluation queued / typed view ready as distinct states. Background recomputation reuses original statements; do not re-extract documents or synchronously run whole-KB materialization in the HTTP request.

## Validation and cost

Use frozen three-domain cases in ontology-property-cases.jsonl, two runs/configuration and calibrated human/gold review. Report wrong additions, correct reuse, directions, typed answer sets, unrelated-fact changes, request/token/retry counts. Scripted deterministic fixtures can establish state and authorization, not model quality. Test multi-evidence counts, quoted Chinese offsets, deleted/superseded evidence, injections, cross-KB IDs, source changes before adoption, concurrent adoption, human rejection and worker restart. No automatic confidence approval.

Rejected alternatives: every none triggers property creation; a second generic agent platform; invented semantic word lists; writing model JSON straight into SQL; interpreting approved as fully materialized. Rollback stops proposal generation and preserves review history; it does not delete accepted facts or silently reverse human decisions.

This document is a discussion proposal, not an accepted ADR. No implementation, migration or public API is included. After the open decisions are resolved, the accepted decision can be numbered under docs/decisions.
