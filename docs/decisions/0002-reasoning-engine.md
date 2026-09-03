# 0002 · Reasoning engine

- **Status**: R0 built: six fact-level violation kinds in `axiom_violations` (five checks plus
  `derived_contradiction`, 0017), eight ontology defect kinds in `ontology_defects`, two Review tabs. R1 built behind the KB switch `materialize_inferences`
  (default off), rules from four axiom kinds (#132, #177, #179). R2 built as a proof chain
  (`GET /kbs/{id}/derived/{id}/proof`, 0016 B1). Contradiction signals for derivations built per
  [0017](0017-a-contradiction-points-upstream.md). R3 not built: every run recomputes the whole KB,
  on `inference_interval_minutes` (default 60).
- **Written**: 2026-08-28 · condensed into English 2026-09-03
- **Related**: [0001](0001-ontology-import-and-governance.md) P5 (this record replaces its schedule);
  [0010](0010-no-relation-is-no-relation.md);
  [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) reuses decision 3's criterion;
  [0016](0016-close-the-open-seams-before-cutting-new-ones.md); [0017](0017-a-contradiction-points-upstream.md)

## The problem

A reasoner amplifies defects. On `Industry Corpus` (28 press releases) 39% of edges were `related_to`,
and 185 `part_of` facts expanded to 828 under transitive closure at depth cap 10, with per-depth counts
oscillating from depth 5 instead of decaying: cycles, real ones, from extraction errors such as
`Microsoft → FarmBeats for Students → Microsoft`. Closure on day one would derive "Microsoft part_of
Microsoft" with an evidence chain attached. The same rule evaluator run backwards is a consistency
check with no truth maintenance and no explosion, so the engine is built as a checker first, the
corpus is cleaned, then materialization is switched on.

## Decisions

1. **R0 checks and writes no facts.** Pure logic in `utopia-reason`; kinds `self_loop`, `asymmetry`,
   `cycle` (depth-first, with the full `path`), `functional` (including inverse-functional) and
   `signature` (#190 / #196: subject outside the domain or object outside the range, computed in SQL
   by `store::reasoning::signature_breaks`; untyped entities excluded). Rows go to `axiom_violations`,
   which asks "wrong data or wrong definition": `fact_retracted` / `axiom_relaxed` / `accepted`.
2. **The ontology checks itself first.** `ontology_defects`, eight kinds (symmetric and asymmetric,
   transitive and functional, subclass cycle, disjoint with an ancestor, inherited disjoint,
   self-inverse, inverse not mutual, sub-property cycle), shown before violations because a
   self-contradicting ontology makes every fact-level conclusion suspect. A KB without an ontology
   pack reports zero, since there is no criterion; importing a pack runs a check.
3. **Derived facts live in their own table.** `derived_facts`, with
   `fact_derivations (derived_fact_id, premise_fact_id, seq)` and the rule on `derived_facts.rule_id`
   (migration `0013_reasoning.sql`). `fact_evidence.chunk_id` is NOT NULL and a derivation's evidence
   is other facts; and of forty-odd queries over `facts` one knew a flag, so a new query would read
   derivations as assertions by default. Separate tables fail the safe way: a forgotten UNION hides
   derivations. `facts.derived_by_rule` is never written; its only reference is a Review filter.
4. **Asserted beats derived, hard.** `reconcile_new_fact` auto-closes older facts of a functional
   predicate, so a derivation on that path would let one wrong rule close human assertions. An
   asserted triple is never derived (`derive.rs`); derived vs asserted does not land and becomes an
   `axiom_violations` row of kind `derived_contradiction` (cap 50 per predicate); derived vs derived
   aggregates per rule pair into `ontology_defects` kind `rules_disagree` and neither lands (0017).
5. **Rules come only from ontology axioms.** Transitive, Symmetric (#132), inverseOf, subPropertyOf
   (#177, #179; projection in migration `0016`); no user DSL. Rule identity is `(kb, predicate, kind)`
   and rows are kept when an axiom is withdrawn, so `rule_id` and history survive. Inverse
   normalization at axiom load fills gaps only; a mismatched pair goes to `inverse_not_mutual`.
   Cold-start bootstrap declares no axiom (`Axioms::default()`): the reasoner's criteria are
   written by people.
6. **Materialization is a KB switch, default off.** The job records its run time before deriving so
   a failure cannot loop; endpoints return an explicit error while off. `MAX_DERIVED_PER_PREDICATE` =
   20,000, truncation reported as `capped`; an `unruled` counter that should stay zero records a
   real bug (rule lookup by predicate instead of `via` dropped cross-predicate rules silently).
   Derived edges are gold on the graph behind a toggle; the entity panel has a "derived" section.
7. **Explanation is a chain** (`reasoning::proof`). `fact_derivations` records asserted premises
   only, so the proof runs derivation → assertions → evidence → chunk → document by `seq`.
   Retracted premises stay listed and flagged, so a proof stays readable after invalidation.
8. **Validity intervals intersect**: half-open, empty means nothing derived, touching endpoints do
   not overlap, precision the coarsest premise, confidence the minimum, literal objects excluded
   (`object_id IS NOT NULL`). Cycles go to Review with their path, since both edges came from the
   model and neither deserves prior trust; R1 never derives a self-loop.

## Dead ends

- **R0 output into `fact_conflicts`, "zero new UI".** That table asks "which one is right";
  violations ask "data or definition". New table, two new Review tabs.
- **Fact-level `disjointWith` violations** as an R0 kind: not built; disjointness feeds the ontology
  self-check only.
- **Derivations shown by `entity_history` for free.** Its UNION has facts, merges and retypes;
  visibility went to the graph and the panel instead.

## Revisions

- 2026-09-02: assumed derivations enter `facts` under the `derived_by_rule` column reserved in
  `0004_graph.sql`; the code has `derived_facts` and the column is never written.
- 2026-09-02: the "11 existing contradictions" rested on the seed ontology's `functional`
  declarations; with the seeds gone, a KB without a pack reports zero.
- 2026-09-03: the contradiction signals promised in decision 4 exist (0017); the `related_to`
  obstacle (39% empty edges) vanished with 0010, since null-predicate edges never enter reasoning.
- 2026-09-03: "Data is wrong" retracts the fact (#202). Until now `decide` only marked the violation resolved; the fact stayed in the graph, and a rerun hit the resolved row and stayed silent. The decision now names the fact (single-fact kinds pick it themselves; asymmetry, functional and cycle cards offer a button per fact), retracts it through `reject_fact`, and reruns the check. A resolved row whose violation is computed again is reopened when its resolution promised a change (`fact_retracted`, `fact_closed`, `axiom_relaxed`); `accepted` stays quiet.

## Open questions

- Materialize or evaluate at query time: 4.5× expansion on a small corpus, unknown at scale. For now
  a per-predicate cap and a periodic full recompute.
- R3 incremental maintenance: correctness is the hardest to verify, and full recompute holds.
