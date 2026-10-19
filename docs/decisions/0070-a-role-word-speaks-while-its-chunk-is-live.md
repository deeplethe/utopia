# 0070 · A role word speaks while its chunk is live

- **Status**: Proposed · 2026-10-16 · [#1267](https://github.com/deeplethe/utopia/issues/1267) · open: all five decisions
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

`statement_qualifiers` gains `chunk_id`, a reference to `chunks`, `NOT NULL`, and its key becomes `(fact_id, role, chunk_id)`. Each passage that states a role word records it against itself, the way `fact_evidence` already records one row per chunk. Writes stay append-only and idempotent: the same push twice writes nothing new, a new push writes rows for its new chunk and touches no old row. Every open statement has evidence, and a typed or implied row gets its statements' evidence with its role words (decision 4), so there is no row without a chunk to allow for.

The backfill copies each existing `(fact, role)` value onto every evidence chunk of its fact, superseded ones included. Attaching it only to the earliest chunk would hide role words that show today: for a statement already re-pushed under 0054 decision 7, the earliest chunk is superseded, the live chunk carries no row because `DO NOTHING` dropped it, and under decision 2 the role would disappear. With the value on every chunk, every read after the migration returns exactly what it returned before; only the pushes that follow can change a value.

`chunk_id` is a new provenance edge, and 0048's invariant covers it: the owner-derived trigger `statement_qualifiers_same_kb` checks that the chunk's document is in the fact's base, and the export preflight scans the edge.

**2. A superseded chunk stops speaking for the statement.**

The rows that speak for `(fact, role)` are the rows whose chunk is live at the read's `as_of` (`chunk_live_at`, and `document_live_at` for the chunk's document). For a stale fact, one with no live evidence at all, they are the rows on its evidence chunks that were superseded last, so a fact waiting in review still shows what it last said. This is one SQL fragment beside the `record_axis` helpers, and every reader of role words uses it: the entity panel, the tools, the export and `materialize`.

There is no ordering among live rows. Two live documents that give different values for one role both speak, and a read returns each value with the chunk it came from, the way it returns two evidence passages. This is 0037 decision 3's answer, that two observations that disagree stay two and neither hides the other, applied to role words on open statements. Flagging such a pair as contested is left open below.

What follows:

- A re-push that changes a value supersedes the old chunk, so only the new value speaks. One that drops a role leaves no live row for it, and the role disappears from the read. The old rows stay in the table, as everything in the ledger does.
- Deleting a document supersedes its chunks; restoring it brings them back. The rule reads chunk and document liveness, so neither needs a recomputation.

**3. A read in the past shows what was said then, on the chunk's clock.**

A role word has no record time of its own. It arrives as a row on its evidence chunk, and its record time is that chunk's: it starts speaking when the chunk becomes live and stops when the chunk is superseded. An `as_of` read applies decision 2 at that instant, so a role word that changed shows the value its live evidence carried at `as_of`. In the example above, a read at a moment between the two pushes says `lines: "40-52"`; a read today says `58-70`. This is what `chunk_live_at` already does for evidence, where a passage replaced today is still live evidence on a graph read as of March, and a role word is part of that evidence.

The copies `materialize` makes onto typed and implied rows (decision 4) keep the `chunk_id` of the row they copy, so they take that chunk's liveness too. A copy is written on the first run after its chunk arrives, and an `as_of` read between the chunk's arrival and that run shows it on the typed row a little before it was made. The read answers what the evidence said at that moment, not when the copy was written.

**4. `materialize` keeps typed and implied rows in step.**

On each run, scoped to the base it runs for, `materialize` copies onto every live typed row the evidence and qualifier rows of its source statements (`typed_fact_sources`) that the row does not have yet, and does the same for every live implied row from the statement that triggered it (`implied_fact_sources`). The copy is the same insert it does today at creation, `ON CONFLICT DO NOTHING`, now run for existing rows too. A typed or implied row then reads its role words through the same rule as its statements, and is stale exactly when they are.

The three `mood` checks use the rule instead of any row: a statement has a mood when a row that speaks for `(fact, 'mood')` exists. A re-push that adds `mood` retires the typed row on the next run; one that removes it lets the row be created.

**5. Typed qualifiers keep 0037's rule.**

This record covers role words, the `statement_qualifiers` rows of the open layer and the copies `materialize` makes of them. The typed `fact_qualifiers` of 0037, one value per declared attribute per edge, keep that record's rule: a second mention with a different value does not overwrite the first, and is reported as `qualifier_conflict`.

## Dead ends

- **Last write wins on `(fact, role)`.** An `ON CONFLICT ... DO UPDATE` is one line. It overwrites an observation, which the ledger never does. It makes the value depend on the order of writes, so deleting the newer document leaves the newer value in place unless something recomputes it from what is left, and restoring needs the same. And an `as_of` read could only show today's value for any moment.
- **Qualifiers in the dedup key.** Each value would make its own statement: `covers` with `40-52` and `covers` with `58-70` as two facts. That splits one observation's identity on every change, and it contradicts 0037 decision 3, which keeps a qualifier out of identity so that a second mention adds to the same row.
- **Role words only on the chunk text.** The payload is stored verbatim as the chunk (0054 decision 2), so the value can be parsed back from it. Every reader would then parse JSON or prose to answer a question the table answers, and a document's chunk has no structure to parse.

## Open questions

- **Contested role words.** Two live documents that disagree on one role both speak (decision 2). Flagging such a pair the way the export flags a contested statement (0062) would make the disagreement easier to find; this record does not decide it.
- **The cost of the re-sync.** Copying evidence and role words onto every live typed and implied row on each `materialize` run is a few set-based inserts per base. It has not been measured on a large base.
