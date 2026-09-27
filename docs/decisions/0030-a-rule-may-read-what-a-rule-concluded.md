# 0030 · A rule may read what a rule concluded

- **Status**: In progress · cuts 1–2 built (migration 0040) · open: the page showing which rules feed which
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) built the rule; [0029](0029-a-rule-may-say-or-once.md) settled the shape of **one** rule, and this record settles how **several** compose — the two are independent. [0002](0002-reasoning-engine.md) built the axiom fixed point this borrows from, and left R3 (incremental maintenance) open, which this record does not close. **[0013](0013-a-source-should-hand-over-its-history.md) is where the invariant this overturns was written down**, in the DDL comment on `fact_derivations`. [0024](0024-the-world-axis-reaches-the-second.md) decides which premise sets a derived bound's precision, and a chained bound obeys it one level up. From #477.

> A well whose total hydrocarbon is above 8 and whose interpretation reads as a gas anomaly is **gas-bearing**. A gas-bearing well with a completion date is a **producer**. Two criteria, written by the same person on the same afternoon, and the second one never fires — because the first one's conclusion is not something the second one can read. To get the producer you write the hydrocarbon condition again inside the producer rule, and now two rules share a premise that nothing in the base says they share. Change what "gas-bearing" means and one of them quietly stops agreeing with the other.

## Why it does not chain today

Three sites, none of them an oversight, all of them from when a rule was a single pass over asserted readings:

- **`attribute_facts()` reads `facts`.** A rule's conclusion lands in `derived_facts`, which no input query touches.
- **Subject scope is the asserted type.** `materialize()` filters an entity into a rule by `entities.type_id`. A rule that concludes class `B` does not make its subjects members of `B` for a rule scoped to `B`.
- **One pass.** Each rule is evaluated once, in load order. Nothing re-reads what the round produced.

The contrast is worth naming, because it makes the shape of the answer obvious: **the axiom reasoner already chains.** `derive()` runs a semi-naive fixed point exactly so `ceo_of → works_at → employs` connects, bounded by `MAX_DEPTH` rounds and a per-predicate output cap, with the frontier sorted every round so two runs over the same base give the same answer. Chaining is not foreign here. It was never wired for business rules.

## The objection that has to be answered first

The `fact_derivations` DDL says it plainly:

> premises are always assertions: derivation excludes derived facts as inputs, or the output of one call becomes the input of the next and a re-run's result depends on what the round before left behind.

That objection is correct, and this record does not waive it. It answers it.

**The feedback happens inside one run, in memory.** A round's conclusions become the next round's inputs *within the same `materialize()` call*, in a fixed point that starts from the asserted facts every time. No input query reads `derived_facts`. A run stays a pure function of `(facts, rules, axioms)` — which is the property that objection protects. That property does not actually follow from "premises are assertions"; it follows from "the run does not read its own past output". The two coincided while there was one pass, and this record separates them.

Reading the *previous* run's `derived_facts` as input stays forbidden, for exactly the reason 0013 gave.

## What is decided

**A fixed point, not an ordering.** Round after round until a round adds nothing, bounded by `MAX_DEPTH` rounds and by premise-chain length, with the frontier sorted so the answer does not depend on hash order. Without a fixed point, whether `A → B → C` fires depends on the order rules happen to load — which works on one machine, fails on another, and is the worst answer available.

**Conclusions rejoin on both channels.** A concluded attribute value becomes an `AttrFact` in the pool, its value read out of `{"value": …}` the same way an asserted one is. A concluded typing joins the subject scope: a rule scoped to `B` sees an entity whose derived `is_a` names `B`, matched by class IRI and falling back to key — the same identity 0021 chose so that renaming a label does not turn a conclusion into a different one.

**Derived membership is timed; asserted membership is not, so the two enter differently.** `entities.type_id` carries no interval: it is a filter and stays one. A derived typing holds only on the interval it was concluded for, and an entity that is a `B` from 2019 to 2022 must not satisfy a `B` rule on a 2024 reading. So a derived typing enters **as a premise**: it lands in the hit's premise list, `validity()` intersects its span with the conditions', and the conclusion narrows to the window in which the entity actually was a `B`. This is not symmetry for its own sake. It is the only way the interval comes out right, and it is what makes the retirement below work without a line written for it.

**A rule may read its own conclusions.** The alternative — only *other* rules' — needs a rule-dependency graph that cannot be computed: a rule's inputs are predicates, not rules, so "does R feed R" is only ever an approximation, and an approximation that forbids a legitimate criterion is worse than the loop it prevents. The loop is safe. The derivable space is finite: subjects are finite, a conclusion's value is a constant written in the rule, and every interval endpoint comes from the finitely many endpoints already in the fact set. Each round only adds, and a conclusion that reproduces a key already reached does not re-enter the frontier. `X is_a B because X is_a B` is not a contradiction the way `A p A` is on a transitive antisymmetric predicate — it is a no-op, and the dedupe eats it. The bound is the round cap, not a prohibition.

