# 0028 · The adjudicator looks before it asks

- **Status**: Implemented
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: #358 asked for it and said when to build it. [0025](0025-governance-reads-the-ledger-before-it-decides.md) cut 2 built the loop this record reuses; [0026](0026-a-decision-records-why.md) keeps the sample that measures it; [0027](0027-an-automatic-merge-is-gated-by-what-it-can-undo.md) draws the boundary the loop now sees before it decides.

> Two records named "Mercury": one with three facts about a planet, one with none. The batch look says unsure at 0.55 and the pair goes to a person, who opens the source document, finds "Mercury, the messaging startup, raised…", and keeps them apart in ten seconds. The code could have opened the same document. With governance off, it never did.

## What the issue asked for, and what was known

#358 asked for an adjudicator that can go and look, several rounds, with tools it already has a backend for, and said not to build it until a number said which kind of miss the adjudicator makes: no comparable precedent (then retrieval is the answer), or evidence in a document nobody put on the card (then investigation is).

Two things happened since. 0025 cut 2 built exactly that loop for the governed path, with four tools and a daily budget, and 0025 decision 10 measured the whole governed path on 589 pairs against a hand-labeled set: 96.9% agreement, four wrong merges, eleven pairs left for people. Of what disagreed, most was judgment (whether "Claude Mythos 5" is "Claude Mythos"), two of the four wrong merges followed a precedent a simulated person left, and pairs with no facts on one side cannot be settled by anyone. That corpus did not say "the evidence was in a document nobody read"; it said the remaining misses are the kind more looking does not fix. This record does not claim the loop raises agreement on that corpus.

What it does claim is narrower and cheap, because the loop exists:

- A base with governance **off** had no second look at all. Everything the batch could not settle, the pairs below 0.8 and the ones without a verdict, went to a person unlooked-at. 0026 sends one confident pair in ten to a person on purpose; sending every unsettled pair with no attempt is not the same policy.
- The loop could not see what the consistency check sees, or what 0027's gate will hold. It could decide "same" on a pair whose merge would put two CEOs on one company, and only then be held.

## Decisions

### 1. One loop, both deciders

`look_again` in the governance job wraps the loop for the adjudicator: a fresh `run_id` per adjudication job, the same tools, the same `LOOP_DAILY_CALLS`. The pairs it takes are the ones the batch left unsettled (no verdict, or below `AUTO_CONF`) that a hard rule would not hold anyway (different type families, a version tail, a phrase containing a name: looking again cannot change a rule) and that carry no revert (that is a person's matter). The loop's verdict replaces the batch's in the cache, is applied through the same `apply_verdict` (so 0026's sample and 0027's gate still apply), and the ledger says `via: investigated`.

### 2. A look is a row, whatever the switch

Every second look the adjudicator takes is an `agent_decisions` row with its trace, its calls and, if it deferred, its question; `applied` when the verdict landed, `proposed` when the pair went to a person. Two consequences: the daily budget, which counts `calls` on those rows, is shared between the governor and the adjudicator; and the Agent queue shows what a machine looked at with tools regardless of whether governance is on. The switch turns the governor on, not the queue. A deferred pair reaches its card with the proposal chip and the question, and the person's answer walks the same path as an answer to the governor.

### 3. The loop sees the boundary before it decides

`consequences` takes no arguments and returns what `impact_of` returns, rendered: relations that allow one value where the sides hold different ones, derived facts resting on either side, chat answers that named either side, and whether the two types share a family. The prompt says what to do with it: a merge that would touch anything outside the graph is held for a person whatever the confidence, so prefer to defer with the question that would settle it. The model learns nothing new about identity from this tool; it learns what its decision would cost, which is the difference between deciding "same 0.9" and being held, and asking "is the CEO Alice or Bob?" and being answered.

### 4. Nothing else moves

The batch prompt is unchanged; confident batch verdicts never enter the loop (0026's sample covers them); the governor's own second-look rule is unchanged. The adjudicator's bar stays 0.8 and the governor's 0.85 / 0.75.

## What a reader sees

With governance off, an unsettled pair that the loop decided shows in Merges or Decisions with the machine as actor, and in the Agent queue as an applied row with its lookups. One it deferred shows on its card as "Agent: unsure" with the question, and in the Agent queue with "Asks: …"; the Merge / Keep on either answers it. The Agent queue's empty state still says governance is off, because it is.

## Dead ends

- **A second loop for the adjudicator with fewer tools.** Two prompts to keep aligned, two budgets, two trace shapes. The loop was already written against a pair and an earlier look; the adjudicator has both.
- **Looking before the batch, for every pair.** Up to seven calls per pair instead of one per twelve; the batch settles most pairs and the loop is for the rest.
- **An ontology tool that dumps the class hierarchy.** The pair already shows the types; what the model needs is whether the rules let them merge (families) and what the check would open (contradictions). Both are in `consequences`.
- **Recording adjudicator looks outside `agent_decisions`.** A parallel table for the same shape of row, invisible to the queue and to the budget.

## Open questions

- **The number.** 0026's sampled pairs carry the machine's verdict in the reason and the person's decision in the ledger, with `via` saying whether the batch or the loop produced the verdict. When enough of them exist, the agreement rate split by `via` says whether the loop earns its calls with governance off; nothing computes it yet.
- **Tools the loop still lacks** (0025's list): the graph beyond a side's direct facts, a full-text search of the corpus, the disambiguator history.
- **Whether a deferred question should skip the queue** when governance is off and land on the card only. Today it does both.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> Implemented · `consequences` joins the second look's tools (facts, quotes, ledger, namesakes): what a merge would touch, read from 0027's gate, and whether the two types share a family · with governance off, the batch adjudicator sends the pairs it cannot settle through the same loop under the same daily budget, and every look is a row in `agent_decisions` so the Agent queue and the budget see it · the number that will justify or retire it comes from 0026's sample, split by `via`
