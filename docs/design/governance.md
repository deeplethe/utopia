# Governance decides from the ledger and can be undone

Records: [0025] (the governor), [0026] (why), [0027] (the impact gate), [0028] (the second look),
[0043] (every queue, in PR #699), [0015] (nods), [0017] (contradictions as a queue), [0016] C2 and
[0003] d4 (switches), [0001] P4; the queue redesign is issue #725. The identity rules are in
[identity](identity.md), the temporal actions in [time](time.md).

## What it does today

**Queues, grouped by where an item came from.** Review holds nods (`pending_facts`, placed first),
duplicates (`resolution_reviews`), conflicts (`fact_conflicts`), unconfirmed (every evidence chunk
superseded), low-confidence (`confidence < 0.75`), violations (`axiom_violations`), defects
(`ontology_defects`) and alignment (a phrase signature or a kind word the aligner's two votes
split on, `phrase_bindings` and `type_bindings` in `undecided`; a person binds it to a property
with a direction or to a class, or says none fits, and the typed graph recomputes at once; the
aligner never overrides a person, #754), plus the Agent queue; ontology proposals sit on the ontology page and mappings
on their own page [0011, 0015, 0017, 0025]. Every human decision is a ledger event with an actor
(`review.merge`, `review.keep`, `merge.revert`, `fact.*`, `conflict.*`) and may carry a `rationale`,
asked as "What told you?", optional, written as `why` on the event [0026].

**The governor.** `knowledge_bases.governance`, on by default; the `govern` job is enqueued when the
switch turns on, after an extraction that leaves pairs, after a person's decision, and hourly for
backlog, at most one waiting per base; it re-reads the switch before every cluster [0025 d2]. A
round takes up to three clusters from the head of a first-in-first-out queue, each cluster the
pairs sharing a name or entity with its head, up to twelve, one model call per cluster, results
applied in order [0025 d3]. The prompt carries precedents from the ledger: what people decided on
these names, on either name, on this type pair, and any reverted merge; only rows with an actor
count, and a precedent quotes the person's `why` [0025 d1, 0026 d3].

**The gate** is a pure function of verdict, confidence, type conflict and precedents. History that
contradicts the verdict blocks; history that agrees lowers the bar from 0.85 to 0.75; otherwise the
agent's confidence decides, merging and keeping alike; conflicting type families never merge; a pair
people already decided is applied as they decided [0025 d4 revised]. A `same` verdict on a version
or phrase shape, or a `different` verdict on identical compatible names, goes to the second look
first [0025 d10]. Then the impact gate: a merge that would put two values of a functional predicate
on one entity at one moment, that a live derivation rests on, or whose side an assistant answer
named, is held for a person as `escalate_impact` whatever the confidence; keeps are not gated
[0027]. Whatever does not clear the gates becomes a proposal.

**The second look.** A pair the batch cannot settle gets one conversation with tools: the full facts
of a side, source passages, a substring search of the ledger, namesakes, and `consequences` (what a
merge would touch, whether the types share a family); at most six lookups, then `decide` or `defer`
with the one question a person could answer; a daily budget of loop calls per base; the trace is
stored under the row [0025 d8, 0028 d3]. With governance off the batch adjudicator sends its
unsettled pairs through the same loop, and every look is an `agent_decisions` row [0028].

**Every look is a row.** `agent_decisions` holds the pair or target, the action, the confidence, the
model's sentence, the precedents shown, the question, the trace, the calls and a status: `proposed`,
`applied`, `accepted`, `overridden`, `reverted`, `superseded` (a person decided a sibling, so the
pair returns to the queue with the new precedent); one open proposal per pair. Answers go through
the person's own decide path, so an answer is a person's decision and a precedent [0025 d5, d6].
The machine's `why` is written on its merges and keeps and is never a precedent [0026 d6]. The
governor never touches the adjudicator's verdict cache; the adjudicator reads precedents too and
keys its cache on them [0025 d7, 0026 d4].

**The fuse.** Two reverts of the agent's merges within seven days of the switch turning on turn it
off, raise `governance.tripped` and write a `kb.updated` row with the machine as actor; overrides
and merges over a keep do not count [0025 d9].

**What a person sees.** A cluster being decided is locked (stage, job and switch together); the
Overview says how many pairs wait; a duplicate card carries the proposal chip whose Merge / Keep
answers it; the Agent queue lists rows newest first with precedents and trace; a held pair says why
[0025, 0027]. No confident pair is held for a person: the one-in-ten sample was removed on
2026-09-14 because it kept a lease split [0026 d5 revised].

**Contradictions are an audit.** A derivation that collides with an assertion does not land and
becomes a `derived_contradiction` violation with a clue (stale, wrong merge, unsure extraction) and
repairs (close at a date, retract, go to duplicates, relax the ontology, accept); derived against
derived aggregates per rule pair as `rules_disagree`; a disputed fact is marked where it sits [0017].

**Switches.** `auto_extend_ontology`, `auto_type_resolution`, `materialize_inferences`, `governance`:
all default on; every automatic action is visible and reversible, and a switch governs action,
never attention [0003 d4, 0016 C2, 0002 d6 revised, 0025 d2 revised]. Nods stay with people [0015].

**Measured.** `scripts/bench/govern.mjs` against 411 hand-labelled name pairs: 97.7% decided on its
own, 96.7% agreement, 6 wrong merges (4 of them one flipping pair), 12 left for people, 13 minutes
for 589 pairs [0025 d10].

## Why

- **The ledger is the precedent and only people write it**; the agent's own rows would let it cite
  itself and grow more confident every round [0025].
- **History as reference, not prerequisite**: with precedent required a fresh base merged nothing and
  200 pairs waited [0025 d4 revised].
- **Merging moves facts; a keep is undone by the next merge**, so only merges are gated, and a merge
  that leaves the graph is held because a revert restores the graph and not the world it was read
  into [0027].
- **A held pair is a reason, not a doubt**, and the card says so [0027].
- **A rationale is free text and optional**; a required field teaches the model that people decide
  for no reason, and a pick-list is wrong for the next base [0026].
- **The verdict cache must know the precedents** or a cached answer is one taken without them
  [0025 d7, 0026 d4].
- **Two reverts in a week** is the strongest signal the ledger has [0025 d9].
- **Written identity rules, two of them mechanical**, because the model's own decisions were wrong
  in patterned ways; a shape raises a doubt and only evidence settles it [0025 d10].
- **A contradiction points upstream**; every button is a repair, and a per-item queue needs a cap
  [0017].
- **A nod belongs to the person who said it**; an agent nodding turns a remark into an assertion
  [0015, 0043].

## Proposed and not built

- **Every queue is governed** [0043, PR #699]: the same governor walks low-confidence and stale
  facts (batches of eight) and then conflicts after its duplicate rounds, each with its own actions
  through the people's store paths (`confirm`, `reject`; `close_old`, `retime_new`, `keep_both`,
  `reject_new`); a date must be in the evidence and a stale fact is confirmed only by a quote in the
  current version; every action records its undo in `detail`; reverts of any kind count for the
  fuse. #725 splits the PR: the temporal fixes merge first, the conflict actions become the
  conflicts queue, the low-confidence handling is dropped. Cut 2 (violations, defects, mappings)
  is open.
- **Queues organised by what a decision changes** [#725]: ontology proposals; alignment (a
  signature or rule the aligner cannot settle; the signature half is built, #754); time anchors (a missing or conflicting document
  date, an unanchored mention); identity (pairs evidence cannot settle, merges the gate held);
  typed-fact errata; cross-document conflicts; nods. Agents handle every item; people see what an
  agent could not settle or what one decision resolves at once. Cards are one template per queue,
  assembled by code from the ledger with zero model calls at review time; actions are approve, edit
  and approve, reject, defer, make a rule; ordering by leverage, then impact, then age; metrics of
  leverage, agreement, revert rate and backlog tune the gate. Retired: the low-confidence and
  unconfirmed queues (unconfirmed becomes an automatic re-attach with an alert), mappings leave the
  Review page. Kept: `agent_decisions`, `execution_gate`, the temporal engine, name facts,
  `pending_facts`, the audit ledger. Sequence: after 0044 cuts 1 and 2, with no legacy layer.
- The errata agent on the typed graph through the gate [0044 d7] is built (migration 0074): its
  held actions are the errata queue on the Review page; see [design/ontology](ontology.md).
- Decision memory as retrieval and corrections aggregated into ontology signals [0001 P4b, P4c].

## Open questions

- A budget for the batch (the loop has one) [0025]; how often the impact gate holds on a base with
  reasoning on [0027].
- The agreement rate split by `via` now that the sample is gone: nothing computes it [0026, 0028].
- Tools the loop lacks: the graph beyond direct facts, full-text search, disambiguator history
  [0025].
- Whether an agent may ever write a rule, and through what pending queue [0021].
