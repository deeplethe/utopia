# 0027 · An automatic merge is gated by what it can undo

- **Status**: Implemented · `execution_gate` in the store: `impact_of` reads what a merge would touch, `hold` is the pure judgement; the batch adjudicator and the governor both ask it before an automatic merge, and a held pair goes to a person as `escalate_impact|<kind> <value>`; the governor records its look as a proposal whose reason begins `held for a person` · exports and a per-deployment opt-in stay open · the roadmap's execution gate for agents' calls is this module's next caller
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: #357 asked for it. [0019](0019-the-second-clock-can-be-rewound.md) made a merge undoable inside the graph, which is what makes inside and outside different things. The 0025 gate (confidence, precedents, two hard rules) is unchanged; this one sits after it. The things a merge can reach are [0002](0002-reasoning-engine.md)'s derivations, [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md)'s violations and the alerts of [0005](0005-alert-center.md), the answers of the chat, and the exports of [0020](0020-an-auditor-reads-it-without-us.md).

> The adjudicator is 0.93 sure that "Acme" and "Acme Corp" are one company, and it is right about that often enough that 0.8 is the bar. It merges them. The consistency check runs and finds one company with two CEOs; the reasoning engine rewrites what "part of Acme" implies; someone asks the chat who runs Acme and gets a cited answer. On Monday a person reverts the merge. The graph is as it was. The violation that was looked at, the derivations that were read and the answer that was given are not.

## What was there

`AUTO_CONF = 0.8` in the adjudicator, 0.85 and 0.75 in the governor, applied to every pair regardless of what the merge goes on to touch. Confidence was the only axis. Rollback is real but stops at the graph: `revert_merge` moves the facts back and `entity_merges` keeps both clocks, so the graph as it stood before the merge can be read again. Anything the wrong merge caused while it was live cannot be recalled: a derivation drawn on it, a violation it opened, an export it entered, an answer it was cited in. Undo restores the graph, not the world the graph was read into.

## Decisions

### 1. A merge is gated on what it would send out of the graph

Three things, checked in this order, and any one of them holds the pair for a person whatever the confidence:

- **A contradiction.** The two sides each hold a fact under the same `functional` predicate pointing at different entities (or, for `inverse_functional`, are each pointed at by different subjects). Merged, that is one entity with two values, which the consistency check reports as a `functional` violation the next time it runs. The check is the first place a merge leaves the graph, so it is the first thing asked.
- **A derivation.** Any derived fact that still stands and has either side as subject or object. The merge changes its premises and the engine rewrites it; what was read from the old row is gone.
- **An answer.** Either side was recognised in an assistant turn (`conversation_messages.resolved`, the entities a turn settled on). That entity is something people ask about, and a wrong merge enters the next answer.

Nothing touched, and the confidence bar decides as before.

### 2. Only merges

A keep costs nothing to undo: the next merge is the undo, and nothing outside the graph is told that two things were kept apart. The gate does not look at keeps.

### 3. One gate, two callers, a third on the roadmap

`execution_gate::hold` is a pure function over an `Impact`; `impact_of` is three queries, run only when a decider is about to merge on its own. The batch adjudicator and the governor call the same two functions. The roadmap's execution gate, "checking an agent's calls against ontology rules and symbolic logic", is this gate pointed at someone else's writes rather than our own, and joins this module rather than growing beside it.

### 4. A hold is a reason, not a doubt

The queue reason is `escalate_impact|<kind> <value>`: `contradiction CEO of`, `derived 12`, `answered 3`. The card says "Held for a person; the merge would not stay in the graph — it would put two "CEO of" facts on one entity", so nobody reads it as the adjudicator being unsure. In the governor the look is still recorded, as a proposal whose reason begins `held for a person: …`, with the model's own reason after it; the person answers it like any other proposal.

### 5. The same yardstick as the consistency check

The contradiction query looks at entity-object edges only, ignores time and ignores literal values, because that is what `too_many` in the reasoning crate looks at. A stricter predictor would hold pairs for violations the check would never open; a looser one would let through the ones it would.

> **Revised 2026-09-14: the contradiction reads time, as the check has since #635.** The consistency check started asking whether two values hold at the same moment in #635, and this query did not follow. The cost showed on the Blackbaud lease bench. The landlord changed from HPBB1 to BBHQ1, and the lease came out under two names, one holding each landlord. The gate called that pair a contradiction and held it for a person every run, so the lease stayed split and questions about it came back with two answers.
>
> `temporal::merge_would_overlap` now takes both sides' rows on each unique state predicate as one timeline and ends each row the way the temporal engine would (0022, #679). A contradiction is two values that still hold at one moment: the same start, a value with no time, a successor too doubtful to take over, or two stated intervals that cross. A succession is not one. Event and eternal predicates are not placed by the engine and are compared as their rows stand. The query still reads entity-object edges only.

## What a reader sees

A pair the adjudicator would have merged shows in Duplicates with the held reason in place of a confidence. In the Agent queue the same pair is a proposal that begins "held for a person". Nothing changes for pairs that touch nothing.

## Dead ends

- **Exports as a hold.** `kb.exported` is in the ledger, but an export is of the base, not of the pair: holding every merge in a base that has ever been exported is a switch, not a gate, and the switch already exists (governance, or the adjudicator's own). A per-deployment opt-in, "automatic merges may leave the system", waits until someone asks for it.
- **Size as impact.** The number of facts that would move says how visible a mistake is, not whether it comes back; `revert_merge` returns a hub as fully as a shell. And the rows people answer under the agent (0025) measure how often the machine is wrong, which is the number size would have been a proxy for; 0026's one-in-ten sample did this until it was removed on 2026-09-14.
- **A higher bar instead of a hold.** 0.95 for pairs with derivations, say. A held pair costs a person one look; a merge that leaves the graph costs a revert that does not fully revert. The asymmetry is the whole point of the issue.
- **Time-aware contradictions.** Two CEOs in different years are not a contradiction in the world. Both the check and this query now know it (decision 5, revised 2026-09-14).

## Open questions

- **Alerts as such.** The alert center's kinds are about pipelines and switches, not the graph; the path from a merge to an alert runs through the consistency check, which is why the contradiction stands in for it. If a direct path appears, it joins `impact_of`.
- **Answers that cited a side without resolving it.** `sources` carries chunks and facts; `resolved` is the honest entity-level signal today.
- **How often it holds.** Run the bench with reasoning on and count `escalate_impact`; if derivations hold most of a base, the derived rule wants a finer question than "any".
