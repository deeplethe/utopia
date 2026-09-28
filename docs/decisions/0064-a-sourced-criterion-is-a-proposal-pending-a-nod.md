# 0064 · A sourced criterion is a proposal, pending a nod

- **Status**: In progress · accepted 2026-09-27 · cuts 1–2 built (#XXX, migration 0098) · open: cut 3 (Review card), the extraction cut (4), and the source-change/decline/withdraw behaviour Maya named
- **Written**: 2026-09-27 (conventions in the [README](README.md))
- **Related**: [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) (the table this extends); [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) (the rule is a contract a person signed); [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) (the proposal queue is to a rule what `pending_facts` is to a fact); [0002](0002-reasoning-engine.md) d5 (the model writes criteria, the line this record redraws); [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) (the documents carry the criteria); [0048](0048-provenance-references-stay-inside-the-knowledge-base.md) and [#901](https://github.com/deeplethe/utopia/issues/901) (the same-KB family the new source columns join); [#507](https://github.com/deeplethe/utopia/issues/507) (the original report); #725 (the queue redesign where this card will live).

> A well report says: *全烃大于 8 且解释结论为气测异常时，可判定为优秀井。* — a rule about a class of wells, not about any one well. The sentence has a page and a file; it has no entity. It is the kind of fact the extraction cannot land, and the kind of rule the ontology page can. Today it falls on the floor.

## Problem

The ontology's business rules live in `attribute_rules` ([0021](0021-a-rule-reads-attributes-and-concludes-a-type.md)). The shape is right for the rule — class, conditions, conclusion — slot for slot. What is missing is provenance: the table assumes a rule is something a person typed, and the column it does not have is `chunk_id`.

A criterion read out of a corpus is a fact about a document, not a rule of the base, until a person says so. It is exactly the distinction [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) drew for facts: `pending_facts` is where unnodded statements wait; the nod turns them into `facts`. The shape a sourced criterion wants is the same shape `pending_facts` has — same id of the sentence, same page, same file — and the same kind of gate.

[0002](0002-reasoning-engine.md) decision 5 drew a line: *"criteria are written by people and the model is ruled out."* The line is two claims, and only one is at stake:

- **The model invents a criterion** — refused; one criterion re-decides the base and no one can say where it came from.
- **The model reports that a document states a criterion** — this is a fact about a document, with a sentence, a page, a filename.

The line between them is **whether it has a source**, and the system has a great deal of source. Cutting the line at provenance leaves the bar where it has to be and lets extraction hand the corpus to governance.

## Decisions

**1. `attribute_rules` gains a source and a state.** Five columns on the existing table:

- `source_kind` `TEXT NOT NULL DEFAULT 'hand' CHECK (source_kind IN ('hand', 'text'))` — hand-written by a person or read out of a text by the model.
- `source_chunk_id` `UUID`, `source_document_id` `UUID` — where the sentence came from. NULL for `hand`.
- `proposed_at` `TIMESTAMPTZ`, `proposed_by` `UUID REFERENCES users(id)` — who proposed and when. NULL for `hand`.

And one axis that is independent of `enabled`:

- `state` `TEXT NOT NULL DEFAULT 'nodded' CHECK (state IN ('proposed', 'nodded', 'declined'))` — what the human review has done so far.

`enabled` keeps its meaning: whether a rule contributes to the typed graph. A hand-written rule a person switched off and a proposal nobody has looked at both read `enabled = false`, and they are two different things the review queue and the rules page have to tell apart; overloading `enabled` would make them indistinguishable in storage and on screen.

**2. `hand` and `text` have different shapes and a check enforces it.**

- `source_kind = 'hand'` ⇒ `state = 'nodded'` — writing the rule is the nod; there is no proposal to wait on.
- `source_kind = 'text'` ⇒ `state` is one of `'proposed' / 'nodded' / 'declined'`.
- `source_kind = 'text'` ⇒ both `source_chunk_id` and `source_document_id` are NOT NULL.

The chunk and the document must belong to the rule's KB — composite `(kb_id, source_chunk_id) REFERENCES chunks (kb_id, id)` and `(kb_id, source_document_id) REFERENCES documents (kb_id, id)`, with `ON DELETE SET NULL (source_chunk_id)` / `… (source_document_id)` so the rule row survives when the chunk is retracted. This is the same family [#901](https://github.com/deeplethe/utopia/issues/901) extended to post-0070 tables; `chunks` and `documents` already carry the `(kb_id, id)` UNIQUE constraint.

**3. Vocabulary that does not resolve stays in the proposal.** A criterion's conditions name attributes, classes, and units that this base actually has; the mapping layer ([0011](0011-a-mapping-is-not-a-fact.md)) and the ontology contract ([0012](0012-the-ontology-is-a-contract-not-a-suggestion.md)) do that work for facts and nothing does it for criteria. The question the original report named — what happens when a word does not resolve — is decided here: **hold the proposal with the unresolved words kept verbatim and the review card says so.** Refusing loses the source; holding keeps the sentence the document said.

**4. The review card is the nod.** The same shape the other queues use ([0043](0043-every-review-queue-is-governed.md), #725): the original sentence, the chunk, the rule it would become, the unresolved words (when any), and three actions — **approve**, **edit-and-approve**, **decline**, **defer**. Approve and edit-and-approve flip `state` to `nodded` and leave the rule's `enabled` to the person; decline flips to `declined` and records `reason`; defer leaves the row alone and reads the same card on the next visit. **Until the nod the row concludes nothing** — a `proposed` row is excluded from reasoning the same way `pending_facts` are excluded from typed facts.

**5. The nod keeps source and reviewer provenance.** Approving a `proposed` row does not change `source_chunk_id`, `source_document_id`, `proposed_at`, or `proposed_by`; it sets `state = 'nodded'`. Source changes (the chunk edited, the document superseded, the sentence moved) show up on the rule's page as a re-review prompt and do not silently rescind the human decision. A rule with a source is a rule whose origin is the passage; 0044's protection of human decisions is preserved.

## What it costs

- **Six columns and one index on `attribute_rules`**; no new table. The case for a separate `pending_rules` table was the case for symmetry with `pending_facts`; the case against is that one table is one fewer migration set to fail under any search path, the existing `enabled` column is already the rule's reasoning gate, and a `source_kind` column tells extraction "write here" and tells Review "show as proposal". A separate table would be one more thing for the eval and derivation paths to walk past.
- **Two composite foreign keys** joining the same-KB family. The pattern is #901's; the catalog guard and `keep_their_kb` already cover `attribute_rules`, so the only new code is the two `*_same_kb` constraints.
- **One partial index** on `(kb_id, source_kind, state) WHERE source_kind = 'text'` — the review queue walks it and nothing else does.
- **A review card**, behind cut 3. The schema can be tested with `proposals_for_review` and a manual nod through SQL; the card is its own cut.

## Dead ends

- **A separate `pending_rules` table mirrored on `attribute_rules`.** Two tables, identical shape, the nod moves a row from one to the other. The cost is one more migration set, one more path the eval/derivation/ledger walks past, and the source provenance that has to be copied or referenced. The case for one table is the case for a single place that answers "what rules does this base have".
- **Overloading `enabled` as the nod.** A person-switched-off hand-written rule and an unreviewed proposal both read `enabled = false`; mixing them in storage is mixing them on the rules page and in the review queue, which is the opposite of what 0043 says queues are for.
- **Refusing a proposal when its vocabulary does not resolve.** A criterion that names "全烃" against an attribute the base does not have is still a fact about a document; refusing loses the source and the sentence with it. Holding keeps both; the review card says what did not resolve.
- **Automatic approval above a confidence threshold.** The rule is a contract a person signed [0012]; an agent's confidence does not change that, and the regression of 0012 was exactly the case where it did.
- **Letting the agent write the rule's structure.** The proposal carries the rule's structure because extraction produced it, but structure is part of approval [0012, 0044 §4]: a property arrives with its domains and ranges, not as a bare label. The agent emits the proposal; the editor shows it; the person edits or accepts; the result is the rule.

## Cuts

1. **Done in this PR.** Schema: `source_kind`, `source_chunk_id`, `source_document_id`, `proposed_at`, `proposed_by`, `state` on `attribute_rules`; the two composite foreign keys joining the same-KB family; the partial index; the `keep_their_kb` trigger is already in place (0070); the catalog guard registers the new edges (0091 family). The decision record (this file) is in the same PR, numbered 0064. `CURRENT_SCHEMA_VERSION` is 89.
2. **Open.** `attribute_rules::set_source(pool, id, kind, chunk_id, doc_id, actor)`, `attribute_rules::propose(pool, …)`, `attribute_rules::nod(pool, …)`, `attribute_rules::decline(pool, …, reason)`, `attribute_rules::proposals_for_review(pool, kb_id)` — store-level helpers, no extraction change.
3. **Open.** Review card: the sentence beside the rule it would become, nod/decline/defer, the unresolved words (when any), the source provenance.
4. **Open.** Extraction cut: when the open contract emits a universally-quantified conditional against an existing attribute with a compatible unit and against an existing class, it writes a `proposed` row with its chunk and the reviewer in the gate. Maya's threshold, exception, narrative, and unit cases (#806) belong beside the rule tests that execute them; the existing engine tests do not establish the quality or correctness of this workflow.
5. **Open.** Source-change behaviour: chunk edited, document superseded, sentence moved — the rule's page reads as a re-review prompt; a person re-nods or declines. Source deleted: the row's source columns go to NULL but `state` is preserved (the human decision stands; the origin is marked "retracted" on the audit log). The decline/reject path writes `reason` so extraction does not propose the same sentence for the same signatures twice.

## Open questions

- The exact cell the review card shows for an unresolved word. A separate field, a tooltip, a banner — the difference is between "the word is a sentence" and "the word is a chip".
- A proposal that names a class the ontology does not have yet. Today the mapping layer would refuse; the proposal that names it is still a fact about a document. The right answer may be a `class_proposed` companion proposal that the same person nods.
- A rule whose source is no longer in the base (the document retracted, the chunk deleted). The source columns go to NULL; the rule stays nodded. The page should say so.
- A person re-editing a nodded, sourced rule. Today the rule loses `state = 'nodded'` and becomes a new proposal; the source provenance stays. The right behaviour may be "edits re-open the proposal", the same way 0053 made alignment re-decide a signature when the basis moved.

## Status history

> Accepted 2026-09-27 · cuts 1–2 built (#XXX, migration 0098): `attribute_rules.source_kind` (hand/text), `source_chunk_id`, `source_document_id`, `proposed_at`, `proposed_by`, `state` (proposed/nodded/declined); the two `*_same_kb` composite foreign keys join the family; partial index on `(kb_id, source_kind, state) WHERE source_kind = 'text'`; the check enforces hand⇒nodded and text-source-required; `enabled` keeps its meaning for in-force rules and the review queue walks the partial index; a sourced criterion's vocabulary that does not resolve stays in the proposal with the unresolved words kept verbatim; the review card (cut 3), extraction cut (4), source-change behaviour (5) open