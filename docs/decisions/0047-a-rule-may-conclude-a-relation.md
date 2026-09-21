# 0047 · A rule may conclude a relation

- **Status**: Implemented 2026-09-22 · pending review · revises the edge exclusion stated in [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md), and asks the question [0030](0030-a-rule-may-read-what-a-rule-concluded.md) parked as "nobody has asked it"
- **Written**: 2026-09-20 (conventions in the [README](README.md))
- **Related**: [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) built the rule and excluded an edge conclusion; [0030](0030-a-rule-may-read-what-a-rule-concluded.md) replaced that exclusion's acyclicity argument with a finiteness one on the value channel, and this record carries it to the edge channel; **[0032](0032-a-rule-computes-what-it-concludes.md) already decided that a rule may reach a value across one relation** and has not built it — this record depends on that loader rather than re-deciding it; [0002](0002-reasoning-engine.md) built the axiom fixed point and left R3 open; [0024](0024-the-world-axis-reaches-the-second.md) governs the precision of a derived bound; [0013](0013-a-source-should-hand-over-its-history.md) forbids reading a previous run's output, which stays forbidden. From #818.

> A holding company owns 60% of a subsidiary, and the subsidiary owns 70% of a third company. Nobody wrote down that the first controls the third, and today nobody can write the rule that says so. The threshold belongs to the rule engine, which reads values and concludes about one entity. The chain belongs to the axiom engine, which walks edges and cannot read a value. The question can be asked once at query time by walking `paths_between`, and the answer evaporates when the tab closes — it never becomes a fact, so it has no interval, no proof, and no place in a review queue.

## What exists

`reasoning::materialize()` runs two reasoners in one call, in this order:

- **`derive()`, once.** It takes the asserted edges `timed_edges()` loaded, closes them under the seven declared axioms, and stops at `MAX_DEPTH` premises or `MAX_DERIVED_PER_PREDICATE` (20,000) rows per predicate. It follows edges and never reads a value.
- **Then the rule rounds.** A fixed point of up to `MAX_DEPTH` rounds [0030]: each round's conclusions rejoin the in-memory `fact_pool`, a derived typing enters the next round as a premise carrying its interval, and no input query touches `derived_facts`. A rule reads values and concludes a class, a constant, or an expression [0032].

So values chain, edges close, and the two never cross. `derive()` has already finished before the first rule round begins, which is the whole of the gap: a rule cannot conclude an edge, and nothing a rule concludes is ever walked.

One thing that looks like the gap is already decided. 0032 lets an operand name an attribute at the far end of one relation — `well → field → depth` — on either side of a rule, with the relation fact as a premise and the interval as the intersection. That is the **read** across a hop, decided and unbuilt, and its own record names the single-hop edge loader as "the bulk of the implementation, not the arithmetic". This record needs exactly that loader and does not re-decide it. What is left, and what #818 is about, is the **conclusion**.

## The objection this has to answer

0021 states the exclusion under "The evaluator is a second pass, not an extension of `derive()`", and gives its reason plainly: the rule pass runs after `derive()`, a rule concludes a type or an attribute that no axiom consumes, so "the ordering is therefore free of a cycle by construction, which a rule concluding an *edge* would not be."

0030 then took half of that argument apart. It made the rule side a fixed point, and when it did, the property it defended was no longer acyclicity but **finiteness**: subjects are finite, a conclusion's value is written in the rule, every interval endpoint already exists in the fact set, each round only adds, and a conclusion that reproduces a key does not re-enter the frontier. A loop became safe rather than forbidden. It parked the rest explicitly — "Axioms and rules stay two reasoners … Folding both into one fixed point is a different question with a different cost, and nobody has asked it."

This record asks it, and answers it with 0030's own argument rather than a new one.

## Decisions

**1. A conclusion may be a relation between the subject and one entity reached across one declared relation.** The rule names a join predicate; `X` is the subject as today and `Y` is any entity such that an edge `X --join--> Y` holds in the pool. Each condition declares which of the two it reads, and the conclusion may be `Relation { predicate, from: X, to: Y }`. `derived_facts` already carries `subject_id`, `predicate_id` and `object_id`, so this is a fourth conclusion shape in the evaluator and no new table. One hop: two hops is this feature applied twice by a second rule, and an unbounded path is a different feature with a different termination question.

**2. Validity is the intersection of every premise interval, including the join edge's own.** This is 0021 decision 4 unchanged, and it extends without a new line: a control relation holds exactly while both holdings hold, with precision from the premise that won each end [0024]. It is also the thing the query-time path walk cannot produce at all, which is **when** the chain held.

**3. A concluded edge joins the pool that `derive()` reads, and `derive()` runs once per round.** This is the cycle's answer. The alternative shape — a single extra axiom pass after the rule fixed point — is refused under **Dead ends** below.

