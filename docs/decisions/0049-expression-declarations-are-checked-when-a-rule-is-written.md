# 0049 · Expression declarations are checked when a rule is written

- **Status**: proposed; domain contract pending review. The opt-in web draft does not validate units or save rules.
- **Written**: 2026-09-21
- **Related**: [0032](0032-a-rule-computes-what-it-concludes.md); [PR #839](https://github.com/deeplethe/utopia/pull/839).

## Problem

0032 asks for datatype and unit checks but does not say whether absent declarations are dimensionless. A rule can otherwise subtract revenue declared in USD from cost declared in EUR and present a meaningless result. The existing expression API and metadata-only editor remain compatible; this proposal must not silently tighten their write contract.

## Decision requested

Approve a conservative write-time declaration subset: numeric `number` attributes,
exact known units, no conversion, and missing/ambiguous units as **unknown** rather
than dimensionless. Decide whether the exact string `1` is the explicit unitless
representation. Prefer enabling same-unit addition/subtraction and numeric factor
scaling first; ratios need the explicit-unitless decision for their target.

The historical policy model used USD/EUR/m/kg/s as experimental known declarations, not
an exhaustive units language. It treats $, ¥, %, basis points, Celsius and arbitrary
compound strings as unknown. The accepted allowlist must be agreed alongside `1`;
it must not infer aliases from labels, backfill empty units, or claim that matching
declarations normalize historic observations.

| Operation | Proposed accepted inputs | Result |
|---|---|---|
| add/subtract | equal known units, or both explicitly unitless | same unit |
| multiply | at least one unitless | other operand's unit |
| divide | unitless denominator, or identical known units | numerator, or unitless |
| constant | finite decimal number | unitless factor |

A bare `revenue(USD)-1` is rejected. A scalar legacy threshold remains governed by
its existing API contract; this policy applies to editing expressions, not a rewrite
of all stored conditions. Changing declarations later can invalidate assumptions:
this is a write-time check, not a new ontology lifecycle/revision system.

## Transaction boundary proposed for the API

Resolve references in the current KB, sort their UUIDs, read/lock the relevant
attribute declarations with `FOR SHARE`, validate, then write the rule in the same
short transaction. `FOR KEY SHARE` is insufficient for concurrent datatype/unit
updates. Metadata-only PATCH and existing enabled toggles do not rewrite/revalidate
legacy definitions. Deletion/foreign-base/permission checks remain server-owned.

The PostgreSQL experiment observes a real blocked declaration UPDATE via
`pg_blocking_pids`; the writer continues to read USD until commit, after which a
subsequent validator sees EUR. This establishes the proposed lock primitive, **not**
that production rule routes already perform it. Keep this separate from A0.

## Investigation and its limits

Historical evidence at `30a0da8ca06ce19325432cfc6be0a3cbfecb642d` on Linux, Node 22.23.2 and PostgreSQL 16.15: ten Node model tests and one isolated PostgreSQL lock experiment passed. The lock probe observed `pg_blocking_pids` for a concurrent **non-key** datatype/unit update: `FOR SHARE` blocked it until commit; `FOR KEY SHARE` did not. This establishes a lock primitive, not production route validation.

The historical standalone browser checked local model save/reopen, invalid constants and failed-save retention. It did not use Utopia APIs and supplies no evidence about a real picker at scale. Its scripts and page have been archived outside the repository, not translated into another executable policy. Model results are not tests of a production unit validator.

The unlisted `/kb/$kbId/expression-draft` route opts into the exploration in `web/`. It uses structured drafts and the existing UI controls with authenticated attributes and rules from the current knowledge base. Draft previews do not persist anything. Attribute declarations are displayed, not interpreted as an approved unit language. The depth question remains a usability decision: automated interaction checks can establish structure, search and focus behavior, but cannot supply a person's tolerance for nested editing.

## Alternatives and remaining decisions

Treating an empty unit as unitless would silently accept undeclared quantities. Inferring aliases or converting observations would introduce a separate normalization contract. A formula string would create a second representation beside the AST. Prefer the explicit three-state declaration model: known, explicitly unitless, unknown. The exact `1` spelling, known-unit allowlist, and whether to enable ratios in the first cut still need approval. USD/EUR/m/kg/s are investigation samples, not a shipped allowlist.

## Implementation after approval

Add server declaration validation in the short write transaction, then integrate
structured drafts into RulesPanel using the existing expression display and protected
metadata editor. Keep grouping, explicit scalar/expression modes, failed-save drafts,
KB switching, UUID selection and exact tree order. Reuse known/unknown shape checks;
never strip unknown keys to make a definition editable. Preview and save must use
the same validated AST. Add actual browser/API create-read-edit-read, concurrent
declaration updates and inference/premise/interval regressions before enabling it.

Rollback of UI retains B1. It does not delete existing rules. No new AST shapes,
relation paths, aggregate operators, formula runtime or unit conversion are proposed.

## Picker observations in this revision

Authenticated API-created bases with 30, 300 and 1000 attributes were read in full
(the ontology endpoint uses `fetch_all`, without pagination or a list cap). The
fixtures include duplicate labels, Chinese and English long labels, mixed declared
datatypes, and absent units. Browser checks built revenue minus cost and margin,
reopened real right-nested subtraction/division rules, edited an inner operand,
and retained invalid numeric text without manufacturing a value. Search reaches
the thousandth attribute by key, with keyboard selection and focus returning to
the picker. The four-edge bound keeps leaves editable; it does not flatten the tree.

At a 390 px viewport the editor remains within its container, including a depth-four
leaf. Nesting nevertheless makes a long vertical form: reaching an inner operand
requires scrolling. These are mechanical observations from automated Chrome, not a
human usability score or a decision that four levels are pleasant. Comparing with a
formula language remains outside this change. HTTP-read failure/retry and unknown
expression shape were checked using explicitly injected browser responses; they
are not claims that the backend accepted a future definition. No draft save route
is enabled, and the existing B1 metadata editor and dependency view are unchanged.
