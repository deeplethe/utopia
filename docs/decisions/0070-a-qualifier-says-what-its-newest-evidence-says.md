# 0070 · A qualifier says what its newest evidence says

- **Status**: Proposed · 2026-10-16 · [#1267](https://github.com/deeplethe/utopia/issues/1267) · open: all four decisions
- **Written**: 2026-10-16 (conventions in the [README](README.md))
- **Related**: [0037](0037-a-relation-carries-its-own-attributes.md) decided that a qualifier is not part of a fact's identity and that the first value stays; [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) owns the open statement and the role words on it; [0054](0054-a-source-may-push-statements-in-the-open-contract.md) decision 7 is the re-push that supersedes a chunk; migration `0061` created `statement_qualifiers`; [0048](0048-provenance-references-stay-inside-the-knowledge-base.md) gets one more edge to keep inside the base; [0019](0019-the-second-clock-can-be-rewound.md) is the record axis `as_of` reads on; [#1267](https://github.com/deeplethe/utopia/issues/1267), [#1272](https://github.com/deeplethe/utopia/issues/1272)

> A CI job pushes a test as a statement: `test-parses-a-date covers parse.rs`, qualifier `lines: "40-52"`. A week later lines are added above the test and the job pushes the same `external_id` with `lines: "58-70"`. The push is accepted, a new version is recorded, the chunk is replaced. The statement still says `40-52`, and will say it forever. The same goes for a `status` that went from `failing` to `passing`, or a `mood` that a later observation drops.

## The problem

`add_statement_qualifier` writes `ON CONFLICT (fact_id, role) DO NOTHING`. That was 0037's rule for typed qualifiers, carried over to the open layer by migration 0061: two observations that disagree should become two rows and a conflict, and until that cut exists, the first value stays. For a document that is mentioned once, the rule is invisible. For a source that pushes the same observation again with a corrected value (0054), it is wrong every time.

Three facts of the code make it so:

- **The statement survives the re-push.** `insert_open_statement` dedups open statements on `(kb, subject, phrase, object | value)` among live rows. A re-push with the same subject, phrase and object finds the existing statement and only adds evidence on the new chunk. The new role word arrives on a fact that already has one, and is dropped.
- **The old chunk is superseded, not deleted.** 0054 decision 7: the earlier chunk is marked `superseded_at`, and a statement whose evidence is only on superseded chunks is stale, handed to review, not invalidated. The statement's identity is right to survive; its role words are not right to stay frozen.
- **Typed rows copy once.** `materialize` copies a statement's evidence and qualifiers onto a typed row only when it creates the row. A typed row made from the first push keeps the first push's role words, and its only evidence is the superseded chunk, so every read marks it stale although the statement it came from was just confirmed. The three `q.role = 'mood'` checks look at `statement_qualifiers` as a whole, so a `mood` written once can never be taken back.

## Decisions

**1. One qualifier row per evidence chunk.**

`statement_qualifiers` gains `chunk_id`, a reference to `chunks`, and its key becomes `(fact_id, role, chunk_id)`. Each passage that states a role word records it against itself, the way `fact_evidence` already records one row per chunk. Writes stay append-only and idempotent: the same push twice writes nothing new, a new push writes rows for its new chunk and touches no old row. Existing rows are backfilled with the fact's earliest evidence chunk, which is the passage the first-written value came from. A row with no chunk remains allowed, for a fact with no evidence at all.

`chunk_id` is a new provenance edge, and 0048's invariant covers it: the owner-derived trigger `statement_qualifiers_same_kb` checks that the chunk's document is in the fact's base, and the export preflight scans the edge.

**2. The newest evidence speaks for a role.**

A read asks which rows speak for `(fact, role)`:

- the rows whose chunk is live at the read's `as_of` (`chunk_live_at`, and `document_live_at` for the chunk's document);
- if the fact has no live evidence at all (a stale fact), the rows on its newest evidence;
- rows with no chunk, only for a fact with no evidence.

Among those, the newest evidence wins: the evidence version's `doc_time`, else the chunk's `created_at`, then `doc_version`, then the chunk id, all descending. This is one SQL fragment beside the `record_axis` helpers, and every reader of role words uses it: the entity panel, the tools, the export and `materialize`.

What follows:

- A role the new payload drops disappears from the read, because no live row carries it. The old row stays in the table, as everything in the ledger does.
- Deleting a document supersedes its chunks; restoring it brings them back. The rule reads chunk and document liveness, so neither needs a recomputation.
- Two live documents that give different values for one role: the newer one wins. That hides a disagreement the ledger would rather show; see the open questions.

**3. A read in the past shows what was said then.**

An `as_of` read applies the same rule at that instant on the record axis. A role word that changed shows the value its live evidence had at `as_of`, not the current value. In the example above, a read at a moment between the two pushes says `lines: "40-52"`; a read today says `58-70`. This is what `chunk_live_at` already does for evidence, where a passage replaced today is still live evidence on a graph read as of March, and a role word is part of that evidence.

**4. `materialize` keeps typed rows in step.**

On each run, scoped to the base it runs for, `materialize` copies onto every live typed row the evidence and qualifier rows of its source statements (`typed_fact_sources`) that the row does not have yet. The copy is the same insert it does today at creation, `ON CONFLICT DO NOTHING`, now run for existing rows too. A typed row then reads its role words through the same rule as its statement, and is stale exactly when its statements are.

The three `mood` checks use the rule instead of any row: a statement has a mood when a row that speaks for `(fact, 'mood')` exists. A re-push that adds `mood` retires the typed row on the next run; one that removes it lets the row be created.

## Dead ends

- **Last write wins on `(fact, role)`.** An `ON CONFLICT ... DO UPDATE` is one line. It overwrites an observation, which the ledger never does. It makes the value depend on the order of writes, so deleting the newer document leaves the newer value in place unless something recomputes it from what is left, and restoring needs the same. And an `as_of` read could only show today's value for any moment.
- **Qualifiers in the dedup key.** Each value would make its own statement: `covers` with `40-52` and `covers` with `58-70` as two facts. That splits one observation's identity on every change, and it contradicts 0037 decision 3, which keeps a qualifier out of identity so that a second mention adds to the same row.
- **Role words only on the chunk text.** The payload is stored verbatim as the chunk (0054 decision 2), so the value can be parsed back from it. Every reader would then parse JSON or prose to answer a question the table answers, and a document's chunk has no structure to parse.

## Open questions

- **Contested role words.** Two live documents that disagree on one role resolve to the newer. Flagging such a pair the way the export flags a contested statement (0062) would keep the disagreement visible; this record does not decide it.
- **The cost of the re-sync.** Copying evidence and role words onto every live typed row on each `materialize` run is two set-based inserts per base. It has not been measured on a large base.
