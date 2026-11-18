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
(`attr | const | text | add sub mul div`, numeric cast, scalar case and date truncation), stored as
JSONB and reachable through the API. The depth cap is four edges from the root. A missing reading,
division by zero or failed conversion concludes nothing [0032, 0036]; the experimental picker
does not enable semantic edits of conversion expressions.

**A conversion has a stored tree.** `{"cast":"number","expr":node}` converts to the existing
finite floating-point number type. `{"case":node,"when":[{"is":1,"then":"app"}],"else":"other"}`
maps codes to values; its one to 32 arms are scalar literals, its results all have the same type,
and an unmatched code without `else` has no value. A missing selector has no value even with
`else`. Case compares text and numeric codes separately; use an explicit cast to interpret a
text code as a number. `{"date_trunc":"month","expr":node}` returns an ISO calendar date;
`year`, `month` and `day` are supported. An ISO date is read as a calendar date; an RFC 3339
timestamp is converted to UTC first, so `2024-03-01T00:30:00+08:00` truncates to `2024-02-01`.
Naive timestamps and invalid dates have no value. These results use the same premise intersection,
history, retirement and chaining as numeric computations. Numeric arithmetic remains floating
point, not fixed-scale decimal accounting, and this does not introduce a unit-conversion policy.

**Text is parsed before a write.** On `POST /api/v1/kbs/{id}/rules` and the corresponding rule
`PATCH`, `conclude_expr` or a comparison condition's `operand` may instead contain
`{"expression":"CAST(amount AS DOUBLE PRECISION) / 100"}`. Attribute **keys** resolve exactly
within that base; quoted keys support spaces or non-Latin text. Only the resulting UUID tree is
stored, including in definition versions. Ordinary numeric threshold strings remain scalar
thresholds. Input is limited to 8192 bytes and the same tree depth bound. Supported text includes
arithmetic, `CAST(... AS DOUBLE PRECISION)` (also `DOUBLE` or `::`), simple literal
`CASE ... WHEN ... THEN ... [ELSE ...] END`, and `date_trunc('year'|'month'|'day', ...)`.
Mixed or unknown JSON fields, unknown names, qualified paths, arbitrary functions, aggregates, subqueries, additional statements,
cast precision/scale and function modifiers are refused. A rename or metadata-only patch keeps the
existing definition. The rule page renders conversions read-only and includes their input
attributes in the dependency view.

The conversion regressions exercise these writes through authenticated routes and the real
materializer: text and date results, chained conclusions, premise intervals, version changes,
failed conversions and cross-base/viewer rejection. Table adoption and SQL rendering per source
dialect remain [#554](https://github.com/deeplethe/utopia/issues/554) and
[#556](https://github.com/deeplethe/utopia/issues/556); existing SQL mappings are unchanged.

**Rules chain inside one run.** A round's conclusions rejoin the pool as readings and as derived
class membership (entering as a premise so the interval narrows to when the entity was that class);
the fixed point runs to `MAX_DEPTH` with a sorted frontier so a run is reproducible; a rule may read
its own conclusions; the previous run's `derived_facts` are never input [0030]. The proof is a tree
(`premise_fact_id` or `premise_derived_id`, one `seq`), walked by `proof()`; a kept row whose
premises changed is reproved [0030]. Retirement is free: every run builds the wanted set and
invalidates what is not in it [0030].

**Explanation.** The entity panel shows a conclusion with its rule and each premise back to its
passage; derived edges are gold behind a toggle; blocked derivations are ghost edges; MCP has
`list_rules` and `rule_matches`, read-only [0002, 0017, 0021]. `list_rules` includes the
active ontology axioms, using the same inverse normalization as the reasoner; retired
declarations are excluded even though their compiled rules remain for historical proofs.
When `materialize_inferences` is off, the section says that derivation is off for this
base and nothing is derived from these declarations.
In chat and MCP, a derived fact names the property its axiom was declared on (for example,
`inverse of works_at`), which can differ from the conclusion's property. `rule_matches`
continues to take business rule IDs.

**A definition has a history.** Every edit that changes what a rule says opens a version (a full
snapshot with a record time) and closes the previous one; renaming or switching a rule off does
not. A derivation names the version it was drawn under, a kept conclusion moves to the new
version, and the proof, the rules panel's history and `GET /kbs/{id}/rules/{rule_id}/versions`
read it, so an invalidated conclusion still says what the rule said when it was drawn [0060].

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
- **One stored expression tree.** A picker or the bounded text compiler resolves attribute IDs
  before saving; storing a string evaluated at run time is forbidden [0032, 0036].
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