**The proof becomes a tree, and the schema has to say so.** `fact_derivations.premise_fact_id` references `facts(id)`, so a derived premise cannot be stored at all today. It gains a nullable sibling, `premise_derived_id` referencing `derived_facts(id)`, with a CHECK that exactly one of the two is set, one `seq` ordering across both kinds so the proof still reads in order, and an index on the new column for the reverse lookup. `proof()` stops being a flat list of assertions and becomes a walk: a derived premise expands into its own step and its own premises, down to the assertions and their chunks. `ProofStep` and its doc comment — "premises are always assertions, so the proof is a chain rather than a tree" — are wrong from that commit and change with it.

**Retirement cascades, and it is free.** This sounds like the expensive part and is not. Retirement here is not an incremental walk down a dependency graph. Every run builds `wanted`, the whole set of what should hold now, and invalidates every live `derived_facts` row that is not in it. If a leaf reading changes, the chain that stood on it is simply not rebuilt this round; the whole tail falls out of `wanted` and retires in the same pass, in the same statement, with nothing in the code aware that a chain existed. **The cascade is a consequence of full recompute, not a feature anyone writes.**

**Row identity is resolved after the diff, not during the round.** A conclusion reached in round 2 has premises that are rows the round has not written yet — and may never write, because an unchanged conclusion keeps its existing row instead of getting a new one. So the fixed point carries a provisional id per conclusion; the diff maps every key to its final id (the live row's for a kept conclusion, a fresh one for a new conclusion); premise ids are rewritten through that map before `fact_derivations` is written. Getting this backwards — writing derivations against provisional ids — points every chained proof at rows that do not exist.

**Asserted and derived readings are both readings.** If a rule concludes `grade = A` on an entity that also asserts `grade = B`, both sit in the pool, and a downstream rule can fire on either, producing two hits with different premises and usually different intervals. This is the 0021 position unchanged — a derived fact never replaces an asserted one — and the proof is what tells them apart. Suppressing the derived reading would need a preference order that 0021 deliberately did not build.

## What it costs

**Display depth**, and only that. The entity panel draws a proof one level deep; a chained conclusion is two or more. That is cut 3's problem, along with the rule table saying which rules feed which — a chain is worth seeing before you delete the rule in the middle of it. On the schema diagram the edges already compose visually: `Well → GasBearingWell → Producer` is two rule edges in a row, drawn with no change.

**A report that says the cap was hit.** Rounds stopped by `MAX_DEPTH` have to be counted and reported, for the same reason the combination cap is: a conclusion that was not reached and a criterion that was not met look identical in the result.

## Not in this record

- **Axioms and rules stay two reasoners.** Rules read literal values, axioms read entity-to-entity edges. The one place they meet is a derived typing, which is a literal on `is_a` and therefore already on the rules' side. Folding both into one fixed point is a different question with a different cost, and nobody has asked it.
- **Incremental maintenance is still open** (0002 R3). This record makes the full recompute do more, not less. If it becomes too slow, that is the record to write — and the cascade being free is exactly what an incremental version would have to reproduce, and would find hard.
- **Reading the previous run's output.** Forbidden, per 0013. The fixed point is what makes it unnecessary.

## Open

- ~~A kept row can keep a stale proof.~~ **Closed in cut 2.** `wanted` is keyed by subject, predicate, value and interval, with premises outside the key, so a conclusion reached this round by a different path than last round used to keep its old `fact_derivations` — already true with asserted premises, and easier to hit with chains, where the stale premise can be a row that was just invalidated. The run now reads the stored premises of every kept row in one query and rewrites the ones that differ; the count is `reproved` in the report.
- **How deep is worth drawing.** Twelve rounds is the bound, not the expectation. If real bases produce chains longer than two or three, the panel needs a shape for that rather than an ever-deeper nest.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> cuts 1 and 2 implemented · the record, and the runner: a fixed point inside one `materialize()` (rounds bounded by `MAX_DEPTH`, rules loaded in id order so a run is reproducible), a concluded value back in the fact pool, a concluded typing joining the subject scope **as a condition** so it carries its interval and its premise, `fact_derivations` widened by `premise_derived_id` (migration `0040`) with a `derivation_premises` view so all six readers see both kinds, `proof()` walking the tree, and a kept row re-proved when its premises changed · five database tests · the page showing which rules feed which is the next cut
