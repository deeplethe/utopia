# 0043 · Every review queue is governed

- **Status**: Decided 2026-09-14 · **no code on `dev`** · cut 1 was built in PR #699 — migration 0058 widening `agent_decisions` to `fact` and `conflict` with `summary` and `detail`, `queue_agent` walking low-confidence and stale facts then temporal conflicts after the duplicate rounds of the same `govern` job, every applied action revertible from the Agent queue — and that PR was **closed on 2026-09-17**, to re-land on the open graph once [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) cut 2 has settled alignment · violations, ontology defects and concept mappings are cut 2 · **the decisions below stand; only the code was withdrawn**
- **Restored to `dev` 2026-09-20.** The file left `dev` with PR #699 and its number stayed allocated and cited — [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) links to it, `docs/design/governance.md` and `docs/design/time.md` reason from its decisions 5 and 1, and `docs/design/README.md` has carried it as `proposed` throughout. A record is the reasoning, and the reasoning was never withdrawn, so the file belongs here whatever happened to the branch. The status line above is the only thing rewritten.
- **Written**: 2026-09-14 (conventions in the [README](README.md))
- **Related**: [0025](0025-governance-reads-the-ledger-before-it-decides.md) (the governor this extends; its open question "Other queues"), [0026](0026-a-decision-records-why.md) (a person's why becomes a precedent), [0027](0027-an-automatic-merge-is-gated-by-what-it-can-undo.md) (act on what can be undone), [0022](0022-an-unknown-date-is-not-an-open-one.md) (the temporal engine whose conflicts this settles), [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) (nods, which stay with people), #695

> On the Blackbaud lease bench the timeline held a deadline back because the model had marked the amendment's new date 0.7. A conflict waited for a person, and the as-of question came back with two answers. The duplicate queue had an agent; the conflict queue and the low-confidence queue had nobody but a person who never came.

## Why a decision is needed

0025 gave one queue, duplicate pairs, to an agent that reads the ledger before it decides. The others kept waiting: temporal conflicts, low-confidence facts, facts whose evidence a new version replaced, axiom violations, ontology defects, concept mappings. On a base nobody curates they only grow, and what they hold back is real. A low-confidence successor may not take over from its predecessor (0022), so the graph answers with both. Decided on 2026-09-14: every queue is governed, except nods.

## Decisions

### 1. One governor, every queue but nods

The `govern` job runs the duplicate rounds of 0025 first, then the fact queues, then conflicts. The order is causal. A merge changes which facts share a timeline, and a confirmed fact withdraws the conflict it was held in (decision 5). The same switch starts and stops it, the same fuse (0025 decision 9) counts reverts of any kind, and the same hourly scan finds bases with backlog. Extraction enqueues the job after every document on a governed base, since conflicts and low-confidence facts come out of extraction, not only duplicate pairs.

Nods (`pending_facts`, 0015) stay out: a remembered sentence becomes a fact when the person who said it nods, and an agent nodding for them would turn a remark into an assertion. 0025 said the same.

### 2. Each queue keeps its own actions, and they are the people's

Facts: `confirm` or `reject`. Conflicts: `close_old`, `retime_new`, `keep_both` or `reject_new`. Each action calls the store function a person's click calls (`temporal::set_confidence`, `temporal::retract`, `temporal::resolve_conflict`, `temporal::correct_interval`), so an agent's decision and a person's leave the graph in the same shape. `retime_new` is the one action people reach through the interval editor, not the conflict card. It exists because the commonest simultaneous conflict on contract chains is a successor whose start the model took from the wrong date.

A decision is a row in `agent_decisions` with `target_kind` saying which queue. `summary` holds what the item looked like when decided: a fact or a conflict has no pair of names to join later, and its rows get rewritten. `detail` holds the action's parameters and what undoing it needs.

### 3. The model decides; the server checks what can be checked

One call per batch of eight, each item with its evidence (document, the document's own date, the quote) and the recent human decisions of that queue as precedents. The rules on precedents are those of 0025 and 0026. The model returns an action, a confidence and a sentence; at `AUTO_CONF` it is applied, below it is a proposal. Nothing the model says about the world is taken on trust where the server can check it:

- A date for `close_old` or `retime_new` must be in the evidence: the fact's start, a document's own date, or a day the quote writes out.
- A stale fact is confirmed by quoting the current version of its document, and the quote must appear there. The quote is then attached as evidence, which is what takes the fact out of the stale queue.

A check that fails turns a confident verdict into a proposal that says why it was held.

### 4. Every action can be taken back

An applied decision records its undo in `detail`:

| Action | Undo |
|---|---|
| Confirm a low-confidence fact | Restore its prior confidence |
| Confirm a stale fact | Remove the attached evidence |
| Reject a fact | Restore the fact (`temporal::restore`) |
| `close_old`, `retime_new` | Invalidate the rewritten row and restore the original (`temporal::undo_rewrite`); the original arrives again, so its conflict is recorded again |
| `keep_both` | Reopen the conflict |
| `reject_new` | Restore the fact and reopen the conflict |

A person answers from the Agent queue: accept or override a proposal, or revert an applied decision. An override runs the person's action through the same path and becomes a precedent.

### 5. Confidence is part of the timeline

Confirming a fact changes whether it may take over, so `set_confidence` locks the fact's timelines, changes the value and recomputes them, as a retraction does. A `low_confidence` conflict whose pair the recompute no longer holds is withdrawn. The question it asked, "may this doubtful value take over", has been answered.

## What a reader sees

Agent rows for facts and conflicts carry their summary in place of two names. A proposal is answered with that queue's own actions, and an applied decision offers Revert whatever it did. The ledger rows are the queue's own actions (`fact.confirm`, `conflict.close_old`, …) with no actor, like the duplicate governor's.

## Dead ends

- **One generic "approve / dismiss" action for every queue.** The queues differ in what a decision does to the graph: closing a value and keeping both are different graphs. A generic approve would have to map back onto each queue's actions anyway, and the model would decide without knowing which one it was choosing.
- **Letting the model set the close date freely.** It is the one thing a successor's evidence can be checked against, and the commonest mistake it would make is the one the conflict came from.
- **Resolving the held conflict when a fact is confirmed.** The confirmation doesn't know which conflicts it answers; the recompute does, because it is the thing that held the pair.

## Open questions

- **Cut 2: violations, ontology defects, concept mappings.** A violation's `fact_retracted` and `fact_closed` fit this shape. `axiom_relaxed` changes the ontology, and `fixed` for a defect means a person changed it; both need a gate before an agent applies them.
- **The second look.** The fact and conflict queues have no tool loop yet (0025 decision 5); the batch sees the evidence directly. Add one when proposals keep asking for something the batch could not see.
- **The interface.** The Agent queue shows these rows with minimal changes; the queue cards themselves don't yet show the agent's proposal inline, as duplicate cards do.
