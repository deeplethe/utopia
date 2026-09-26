# 0062 · The export says what is contested

- **Status**: accepted 2026-09-26 (#922) · implements [#564](https://github.com/deeplethe/utopia/issues/564) · precondition: the current-ontology invariant described under [The `open` invariant](#the-open-invariant)
- **Written**: 2026-09-25 (conventions in the [README](README.md))
- **Related**: [0020](0020-an-auditor-reads-it-without-us.md) (the export this extends); [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) (axiom violations); [0017](0017-a-contradiction-points-upstream.md) (`derived_contradiction`); [#550](https://github.com/deeplethe/utopia/issues/550) (the export is the supported machine-readable read contract); #612, #613, #618, #619 (the storage-side findings this record had to respect)

## Problem

The export contains the assertions themselves but not the epistemic state the base has already established about them. A downstream reader gets two evidence-backed statements — "Acme revenue = 100M" and "Acme revenue = 130M" — with their provenance and intervals, and nothing that says Utopia considers them contested. The reader's options are to treat them as ordinary independent facts, or to re-implement the conflict detection Utopia already owns. Both are wrong in the way 0020 names: the export is the read contract, and the read contract has to carry what the ledger knows.

## Decision

The export carries the two findings tables as two distinct resource classes. Both already exist; nothing here invents a new state machine, and the export stays read-only — it selects from `fact_conflicts` and `axiom_violations` inside the same read-only `REPEATABLE READ` snapshot as every other page, and never through `temporal::list_conflicts` or a detection run.

### Two classes, not one

`utopia:FactConflict` and `utopia:AxiomViolation` stay separate because they are different claims:

- A **fact conflict** says *two assertions disagree about the same ledger slot* — the ledger could not decide whether the new one closes the old one (`no_time`, `simultaneous`, `low_confidence`, `described_evidence`). Its adjudication is about the facts.
- An **axiom violation** says *the data contradicts a rule the ontology currently declares*. Its adjudication is about whether the error is in the data or in the definition.

Their resolution vocabularies are different for the same reason; flattening them into one class would force one column to mean two things.

### `utopia:FactConflict`

A conflict is addressable at `conflict:{uuid}` under the base namespace. It points **from** the conflict **to** the two statements by their existing fact IRIs — a consumer that ignores conflicts reads facts exactly as before:

```
<conflict> a utopia:FactConflict ;
    utopia:reason "simultaneous" ;                -- no_time | simultaneous | low_confidence | described_evidence
    utopia:priorStatement <fact/old> ;            -- the incumbent assertion
    utopia:incomingStatement <fact/new> ;         -- the arriving assertion
    utopia:status "open" ;                        -- open | resolved | withdrawn
    utopia:resolution "closed" ;                  -- only when resolved: closed | kept_both | rejected_new
    prov:generatedAtTime "…" ;                    -- created_at: when the conflict was recorded
    utopia:closedAt "…" .                         -- resolved_at, when set
```

`status` and `resolution` are contract vocabulary — `contest_integrity` validates stored values against the listed sets before the first byte is emitted, so the contract promises the listed values and nothing else. `withdrawn` (0051) is a status, not an adjudication: its `closedAt` is the moment one side was invalidated — which is when the conflict stopped applying — not a page-open timestamp. Rows keep their real timestamps because `GET /export` has no `as_of` parameter: "was this contested in March" is answered from the payload or not at all.

### `utopia:AxiomViolation`

A violation is addressable at `violation:{uuid}`:

```
<violation> a utopia:AxiomViolation ;
    utopia:kind "functional" ;                    -- the finding kinds below
    utopia:status "open" ;                        -- open | resolved
    utopia:resolution "accepted" ;                -- only when resolved
    utopia:onStatement <fact/a> ;                 -- the statements the finding is about
    utopia:onStatement <fact/b> ;
    utopia:evidencePath ( <fact/a> <fact/mid> <fact/b> ) ;  -- ordered, when the row carries a path
    utopia:onRelation <relation> ;                -- open rows only: the relation the criterion is declared on
    utopia:criterion owl:FunctionalProperty ;     -- open rows only: the criterion itself
    utopia:detectedAt "…" ;
    utopia:closedAt "…" .
```

The criterion link is what lets a reader re-derive the finding instead of trusting it. `kind` plus the relation names it exactly:

| kind | criterion |
|---|---|
| `self_loop` | `owl:IrreflexiveProperty` |
| `asymmetry` | `owl:AsymmetricProperty` |
| `cycle` | `owl:TransitiveProperty` |
| `functional` | `owl:FunctionalProperty` |
| `inverse_functional` | `owl:InverseFunctionalProperty` |
| `signature` | `rdfs:domain` and `rdfs:range`, together — the row does not retain which side broke, so the contract does not pretend |
| `derived_contradiction` | the OWL term named by the row's retained `detail.axiom`, on the relation `detail.predicate_id` |

Two rules keep the link honest:

- **Open rows only.** A violation resolved as `axiom_relaxed` (or `criterion_changed`) names an axiom that was removed or widened — linking it into the current vocabulary would assert the opposite of what happened. `fact_retracted` and `fact_closed` do not touch the ontology, but the ontology can have moved since; one rule that stays true beats three that need checking. Resolved rows carry kind, resolution and timestamps and no criterion link.
- **The ordered path is the evidence for a cycle.** `path` is emitted as an `rdf:List` of fact IRIs in traversal order; "A→B→C→A" is the finding, "A and C conflict" is not. The same list carries the premises of a `derived_contradiction` and the member set of a grouped exclusivity violation. Only asserted statements are addressable in the export: a premise chain that ran through a rule-derived edge keeps the asserted members of the chain, because a premise that is not itself a fact row has no IRI — the export never names a statement it does not contain.

`detectedAt` says what the column says: when this row was recorded, and — because a re-found resolved row reopens with `detected_at` reset — when it was reopened. It is not "first detected": a row that stayed open keeps its original value, and the contract does not claim otherwise. `closedAt` is `decided_at`: when the row stopped being open.

### The `open` invariant

The criterion link on an open row is only true if `open` means *reconciled against the current ontology*. Today's `update_relation_type` can change axioms, domain and range and return without reconciling, leaving open rows whose criterion no longer exists — a state every consumer of the table, not only the export, can observe. The invariant is therefore owned by ontology mutation, not by the exporter:

**When an ontology edit changes a criterion the consistency check reads — axiom flags, `inverse_of`, `sub_property_of`, `temporal`, domain, range, or the class hierarchy — the same transaction closes the open violations whose criterion no longer applies with `resolution = 'criterion_changed'` and re-detects under the new ontology.** `criterion_changed` (migration `0093`) is recorded as a resolution but not an adjudication: `decided_by` stays empty. A row that was already stale under the old criterion — one no detection under it would have found — is deleted as before, not re-labelled.

Mechanically this is one detection pass before the write (the old-ontology baseline), the ontology write, and the existing detection + settle again, in one transaction. Rows a fresh detection no longer finds are partitioned against the baseline: found there → `criterion_changed`; absent there → gone. Reopened rows follow the existing rule — a `criterion_changed` row whose violation recurs reopens like `axiom_relaxed` does.

What is deliberately not claimed:

- The covered surface is every supported criterion mutation: `update_relation_type`, class-parent edits, class deletion, relation deletion that unlinks `inverse_of`/`sub_property_of`, and ontology **import** — which cannot share one transaction across its own batched writes, so it captures the old-ontology baseline before apply and reconciles after, giving retired rows the same `criterion_changed` end state. The interval inside apply's own commits is the pre-existing non-atomicity of import; no committed state after the request completes carries an open row valid only under the old ontology, and a failed reconciliation is an error, not a quiet pass.
- Facts changing is a data event, not an ontology mutation; invalidation-facing cleanups continue to work as they did.

### History the storage does not keep

The two tables have different guarantees and the contract does not pretend otherwise:

- `fact_conflicts` rows are durable contest records: a resolved or withdrawn row stays.
- `axiom_violations` rows are **current state, recomputed**. A resolve-then-reopen leaves no trace; `detected_at` resets on reopen; open rows a run cannot re-find are removed. The export serialises the present; nothing in it should be read as an append-only violation history.
- `path` / `detail` are the evidence as of the latest detection that recorded the row (refreshed per run since #619); they are not a proof reproduced at export time.

If the contract later needs real adjudication history, the storage needs an append-only event rather than a mutable status column. That is a separate decision.

### Out of scope — workflow is not epistemics

None of this enters the contract, now or later, just because it exists in storage:

- `pending_facts`, duplicate/entity-resolution review, mappings, agent proposals, and the rest of the Review UI's queue state;
- `decided_by` and person identity — *that* it was decided, when, and to what is epistemic; *who clicked* is workflow;
- an `as_of` export, a generic review-event stream, or a business-rule vocabulary.

## Vocabulary added

Everything without a standard spelling mints under `urn:utopia:ns:`: the classes `FactConflict` and `AxiomViolation`; the links `priorStatement`, `incomingStatement`, `onStatement`, `evidencePath`, `onRelation`, `criterion`; and the literals `reason`, `kind`, `status`, `resolution`, `detectedAt`, `closedAt`. The value sets above are the contract: readers should treat unknown values as a newer contract, not silently.
