# A rule concludes from readings it can name

Records: [0002] (axioms), [0021] (attribute rules), [0029] (or), [0030] (chaining), [0032]
(computed conclusions), [0017] (contradictions), [0036] d4 (definitions as rules), [0034] (the seam
to actions). Interval rules are in [time](time.md), the review side in
[governance](governance.md).

## What it does today

**Two reasoners, one materialisation job.** Axioms compiled from the ontology (transitive,
symmetric, inverseOf, subPropertyOf; functional, inverse-functional, asymmetric, irreflexive for
checking) run `derive()` over entity-to-entity edges; business rules (`attribute_rules`) run a
second pass over literal readings; both write `derived_facts` and `fact_derivations`, in a job every
`inference_interval_minutes` (60) under `materialize_inferences`, on by default; every run
recomputes the whole base [0002, 0021, 0030]. A fresh base holds only open statements, which have
no predicate, so there is nothing to derive until alignment writes typed rows [#731].

**R0 checks first and writes no facts.** Six fact-level kinds (`self_loop`, `asymmetry`, `cycle` with
its path, `functional`, `signature`, `derived_contradiction`) go to `axiom_violations`, which asks
"wrong data or wrong definition" (`fact_retracted`, `fact_closed`, `axiom_relaxed`, `accepted`);
eight ontology defect kinds go to `ontology_defects` and are shown first; "data is wrong" retracts
the fact and reruns [0002, 0017]. A functional violation is two values holding at one moment; a
succession is none [0017, #635].

**Asserted beats derived, hard.** An asserted triple is never derived; a derivation that contradicts
an assertion stays out and becomes one capped `derived_contradiction` row; two derivations that
contradict each other both stay out, aggregated per rule pair as `rules_disagree` [0002 d4, 0017].
`MAX_DERIVED_PER_PREDICATE` (20,000) truncates and reports; validity is the intersection of premise
read intervals, confidence the minimum, precision from the premise that set the bound; an empty
intersection derives nothing [0002 d8, 0022, 0024].

**A business rule** names a subject class, a conclusion (a class IRI on the builtin `is_a`, an
attribute value, or a computed expression) and conditions `(attribute, op, operand)` with ops
`> >= < <= in not_in present`, grouped: conditions in a group join with and, groups with or, one
level [0021, 0029, 0032]. A concluded typing is a derived attribute fact and never writes
`entities.type_id` [0021 d2]. An operand or a conclusion may be an expression tree over attributes
(`attr | const | add sub mul div`, depth capped), stored as JSONB, reachable through the API, the
picker not built; a missing reading or a division by zero concludes nothing and is reported [0032].

**Rules chain inside one run.** A round's conclusions rejoin the pool as readings and as derived
class membership (entering as a premise so the interval narrows to when the entity was that class);
the fixed point runs to `MAX_DEPTH` with a sorted frontier so a run is reproducible; a rule may read
its own conclusions; the previous run's `derived_facts` are never input [0030]. The proof is a tree
(`premise_fact_id` or `premise_derived_id`, one `seq`), walked by `proof()`; a kept row whose
premises changed is reproved [0030]. Retirement is free: every run builds the wanted set and
invalidates what is not in it [0030].

**Explanation.** The entity panel shows a conclusion with its rule and each premise back to its
passage; derived edges are gold behind a toggle; blocked derivations are ghost edges; MCP has
`list_rules` and `rule_matches`, read-only [0002, 0017, 0021].

## Why

- **A reasoner amplifies defects**: 185 `part_of` facts became 828 under closure, with cycles from
  extraction errors, so the engine is a checker first [0002].
- **Derived facts in their own table** because of forty queries over `facts` one knew a flag; a
  forgotten UNION hides derivations instead of mixing them in [0002 d3].
- **Rules come from the ontology or from a person, never proposed by the model** [0002 d5, 0021 d3].
- **Widen `derived_facts` rather than add a parallel table**: a typing and an attribute are special
  cases of the shape `facts` already has [0021 d1].
- **A derived typing is an overlay**: writing `type_id` would put a derivation where the asserted
  type lives and force reconstructing ground truth on retraction [0021].
- **One level of or**: a hit reports the readings that made it true, and a disjunction has no
  premises of its own; absence is not a condition because it has no premise and the base is
  open-world [0029].
- **A fixed point, not an ordering**, or whether `A → B → C` fires depends on load order; the DDL's
  objection protects "a run reads only asserted facts", which the in-memory fixed point keeps [0030].
- **Picked, not typed**, because a picker cannot name an attribute that does not exist and a text
  box grows a grammar; storing a string evaluated at run time is what is forbidden [0032].
- **Aggregation is refused** because a sum asserts a completeness the base cannot hold, its proof
  cannot explain its retirement, and it forecloses incremental maintenance [0032].
- **Repairs, not rulings**: a contradiction is stale knowledge, a misread, a wrong merge or an
  over-strict ontology, and each button goes there [0017].

## Proposed and not built

- Reading a value across a relation (`well → field → depth`); unit and datatype checks when an
  expression is written; the expression picker [0032].
- The page showing which rules feed which and a deeper proof rendering [0030].
- A definition over aligned attributes as a rule, with a shared convention as one rule others read,
  and an executor that runs it at the source where a sum is honest [0036].
- Rules firing actions on a derived row's first appearance, deduped on `(rule, derived_fact)`
  [0034]; implication rules of alignment, executed by code and marked implied [0044 d3].
- R3 incremental maintenance, deferred until a full re-derivation exceeds a threshold (10 s, a
  guess) [0002, 0016 B4].
- A canvas marker for rule-classified entities; editing a conclusion in place; an agent writing a
  rule behind a pending queue [0021].

## Open questions

- Materialise or evaluate at query time at scale [0002]; the cap of 50 per predicate for
  contradictions and per-pair `accepted` granularity [0017].
- Category conditions match strings literally across languages [0021, 0029].
- Whether a derived typing should feed extraction or alignment [0021].
- How deep a chain is worth drawing; whether a computed conclusion feeds the next rule (it should,
  untested) [0030, 0032].
