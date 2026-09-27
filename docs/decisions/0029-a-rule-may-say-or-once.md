# 0029 · A rule may say "or", once

- **Status**: Implemented (migration 0039)
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) built the rule and made its conditions a conjunction; this record widens that shape by exactly one level and says why not further. [0002](0002-reasoning-engine.md) ruled out a user-defined rule language, which is the boundary this record stays inside. From #476.

> A criterion is rarely one pattern. A gas-bearing well is one whose total hydrocarbon is above 8 **and** whose interpretation reads as a gas anomaly — **or** one whose composite interpretation already says gas-bearing. Today that is two rules, with two names, concluding the same class, and nothing in the base says they are one criterion. Delete one and the other keeps firing; read the ontology and you cannot tell they belong together.

## The shape: groups, one level

A condition carries a **group**. Conditions in a group are joined with **and**; groups are joined with **or**. `(A and B) or (C)`. There is no nesting below that.

This is not a concession to keep the editor simple. It is the shape the evaluator already has.

A hit does not report "true". It reports **which facts made it true** and the interval on which they all hold — that is what `derived_facts` stores, what `fact_derivations` chains, and what the entity panel shows a person when they ask why. To get there, `evaluate()` collects the facts matching each condition, walks the cartesian product of those sets, and intersects their intervals (`validity()`), capped at 64 combinations because the product grows with re-reported readings. **That computation is a conjunction.** Running it once per group and emitting hits per group keeps every property it has: the premises are the readings of the group that fired, the interval is their intersection, and the conclusion retires when any of them changes.

An arbitrary `and`/`or`/`not` tree would have to answer a question with no good answer: *what are the premises of a disjunction?* Either the fired branch's (which is what groups already give, without the tree) or all of them (which would attach a well's conclusion to readings that had nothing to do with it, and retire the conclusion when an irrelevant reading changes). Depth beyond one level buys expressive power that nobody writing these criteria has asked for, at the cost of the one property this ledger exists to keep.

## Three consequences worth writing down

**The cap is per group.** A group whose combinations blow past 64 is skipped and reported; the other groups still conclude. The report counts a `(rule, entity)` pair once, not once per group — what a person needs to know is "something was not expanded here", and a second number would not change what they do about it.

**Two groups on the same interval are one conclusion.** They land on the same row: same subject, same predicate, same value, same interval. The evaluator dedupes by interval and keeps the premises of the **first group in group order** — so the proof is stable across runs rather than dependent on storage order. It is *a* proof, not the only one. Recording every path would grow the proof tree with the number of ways a criterion can be met, and a reader who asks "why is this well gas-bearing" is served by one true answer.

**Group numbers are not a sequence to maintain.** `(rule, group_seq, seq)` locates a condition; the API takes whatever integers a caller sends and the evaluator sorts them. Deleting the middle group of three does not renumber anything.

## `not in`, and the negation we are not doing

`not_in` joins the ops. It reads a value that **is** there and asks whether it falls outside a set, so it has a premise fact and an interval like any other condition. A reading of "water zone" satisfies "is not one of {gas zone}".

**"This entity has no such attribute at all" is not in this record, and not by oversight.** Two reasons, either one sufficient:

- It has no premise. Every derived fact in this system points at the readings that made it true and retires when they change. A conclusion drawn from *absence* points at nothing, so nothing can retire it — it would sit there until someone re-ran the job and noticed the absence had ended.
- The base is open-world. A missing reading means nobody wrote it down, not that the value is false. Extraction lags documents, documents lag the world, and a rule that concludes from silence concludes from the lag.

If it is ever wanted, it needs its own record answering what makes it true, what its premise is, and when it retires.

## What the page shows

The editor is blocks: rows inside a block joined by `and` written at the head of the row, blocks divided by a rule and the word `or`, one button per block to add a row and one at the bottom to add a block. A block is not drawn as a box — the connectives already say how far it reaches, and a border around a run of form fields is a panel spent on nothing (DESIGN.md 6). Deleting the last row of a block takes the block with it, which is why no block is ever empty except the first one before anything is written; an empty conjunction is true of everything, and there is no state in which one can be saved.

The rule reads as one sentence in the table — `A and B or C` — because it is one criterion however many ways it can be met. The schema diagram draws a rule as a single edge from the subject class to the concluded class, and that does not change: `or` is inside the criterion, not a second edge.

## Open

- **Chaining is a separate question** (#477): a rule still cannot read what another rule concluded, and this record does not change that. The two are independent — this one is the shape of one rule, that one is how rules feed each other.
- **Set membership is still literal.** `in` and `not_in` compare strings exactly, so the same category in another language does not match. 0021 left this open and it stays open.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> implemented · `group_seq` on `attribute_rule_conditions` (migration `0039`, existing conditions default to group 0 so every rule keeps its meaning), the evaluator runs group by group with the combination cap per group and one dedupe by interval across groups, `not_in` beside `in` · the API carries `group` on a condition and defaults it to 0, so a caller that sends a flat list still sends one conjunction · the rule editor writes blocks and the table reads the sentence back with its "or" in it
