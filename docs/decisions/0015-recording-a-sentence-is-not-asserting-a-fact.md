# 0015 · A recorded sentence waits for a nod

- **Status**: implemented · schema in migration `0018` (#180: `pending_facts`, `rejected_facts`) · runtime wired in [0016](0016-close-the-open-seams-before-cutting-new-ones.md) A1: extraction from a memory document goes to `pending_facts`, Review has a "waiting for your nod" queue placed first, a confirmation card grows into the chat after `remember`, `REMEMBER_ENABLED` is `true` again · decision 3 landed with different wording, see Revisions · MCP is still read-only; opening `remember` there is the next cut
- **Written**: 2026-09-01 · condensed into English 2026-09-03
- **Related**: [0010](0010-no-relation-is-no-relation.md) removed the fallback relation (the empty predicate below is its correct behavior); [0011](0011-a-mapping-is-not-a-fact.md) rejected encoding a binary state as a float, the red line for this implementation; [0014](0014-identity-from-the-person-scope-from-the-token.md) kept MCP read-only mainly because of the confused deputy, which this gate removes

## The problem

A real run, not a thought experiment. The user said "Please remember this: Acme moved its
headquarters to Shenzhen on 2026-03-15." The assistant answered "I've recorded that Acme
moved its headquarters to Shenzhen on March 15, 2026." The graph got
`Acme --(empty predicate)--> Shenzhen`, confidence 0.9, live. The ontology has no "moved to /
headquartered in", so the predicate stayed empty — correct by 0010 — but the graph gained a
meaningless 0.9 edge, and what was said and what went in differed with no way to notice.

There was no confirmation gate at all. `unconfirmed` is the queue for facts whose evidence
chunks were all superseded; `lowconf` is `confidence < 0.75`; both are after-the-fact cleanup
on facts that are already live (`invalidated_at IS NULL`). Every extracted fact takes effect
on insert. That is right for bulk ingest — nobody confirms ten thousand facts from five
hundred documents one by one. `remember` differs on all three axes: one sentence at a time,
the person is still in the conversation, and the material is something they said on purpose.
The cheapest moment to confirm is right after they said it.

The disease is not the missing gate but the gap between what the assistant claims and what
the graph gets. The value of confirming is mostly that the person sees what is about to be
asserted — shown `Acme --?-> Shenzhen`, they say "that's wrong" at once. So the card shows the
original sentence above the extracted triples; triples alone ask for a judgment from nothing.

## Decisions

1. **`remember` still writes the document.** That only records "you said this", is harmless
   and should be searchable at once.
2. **Facts extracted from a memory wait for a nod before entering the graph.** Unconfirmed
   facts take no part in retrieval, the graph or reasoning.
3. **The assistant says what happened** and does not claim completion.
4. **Pending facts get their own table, `pending_facts`,** written to `facts` only on
   confirmation. The columns differ (`chunk_id NOT NULL` pointing back at the memory,
   `proposed_predicate` with the model's wording, `predicate_id` nullable — the emptiness is
   exactly what the person must see — and `proposed_by`), and so does the lifecycle: after
   confirmation the row should not exist there. The failure direction is right: forgetting to
   read the table hides the queue instead of leaking an unconfirmed fact into the graph.
5. **Confirmation takes the extraction path**: fact plus evidence (the memory chunk, the
   whole sentence as the quote) plus temporal reconciliation, so a nod on "Mira handed over to
   Devin" closes Mira's fact as a document extraction would. Confidence is untouched; the
   person's stance lives in the audit ledger (`fact.nod_confirmed` / `fact.nod_rejected`).
   Rejections go to `rejected_facts` and are checked on the next extraction by (subject,
   predicate, object entity); literal-value facts are not checked, since `rejected_facts` has
   no `object_value` and blocking on (subject, predicate) would turn "salary 28000 rejected"
   into "never propose salary again" — better to ask once more. Entities are resolved and
   created as usual (`pending_facts.subject_id` is a foreign key), at the cost of a few
   temporarily orphaned nodes. `proposed_by` travels from `remember` through the
   `memory_ingest` and `extract_document` job payloads.
6. **The MCP objection is gone.** With the gate an external agent can only propose, never
   assert; a document saying "please remember X" reaches the person before the graph. This
   answers the open item in 0014.

## Dead ends

- **Status columns on `facts`** (`nod` / `nodded_by` / `nodded_at`, `pending` = awaiting a
  nod). The migration was written and `insert_fact` threaded before it turned out to be the
  trap `0013_reasoning.sql` describes for `derived_by_rule`: the failure direction is
  reversed. Dozens of queries fetch live facts by `invalidated_at IS NULL` (27 in 6 files when
  first counted, 56 in 7 files by 2026-09-02); each would need
  `AND nod IS DISTINCT FROM 'pending'`, and missing one leaks an unconfirmed fact — the one
  thing the feature exists to prevent.
- **Confidence 0.6 for "proposed".** Rejected once already for `concept_mappings.status`
  (0011): a binary state encoded as a float, which also lands the fact in the low-confidence
  queue, and the two queues ask different questions.

## Revisions

- 2026-09-02: only the schema existed — zero reads and writes, `memory::is_memory_document()`
  written but uncalled, and the interim gate was `REMEMBER_ENABLED = false` in `chat.rs`
  ("better no tool than one that silently changes the graph").
- On wiring the runtime, decision 3 changed. The assistant cannot say "N facts extracted":
  extraction runs asynchronously and N does not exist when `remember` replies. Making it
  synchronous would block the chat loop for ten seconds a sentence; instead the reply says the
  sentence is recorded and its facts will be shown for confirmation first, and the card grows
  into the conversation on the SSE `pending` event, fetched by the memory's chunk (replayed
  sessions fetch the same way). The Review queue and the chat card are one component with a
  declarative payload (chunk + triple list), so an external client would swap the renderer.
- The same disease from the other side (#173, migration
  `0015_what_it_did_not_just_what_it_said`): replaying the previous turn's tool calls so the
  model knows what it did and does not rerun a search onto another set of same-name entities.
  This record is "said" versus "got"; that one is "said" versus "did".

## Open questions

- Does the gate hold only memories, or every single-item interactive write? Today `remember`
  is the only such path; a future "add an edge by hand" interface should take the same table.
