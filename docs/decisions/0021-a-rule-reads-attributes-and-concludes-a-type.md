# 0021 · A rule reads attributes and concludes a type

- **Status**: planned · nothing built; the derivation machinery it stacks on is live
- **Written**: 2026-09-05 (conventions in the [README](README.md))
- **Related**: [0002](0002-reasoning-engine.md) built the derivation runner and ruled out a user-defined rule language for ontology axioms; this record adds one narrow rule kind that lives beside the axioms, not inside them. [0009](0009-no-type-is-a-type.md) made `type_id` nullable and a type a considered claim; a rule concludes a *second* type without touching the asserted one. [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) draws the asserted / derived line this conclusion sits below. From #277.

> A mud-logging user wants a well marked gas-bearing when its total-hydrocarbon reading is above a threshold **and** its interpretation category is "gas anomaly". Both values are already captured as dated attribute facts with the source passage attached. The rule that reads them has nowhere to live: the reasoning engine compiles ontology axioms only, and derivation runs over entity–entity edges, so a literal comparison cannot be expressed and "this well is gas-bearing" cannot be a derived fact with premises.

## What the ground already gives, and what it withholds

Three parts of the machine are reusable as they stand:

- **`fact_derivations`** links a derived fact to its premise facts in order — the proof chain 0002 built. A rule conclusion wants exactly this: the well is gas-bearing *because of* these two readings.
- **Materialisation cadence** — `materialize_inferences`, `inference_interval_minutes`, the invalidate-on-retracted-premise pass — is rule-agnostic. A business rule reruns on the same switch.
- **The asserted-over-derived precedence** — a derived row never overrides an asserted one, and reads that want only ground truth filter `derived_by_rule IS NULL` / skip the `derived_facts` union.

Two parts withhold:

- **`derived_facts` is narrower than `facts`.** `facts` carries either an entity object or a literal (`CHECK (object_id IS NOT NULL OR object_value IS NOT NULL)`); `derived_facts.object_id` and `predicate_id` are both `NOT NULL`. So a derivation today is always an entity–entity edge. Neither conclusion #277 names fits: a derived typing (`Well` → `GasBearingWell`) is a class, not an edge; a derived attribute (`gas_potential = good`) is a literal, not an object.
- **`rules` holds axioms, not conditions.** Its shape is `(predicate_id, kind ∈ {transitive, symmetric})`: one predicate, a closed axiom kind, no place for a comparison. A conjunction of `reading > 12 AND category ∈ {…}` concluding a class has no column to land in.

## Decisions

**1. Widen `derived_facts` to match `facts`, rather than add a parallel table.**

Make `object_id` nullable, add `object_value JSONB`, and copy the `facts` check: `object_id IS NOT NULL OR object_value IS NOT NULL`. A derived attribute (`gas_potential = good`) is then a literal-valued derived fact, and everything downstream — the proof chain, invalidation, the materialisation pass, "drawn as derived on the graph", the `derived_by_rule` precedence — works on it unchanged, because those paths key on the row being in `derived_facts`, not on its object being an entity.

