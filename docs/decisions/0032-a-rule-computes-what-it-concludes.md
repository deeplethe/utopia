# 0032 · A rule computes what it concludes

- **Status**: cuts 1 and 2 implemented · the record, and the capability: `Expr` in `utopia-reason` (`Attr | Const | Arith`, depth capped at `MAX_EXPR_DEPTH`), a third conclusion kind `computed` with the tree in `attribute_rules.conclude_expr` (migration `0041`), an operand that may be an expression on the comparison ops, the evaluator building the combination **before** testing conditions (a computed threshold reads the readings this round picked), every attribute the expression touched landing in the premises, and a malformed tree refused where it is written rather than skipped at materialisation · seven evaluator tests and three database tests · **the picker is the next cut**, so today an expression is reachable through the API and not through the page · reaching an attribute **across a relation** is decided in this record and not yet built
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) built the rule and made its conclusion a **constant**; this record makes it computable. [0029](0029-a-rule-may-say-or-once.md) widened the shape of one rule by one level of `or`; [0030](0030-a-rule-may-read-what-a-rule-concluded.md) let rules feed each other — this is the third axis and independent of both. [0002](0002-reasoning-engine.md) ruled out a user rule language, and this record says exactly what that ban does and does not forbid. [0024](0024-the-world-axis-reaches-the-second.md) governs the precision a computed bound inherits. From #488.

> A criterion is often about a number nobody wrote down. Net pay is gross thickness times net-to-gross; a margin is revenue minus cost; a wetness ratio is one gas reading over another. Both readings are in the base, dated, with the passage they came from attached. The rule that needs their product cannot say so: `attribute_rules.conclude_value` is a JSONB **literal**, and a condition compares an attribute against a literal too. So the number gets computed in someone's spreadsheet and typed back in as an asserted fact — losing the two readings it stood on, and going stale the moment either changes.

## An operand is an expression

On both sides of a rule. A condition can ask `revenue − cost > 0`; a conclusion can be `margin = revenue − cost`. It is one node type, stored as JSONB:

```
{"attr": <predicate_id>} | {"const": <number>} | {"op": "add"|"sub"|"mul"|"div", "l": <node>, "r": <node>}
```

with a depth cap. A bare `{"const": …}` is what every rule stores today, so nothing has to migrate.

**The premises and the interval come out right for free.** An expression reads N attribute facts; all N are its premises, and the value holds on their intersection — which is exactly what `validity()` already computes for a conjunction of conditions. A computed conclusion retires when any reading under it changes, with no new machinery.

## Picked, not typed

A class/attribute picker plus an operator picker. **Not a formula text box.**

A text box is not wrong because it would break the display — a string parsed at save time into the same tree keeps every property that matters: the rule still renders as a sentence, still draws as one edge on the schema diagram, two rules still diff. That was the wrong reason, and it is worth writing down as wrong because it is the one that first comes to mind. The right reasons are two:

- **Errors move from unfillable to undiscovered.** A picker can only compose an expression from predicates that exist on the classes in play. A box lets you save `revenue − cost` against a class with no `cost`, and the symptom is not an error — it is a conclusion that quietly stops appearing at the next materialisation.
- **A grammar grows on request.** Once there is a box, `if`, rounding, `coalesce` and `sum(...)` are each one line of asking. The first few are merely work. The last one is not (below).

**The concession:** a picker is right for one operator and wrong for four. `(revenue − cost) / revenue` as nested dropdowns is worse than typing it. If the shapes people actually reach for turn out to be that deep, a box that parses into this same tree is the answer and nothing here forbids it. What is forbidden is storing the string and evaluating it at run time.

This is also the whole of what 0002's "no user DSL" forbids today. That clause was written when rules *were* ontology axioms — a rule was a projection of a declaration, not an authored object. 0021 already crossed that line deliberately by letting a person write a criterion; what survives is narrower and sharper: **everything a rule says is structured, so what the page shows is generated from what runs and cannot drift from it.**

## A value may be reached across a relation

