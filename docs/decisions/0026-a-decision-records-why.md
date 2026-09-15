# 0026 · A decision records why

- **Status**: Implemented · migration 0038 adds `resolution_reviews.rationale`; every human decide path (`decide_review`, the batch, an answer to the agent, a manual merge from the entity panel) takes an optional rationale, keeps it on the row and writes it as `why` on the ledger event; `Precedent.why` carries it into the prompt lines and the `ledger_search` tool; the batch adjudicator now reads the same precedents and keys its verdict cache on them; ~~one confident pair in ten goes to a person~~ removed 2026-09-14 (decision 5, revised); the model's own `why` is written beside machine decisions
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: [0025](0025-governance-reads-the-ledger-before-it-decides.md) made governance read the ledger; this record makes what it reads worth reading. #356 asked for it. The impact gate (#357) and the investigating adjudicator (#358) build on it; a rationale vocabulary that converges is a rule in the sense of [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md).

> A person merges "Acme / Acme Corp" because the two share an address, and keeps "Zhang Wei / Zhang Wei" apart because one is a professor and the other a student. The ledger records merged, kept, by whom, when. The next morning the agent reads "a person merged Acme with Acme Corp" and proposes to merge "Acme Bank" with "Acme Corp", because the outcome is all it can see. One merge made in a hurry is read as a policy by everything that comes after it.

## What was there

`resolution_reviews.reason` records why a pair was **queued**: `namesake`, `contains`, `escalate_unsure`, written by the code that queued it. Nothing records why it was **decided**. The ledger event a human decision leaves (`review.merge`, `review.keep`, `merge.manual`, `merge.revert`) carries the two names, the score and the actor, and 0025 reads those events back as precedents. A precedent was therefore a verdict without a ground, and the model was told to follow it.

The batch adjudicator was worse off. 0025 gave the governor precedents and kept it off the verdict cache (decision 7) because a cached answer would be one taken without them; the adjudicator kept its cache and its ignorance, sending twelve pairs a call with no history at all. And since every pair it was confident about never reached a person, the corpus of human decisions was only the hard cases: neither representative of how a person judges in general, nor usable to measure how often the machine and a person agree.

## Decisions

### 1. A decision may carry a rationale

One free-text column, `rationale`, beside `reason`, so the two questions stay two columns: why it was queued, why it was decided. The four human paths take it: `decide_review` and the batch, an answer to the agent's proposal, and the merge-in dialog on an entity. Whitespace is null. It is asked as a question, "What told you?", in a single input beside the buttons, and it may be left empty. A required field yields "ok" and "dup" and teaches the model that people decide for no reason.

### 2. The rationale lives in the ledger too

Precedents are read from `audit_events`, not from the reviews table (0025 decision 1: a precedent is what a person did, and the ledger is where that is). So the rationale is also written as `detail.why` on the four events. The column is for the row a person is looking at; the ledger is for every reader that comes later, including the export.

### 3. A precedent quotes its reason

`Precedent.why` is read by `precedents_for` from `detail->>'why'`. `render_lines` appends `; they wrote: "…"` to same-pair, same-name and revert precedents; the type-pair habit stays counts, because a sum of reasons is not a reason. The `ledger_search` tool of the investigating loop quotes it the same way. The prompt says what to do with it: weigh the stated ground, not only the outcome; a decision made for a reason that does not hold for this pair is not a precedent for this pair. The `why` a person never wrote renders as before, so the 0025 corpus is unchanged.

### 4. The adjudicator reads precedents, and its cache knows

The batch adjudicator now asks `precedents_for` for each pair and sends the rendered lines with the pair, as the governor does. Its cache stays, but `pair_key` folds the rendered precedent lines into the hash. A new human decision changes the lines, the lines change the key, and the stale verdict is simply never found again. Nothing invalidates anything, and a base with no history keys exactly as before.

### 5. One confident pair in ten goes to a person anyway

`HUMAN_SAMPLE_PCT = 10`, chosen by `review id % 100`, so the same pair is sampled on every run and a run is reproducible. The pair is escalated with `escalate_sample|<verdict> <confidence>` instead of being applied; the card says it was sampled and what the adjudicator would have said, so nobody reads it as doubt. Two things come out of it: the human corpus keeps ordinary pairs in it, and every sampled row holds both the machine's verdict and the person's, which is the raw material for an agreement rate.

> **Revised 2026-09-14: no confident pair is held for a person.** The sample is removed. On the Blackbaud lease bench it held a pair the adjudicator had judged the same at 0.95, so the lease stayed split into several entities and questions about it came back with several answers. What goes to a person is now only what the machine is unsure of, or what the gate (0027) holds. The reason for the sample, an agreement rate against people, has another source: every agent decision a person answers (0025) already holds both verdicts. Rows escalated as `escalate_sample` before this change keep their label, and the governor takes them like any other waiting pair.

### 6. The machine's why is kept, and is not a precedent

`apply_verdict` writes the model's one-line `why` into the ledger on the merges and keeps it makes. `precedents_for` still reads only people's rows, as 0025 decided; the line is there so that Decisions shows what the machine went on, and so that a later reader can compare it with what the person wrote on the sampled pairs.

## What a reader sees

The duplicate card has an input beside Keep / Merge; the batch toolbar has one input for the whole batch; the Agent queue's answers take one; the merge-in dialog on an entity takes one. Decisions shows the quoted why beside the row, for people and for the machine. A precedent under an agent decision ends with the quote. A sampled pair reads "Sampled for a person; the adjudicator was confident (same 0.92)".

## Dead ends

- **A required rationale.** See decision 1.
- **A pick-list of reasons** (same address, subsidiary, different person). The list would be wrong for the next base, and free text costs nothing when the reader is a model. If a base's rationales converge on a vocabulary, that vocabulary is a rule and belongs in 0021's shape, not in a dropdown.
- **A `decision_rationales` table.** Two readers, two places; the ledger already is the second place, and the export already carries it.
- **Random sampling.** A coin makes a run unrepeatable, and a pair sampled on one run and not the next would be answered from the cache on the next.
- **Sampling inside the governor too.** The governor already proposes rather than applies whenever its gate says so, and a person answers every proposal; adding a blind sample there would double-ask. Revisit if the fuse (0025 decision 9) ever trips on pairs the gate applied.

## Open questions

- **Impact, not confidence** (#357). Answered by [0027](0027-an-automatic-merge-is-gated-by-what-it-can-undo.md): a merge that would leave the graph is held for a person whatever the confidence.
- **An adjudicator that investigates** (#358). Answered by [0028](0028-the-adjudicator-looks-before-it-asks.md): the batch escalates its unsettled pairs into the governor's loop, with governance off too.
- **The agreement rate.** The sample is gone (decision 5, revised). The rows a person answers under the agent (0025) carry both verdicts; nothing computes the rate yet.
- **History on a merge target.** The merge shows under the withdrawals it caused in the same second, which is honest chronology and hard to read. Folding consequences under their cause is presentation, and belongs with the rationale it now has.