The alternative was a `derived_typings` table plus a separate widening for attributes: two new shapes for the derived reader to learn, where the ground already had one general shape (`facts`) that a rule conclusion is a special case of. The seam this product keeps closing is exactly this one — a half-built parallel path beside a general mechanism (0016's whole premise) — so the mechanism moves, not around it.

**2. A derived typing is a derived attribute fact on a builtin `is_a` predicate.**

`Well` → `GasBearingWell` is stored as a derived fact whose predicate is the builtin `is_a` and whose `object_value` names the concluded class (by IRI, so it survives a label edit — see [0009](0009-no-type-is-a-type.md)). It does **not** write `entities.type_id`: that column holds the asserted type, and a rule conclusion is derived, below it (0015). The entity keeps its asserted `Well`; the graph and panel show `GasBearingWell` as a derived overlay that vanishes when the rule stops firing. This is why decision 1 is worth its cost — with a literal-valued derived fact in hand, a derived typing needs no second concept, only a reserved predicate.

**3. Rules live in a new `attribute_rules` table, authored by a person, never proposed by the model.**

Same governance line as the axioms (0002): the reasoning engine's criteria are written, not guessed. A rule row names the subject class it applies to, the conclusion (a class IRI for a typing, or an attribute predicate + value for a derived attribute), and a conjunction of conditions stored structured, not as free text — each condition is `(attribute_predicate, op, operand)` where `op` is one of `> >= < <= in present` and the operand is a number, a number range, or a set of category strings. Authored in the ontology workbench or imported from a file. No arithmetic across entities, no aggregation, no disjunction across attributes of different entities: one entity, its own attributes, one conclusion. A general rule language stays ruled out, for the same reason 0002 ruled it out.

**4. Validity is the intersection of the premise intervals.**

A well with a 2023 reading that fires the rule and a 2025 reading that does not is gas-bearing over the 2023 interval and not over the 2025 one — two intervals across two evaluations, never a single row that flips. The conclusion's `valid_from` is the latest premise `valid_from`, its `valid_to` the earliest premise `valid_to`; an empty intersection fires nothing. Precision follows the coarser premise, on the same `(date, precision)` invariant the facts table keeps. This is the one piece of new temporal logic; the rest is the existing derived-fact lifecycle.

## The evaluator is a second pass, not an extension of `derive()`

`derive()` in `utopia-reason` walks `TimedEdge`s — entity–entity triples — under the axiom set. Attribute facts, with literal values, never enter it, and should not: a comparison against a threshold is a different operation from following a transitive edge. The business-rule pass is its own function in the same crate: for each entity in scope, load its attribute facts, evaluate each rule's conjunction, and emit a `Derived` carrying the satisfying premises and the intersected interval. It runs in the same materialisation job, after `derive()`, so a rule can conclude a typing that a later axiom pass never consumes (axioms are over edges; the typing is an attribute) — the ordering is therefore free of a cycle by construction, which a rule concluding an *edge* would not be. That restriction — conclusions are types or attributes, never edges — is decision 3's `op` set doing double duty, and worth stating plainly.

## Phasing

1. **Schema** — widen `derived_facts` (decision 1), the builtin `is_a` predicate (decision 2), `attribute_rules` and its condition rows (decision 3). One migration per the domain-file rule; folded per [[migration-policy]] after main.
2. **Declaration** — store/read for `attribute_rules`, and the ontology-workbench surface to author one (subject class, conditions over its attributes, conclusion). File import can follow.
3. **Evaluation** — the second pass in `utopia-reason`, wired into the materialisation job; interval intersection; invalidate-on-retracted-premise already generalises.
4. **Explanation** — the entity panel shows the rule and each premise fact pointing back to its passage; the graph draws the derived typing as derived.
5. **Acceptance** — the #277 sample: define the two attributes and the rule, upload the report, the well appears as `GasBearingWell` with both readings as its explanation; lower the threshold below the reading and it clears on the next run; the interval matches the report's date.

## Dead ends

- **Writing `entities.type_id` from a rule.** It would put a derived conclusion where the asserted type lives, so a retracted premise would have to restore the prior asserted type — reconstructing ground truth from a derivation, which 0015's line forbids. The overlay avoids it.
- **A general rule DSL.** 0002 already walked this road for axioms and turned back; #277 is explicit that the scope is one entity's own attributes. A DSL would be built for a generality no one asked for and would reopen "the model writes the criteria".

## Open questions

- **Does a derived typing feed the extraction prompt or the mapping probe?** Those read the ontology's classes; a well that is derived-`GasBearingWell` is not asserted to be one. Leaning no — a derivation is a read-time overlay, not a fact the ontology teaches back into extraction — but a rule that concludes a class the corpus then reinforces is a loop worth watching.
- **Category conditions and language.** `interpretation_category ∈ {"气测异常", "气测异常后效"}` is text matched literally. If the same category arrives in English from a different source, the set misses it. Out of scope here; the mapping layer (0011) is where a category vocabulary would belong.
- **One conclusion per rule, or several.** #277 says one. A report that sets both a typing and a `gas_potential` grade from the same two readings would want two rules over the same premises today; whether that is a nuisance is a question the sample will answer.
- **Where authored rules live for export.** If a deployment's rules are part of its ontology, RDF export (#308) has to say how — a rule is not an OWL axiom. Deferred to whenever export lands.