The picker names a class and an attribute, so it can name an attribute that is **not** the subject's: `well → field → depth`. This record allows that, and the reason is that a path is not a new kind of thing.

- Its premises are definite: the relation fact plus the attribute fact at the end of it. Both are rows in `facts`.
- Its interval is their intersection. A well that sat in that field only from 2019 to 2022 contributes that value only for those years — which is the answer you want and the one you would have to write by hand otherwise.
- **A to-many relation is not a problem either.** Two `field` facts give two candidate values, each with its own premises and its own interval. That is the same shape as an attribute reported twice, which the evaluator already walks as a cartesian product under `MAX_COMBOS`.

What it costs is real and belongs here: the rule evaluator reads the **literal** channel only (`attribute_facts()` loads `facts` where `object_value IS NOT NULL`). A path needs the entity–entity channel as well, so either the loader gains a companion for single-hop edges or the store resolves paths into flat readings before evaluation. That is the bulk of the implementation, not the arithmetic.

## Aggregation is not in this record, and not by oversight

`sum`, `count`, `avg` over a to-many relation are out. The line is not "one value versus many" — a traversal that yields three values yields three conclusions, each pointing at the readings that made it true. A `sum` yields one, and the difference is what that one conclusion claims.

**It asserts completeness, which this base cannot.** Every other derived fact here says "these readings hold, therefore this". A sum says "these readings **are all the readings**, therefore this". The second half is not a fact the base can hold: it is open-world, extraction lags documents, and documents lag the world. The honest statement is "the wells we know about total 300", and that is not what the row would say.

**Its proof cannot explain its own retirement.** Retirement itself is fine — recompute is total, so a fourth well simply produces 400 and the 300 falls out of `wanted` like anything else. But the retired row still names readings A, B and C as its premises, and all three are still live. A reader looking at a conclusion whose every premise still holds, asking why it was withdrawn, finds nothing in the proof: the reading that changed the answer was never one of its premises.

**It forecloses incremental maintenance** (0002 R3, still open). An incremental pass walks premises to decide what to invalidate. A new reading that is nobody's premise wakes nothing. Full recompute hides this today; the record does not, because R3 is a road this product still intends to take.

If it is ever wanted it needs its own record, answering what a completeness claim means here and how a proof reports what it did not see.

## Three consequences worth writing down

**Row count is the real cost.** A constant conclusion collapses many readings into one row — `grade = A` is the same value however many readings satisfied it. A computed one produces **a different value per combination of readings, each on its own interval**: two revenue readings and three cost readings are six conclusions. `MAX_COMBOS` (64) today bounds premise expansion; here it bounds *output*, and the cap needs measuring against a real base before this ships rather than after.

**Units and datatypes have to be checked when the expression is written, and today nothing checks them.** `relation_types` carries `unit` and `datatype` and no code compares them. `revenue (USD) − cost (EUR)` must be refused by the picker, not silently subtracted; the result's type has to match the concluded predicate's. This is new work that the constant case never needed.

**Revision proposed 2026-09-21:** [0049](0049-expression-declarations-are-checked-when-a-rule-is-written.md) answers the missing declaration semantics and write-time locking question below. Missing units are not assumed unitless; exact `1`, the allowlist and first-cut operations remain proposals. The accepted expression semantics and metadata-only fallback are unchanged.


**A missing reading is not a zero, and neither is a division by zero.** If any attribute in the expression has no reading on the interval, the expression has no value and nothing is concluded — consistent with 0029. Division by zero is the same: no conclusion, **reported** the way `capped` is, because "not computed here" and "the criterion was not met" look identical in the result otherwise.

## Open

- **How deep before the picker loses.** Stated above as a concession, not settled. One operator is certainly a picker; nobody has yet said what they need beyond that.
- **Does a computed conclusion feed the next rule?** It should — [0030](0030-a-rule-may-read-what-a-rule-concluded.md) puts a concluded value back in the fact pool and says nothing about how the value was arrived at. Worth a test rather than an assumption.
- **Rounding and display.** A ratio of two readings is a long decimal. What the ledger stores and what the panel shows are not necessarily the same, and neither is decided here.