Termination is 0030's argument on the other channel. The edge space is finite: entities, declared predicates, and interval endpoints drawn from the finitely many already in the fact set. Each round only adds, dedupe on `(subject, predicate, object, interval)` eats the no-ops, and the run stays a pure function of `(facts, rules, axioms)` because the input queries still read no `derived_facts` row [0013]. The bound is the round cap, reported through `rule_rounds_capped` as it is today.

What changes is the **size** of the space rather than its finiteness: edges are quadratic in entities where values are linear. The existing per-predicate cap is what stands between a joined rule and a blowup, and whether 20,000 survives a rule feeding the edge pool is decision 4's problem.

**4. The caps are set by measurement, published in the PR that changes them.** Three numbers are in play and none is chosen here, because a number picked in a record is a guess wearing a decision's clothes. `MAX_COMBOS` (64) bounds premise expansion per subject today; with a join the population is pairs times the combinations on each side, so it applies per `(rule, X, Y)` and an overflowing pair reports as capped the way an entity does. `MAX_DERIVED_PER_PREDICATE` now bounds a predicate a rule can also write into. `MAX_DEPTH` rounds now costs an axiom pass each. The numbers come from the SEC and contracts corpora, and 0032 already asked for the same measurement on `MAX_COMBOS` before it ships.

**5. Out of this record.** No negation, no aggregation, no user-defined recursion, no second hop, and no join on anything but a declared relation predicate. Aggregation stays refused for 0032's reason, which this record does not weaken: a `sum` asserts that these are all the readings, and an open-world base cannot hold a completeness claim. Negation needs stratification before it can even be stated safely, and stratification is exactly what decision 3 declines to build.

## What it costs

**`derive()` runs per round.** 0030 puts the analogous move on the value side at two to three times a single pass, because real chains are one or two links and the fixed point converges when a round adds no new key — but that figure is an estimate in a code comment rather than a measurement, so it is a reason to expect the cost to be tolerable and no evidence that it is. Here the pass being repeated is the expensive one, so this number is measured before the cut lands.

**Contradiction checking moves inside the round.** Today `contradictions()` runs once on the single derivation, and `blocked` is computed before the rule rounds start. A concluded edge can contradict an assertion or another derivation, so the check has to see the edges a round added. The blocked set is also an input to the next round: an edge that lost to an assertion must not be joined on, or a rule fires on something the graph refused to show.

**A joined conclusion is otherwise an ordinary derived fact**, so the machinery around it applies with no new work. The proof tree shows the join edge as a premise [0030], retracting any premise retires the conclusion in the same total recompute [0002], the temporal engine closes and contradicts it like any other row, and the review queues see it. That is the argument for spending the cut here rather than on a richer query language: a query answers once, a fact takes part.

**The interface has to say the new shape out loud.** `RulesPanel` gains the join and a side marker per condition; `list_rules` and `rule_matches` gain the same in their text — the three places #808, #815 and #816 just touched.

## Dead ends

- **One more `derive()` after the rule fixed point.** A fixed three-pass ordering terminates by construction, which is tempting precisely because it is the property 0021 was protecting. It fails for the reason 0030 already gave against rule-dependency approximations: a rule joining on a predicate that another rule concluded would silently never fire, and in the result "the criterion was not met" and "the edge was never seen" look identical. An approximation that forbids a legitimate criterion is worse than the loop it prevents.
- **A rule-dependency graph deciding which rules may join.** 0030 showed it cannot be computed: a rule's inputs are predicates rather than rules, so "does R feed R" is only ever an approximation.
- **Answering the holding question in the query layer.** `paths_between` already walks it, which is why the gap went unnoticed. The answer has no interval, no premises, no retirement, and cannot be contradicted — so nothing downstream of it can exist.

## Sequencing

Schema and model; the single-hop edge channel 0032 also needs; the evaluator with its pairs, caps and per-round contradiction check; writing, proofs and the report; then the interface. The first three land together or not at all, because a half-built join writes facts nobody can explain.

## Open questions

- **A pair `(X, Y)` reachable by several join edges with different intervals.** Each edge is its own premise set and therefore its own conclusion row, the same way two readings are today [0032]. Worth confirming against a real corpus that it does not multiply rows past usefulness.
- **Whether the join may run backwards** as `Y --join--> X` without a declared inverse. Declaring `inverse_of` already expresses it and keeps one direction in the model, which argues for refusing the sugar.
- **Chaining through a concluded predicate is available two ways** once decision 3 holds: another rule joining on it, or the predicate declared transitive so the axiom pass extends it. They produce different proofs for the same conclusion, and nothing here says which one a person should reach for.
