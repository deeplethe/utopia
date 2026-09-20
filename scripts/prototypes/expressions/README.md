# Proposed declaration policy for the expression picker

Status: **proposed; no production validation or editor changes**. Refs #488.
ADR 0032 requires unit/datatype checks, but does not define what a missing unit
means or authorize implicit conversion. The separate expression-operand API fix
restores an already described capability; this proposal deliberately does not
silently tighten that API's accepted computed definitions.

## Decision requested

Approve a conservative write-time declaration subset: numeric `number` attributes,
exact known units, no conversion, and missing/ambiguous units as **unknown** rather
than dimensionless. Decide whether the exact string `1` is the explicit unitless
representation. Prefer enabling same-unit addition/subtraction and numeric factor
scaling first; ratios need the explicit-unitless decision for their target.

The executable policy uses USD/EUR/m/kg/s as experimental known declarations, not
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

## Editor experiment and evidence

`scripts/prototypes/expressions/` holds a standalone picker and policy module:

```sh
node --test scripts/prototypes/expressions/policy.test.mjs
python -m http.server 5190 --bind 127.0.0.1 --directory scripts/prototypes/expressions
# Open http://127.0.0.1:5190; all saves are local model state, never API requests.
```

For the optional DB lock experiment, install psycopg[binary]==3.2.10 in an isolated
venv and run `python -m unittest -v test_declaration_lock` from that directory with
UTOPIA_DATABASE_URL pointing to an isolated PG16 test database. It creates/drops a
random `unit_model_*` schema and releases the writer even on assertion failure.

Linux: **10 Node tests and 1 PostgreSQL lock experiment passed**. Tests cover the
operation table, unknown versus unitless, nonnumeric/missing attributes, non-finite
and unfinished constants, UUID-independent label changes, grouped-condition
preservation, metadata-only patches, mixed/unknown legacy shapes, constant-only
rejection and depth. Root depth is zero: four edges/five nodes is accepted by the
API and parser; the independent Expr::depth method uses leaf depth one.

A headless Chromium interaction check passed local save/reopen, invalid constant,
failed-save draft retention/retry, and unknown-definition metadata-only save. This
is **not real-backend E2E**, is not the final localized/accessibility-reviewed UI,
and does not replace B1. The toy attribute identifiers illustrate tree identity;
production UUID and same-KB validation remain the API's responsibility.

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
