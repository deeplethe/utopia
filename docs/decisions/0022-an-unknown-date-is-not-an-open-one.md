# 0022 · An unknown date is not an open one

- **Status**: proposed · to be built in two cuts — the predicate, the anchor and every read of asserted facts first (#345, #352), derived rows second
- **Written**: 2026-09-06 (conventions in the [README](README.md))
- **Related**: [0003](0003-ontology-growth-loop.md)'s graph migration gave the end of a fact three states and refused to store a document's date as an indeterminate instant; this record keeps that refusal and puts the date in a column that says what it is. [0019](0019-the-second-clock-can-be-rewound.md) put the record-axis predicate in one place (`record_axis`) and kept `at` and `as_of` apart; this record does the same for the world axis. [0017](0017-a-contradiction-points-upstream.md) gave derived rows their own precisions, and [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md)'s evaluator intersects premise intervals — both inherit the rule below. From #345 and #352, both found by the temporal benchmark (#306).

> The write side already tells "still holds" from "ended, date unknown", and it never invents a start the text did not give. Every read collapses both back into "holds at every moment". Asked about a moment before any evidence existed, or after a stated ending whose date is missing, the graph answers with confidence — and the row it cites is the one that says it should not.

## What the ledger writes, and what the reads make of it

`facts` records each end of an interval with its own precision (0003):

| the text says | `valid_from` | `valid_to` | `valid_to_precision` |
|---|---|---|---|
| started on a date | date | | |
| nothing about a start | NULL | | |
| still holds | | NULL | NULL |
| ended, date not given | | NULL | `'unknown'` |
| ended on a date | | date | `year` / `month` / `day` |

Every world-axis read tests the interval the same way:

```sql
(f.valid_from IS NULL OR f.valid_from <= $at) AND (f.valid_to IS NULL OR f.valid_to > $at)
```

Two rows of the table lie under it. A NULL start is read as *since always*; a NULL end is read as *still holds* whether or not the precision beside it says the fact is over. Both are the same mistake: an absent date treated as an open bound, when the ledger wrote it to mean an unknown one.

The benchmark asked the two questions that expose it. *Who did Lin Zhao work for in January 2023?* — the graph answers "the Platform Group, as a Staff Engineer", from two facts with no start whose documents are dated 2024 and 2025; the offer letter is dated June 2023 and the answer should be none. *What is Lin Zhao's title in January 2026?* — "Staff Engineer", cited from the row that records she no longer holds it, ending date not given.

The predicate is written by hand at every read site: `graph::edges_among` (the asserted and the derived branch), the `entity_facts` chat tool (facts and derived, in Rust), the RDF current triple in `rdf.rs` — the one reader that does check the end's precision, and still reads a missing start as started — the graph page's slider filter in `Graph.tsx`, which is worse than the SQL (an edge with no start is active at every slider position, whatever its end says), and the entity panel's "current" filter. 0019 named this shape as the risk: a defence spread across read sites fails where one is missed, and neither SQL nor `cargo check` says a word.

Derivations inherit it. `reasoning::timed_edges` maps `valid_to` to an open bound whether the precision says `'unknown'` or not, and `derived_facts` could not store the state anyway — 0013's CHECK ties a precision to a date, so `'unknown'` beside a NULL end is rejected there. A transitive chain through a "former CEO" fact yields a derived edge that holds today. In the benchmark base 38 of 152 live facts have no start and 5 have ended on an unknown date; every derivation that touches one is open on the wrong side.

## Decisions

### 1. One predicate, one place

`crates/utopia-store/src/world_axis.rs`, beside `record_axis`: `facts_hold_at(alias, param)` and `derived_hold_at(alias, param)`. Every server read of the world axis takes them. The Rust-side filters in `tools.rs` stop re-implementing the rule: the reads they filter gain an `at` parameter and apply the same SQL.

The client never re-derives it either. Edges and entity facts gain `holds_from` / `holds_to` — the interval **as read**, projected by the same expression the predicate uses — and the slider and the panel filter on those. The stated interval stays in `valid_from` / `valid_to` for display. A second implementation of the rule in TypeScript would be the next place to forget a change.

`at` absent keeps meaning **every moment** on the world axis: the canvas shows history and the slider narrows it. On the record axis absence means now (0019). The defaults differ for a reason — nobody holds a belief later than now, but a graph without a time is a graph of all time.

### 2. An unknown bound reaches as far as the evidence, and no further

`facts.attested_at TIMESTAMPTZ NOT NULL`: the earliest date among the documents whose observations were merged into the row. The rule:

- the lower bound is `valid_from` when stated, else `attested_at`;
- the upper bound is `valid_to` when stated; `attested_at` when the precision says `'unknown'`; open otherwise.

```sql
COALESCE(f.valid_from, f.attested_at) <= $at
AND CASE WHEN f.valid_to IS NOT NULL            THEN f.valid_to    > $at
         WHEN f.valid_to_precision = 'unknown'  THEN f.attested_at > $at
         ELSE TRUE END
```

The asymmetry between the two ends is kept on purpose. An open end still reads as *holds until told otherwise*: forward continuation is a convention the ledger can afford, because endings arrive as records — a later document, a person's correction — and close the row. A missing start does not read as *since always*, because backward continuation has no corrector; nothing will ever arrive to say "and in 2023 it had not started yet". So a fact holds from the moment there is evidence for it, and before that the honest answer is none: a raise approved in a note dated 2024-02-20 was approved no later than that, and nothing places it in January 2023 (#352). An ending the text states without a date bounds the fact from above at the document that states it (#345): the row says "does not hold by 2025-10-15", and now the reads say the same.

A row with both ends unknown — "no longer holds X", date not recorded, start never given — holds at no moment. It is a closing statement, and reading it as one is right; the panel still lists it under history, marked ended.

Why the document's date and not `recorded_at`: back-filled corpora. The benchmark ingests documents about 2023–2025 in one evening; anchored on `recorded_at`, every undated fact would appear in 2026 and at no historical moment. `recorded_at` is also the other clock — 0019 separated the two and this record does not leak one into the other. `documents.doc_time` is world-axis (published, modified, or given on ingest).

Why a column and not a join over `fact_evidence` at read time: the slider filters a payload, not a table; every read site would carry a correlated subquery; and the time-refinement path in `insert_fact` copies a superseded bare row's evidence onto the dated row, so `min(doc_time)` over evidence would anchor a refined ended-unknown row at the document that said it *held*. The anchor is set when the row is written, moved only earlier, and inherited by every superseding row.

Why not the document's date in `valid_from` / `valid_to` with a marker precision: 0003 refused this for the end — the indeterminate instant — and the reason holds for the start. Every reader of a stated column would have to check the precision before believing the date, and the panel would print "since 2024-02-20" for a fact the text only places *by* then. The anchor lives in a column that says what it is.

### 3. Where the anchor comes from

- **Extraction.** `insert_fact` / `insert_value_fact` take the document's `doc_time`. When an observation merges into an existing row (same stated start), `attested_at = least(attested_at, doc_time)`: an earlier document is earlier evidence. A document with no date anchors at the moment of recording — the best the ledger has.
- **The pending-facts nod** (0015) goes through the same call with its own document.
- **A person's own fact** from the API anchors at now: the person is the evidence and is speaking now.
- **Superseding rows** — `close_superseded`, `correct_interval`, the adoption rewrite — copy the anchor from the row they supersede. A correction restates the same evidence; it does not become newer evidence.
- **Backfill**: `min(doc_time)` over the row's evidence, else `recorded_at`. One migration.

### 4. Derived rows store the interval as read

The evaluator intersects each premise's *read* interval, not its stated one: `overlap()` receives `(valid_from ?? attested_at, valid_to | attested_at-if-unknown | open)`. A derived row's `valid_from` / `valid_to` are therefore what its premises jointly support, and `derived_hold_at` is plain containment — derived rows need no anchor of their own.

A bound that came from an anchor rather than a stated date carries **no precision**. 0013's CHECK on `derived_facts` loosens from "a date iff a precision" to "a precision only with a date", and the UI renders a precision-less date as a date. The proof chain still shows where the bound came from: the premise whose panel row has a blank start. This also retires an accident in `coarsest`, which ranks `'unknown'` as the finest precision.

### 5. What a reader sees

The slider and a timed question stop returning a fact before its evidence or after its stated ending. Two benchmark questions leave `known_gap` and are counted. The stated interval on the panel does not change — blank stays blank — so the only visible difference is which edges the slider shows at a moment and which facts a timed answer cites.

## Phasing

1. Schema (`attested_at`, its backfill, the derived CHECK), every writer, `world_axis`, every server read, `holds_from` / `holds_to` on edges and entity facts, the two client filters, the benchmark's two questions un-gapped. One database-backed test carries the table of cases in both directions: a no-start fact absent the day before its document and present on it; an ended-unknown fact present the day before its document and absent from it; a stated interval untouched; the both-unknown row absent at every moment yet listed on the entity.
2. Derived rows: the evaluator on read intervals, precision-less anchored bounds, and a test that a chain through an ended-unknown premise ends at that premise's document.

## Dead ends

- **Anchor on `recorded_at`.** No column, no migration — and wrong for every back-filled base, see above. It also reads the record clock into a world-axis answer.
- **Compute the anchor from evidence at read time.** Correct until the refinement path copies evidence, and the client cannot do it at all.
- **Exclude every unknown-bounded fact from timed reads.** A quarter of the benchmark's facts have no start; the slider would show a near-empty graph at every position and hide facts there is evidence for at that very moment.
- **Three-valued reads** — holds / does not / unknown — surfaced to consumers. Honest in a different way, and every consumer (canvas, tool, export, rules) would have to carry and render the third value. This record collapses "unknown" into "not known to hold", which is what a graph that stays silent means. Revisit if someone needs to tell "no" from "don't know" at a moment.
- **A marker precision on the stated columns.** 0003's objection, again: a definite-looking date every reader has to distrust.

## Open questions

- **One write path drops an ending.** `insert_fact_inner` reuses a live row with the same stated start as an exact duplicate before it looks at the end; a bare open row (no start, no end) and a new "ended, date unknown" observation with no start are such a pair, so the ending is lost. The `ended_when_unknown` test covers the weak-statement path, not this one. Separate issue.
- **An ending does not close the dated row it ends.** `reconcile_new_fact` returns early for any fact that has ended, so the "no longer holds" row from #352 sits beside the 2023-06-01 row that still says "holds", and that row still reads as holding today. Whether an ended-unknown observation should close the open row of the same assertion — its precision to `'unknown'`, its anchor to the ending's document — is a write-side decision that would make the both-unknown row rare. Separate issue.
- **Retrieval is untimed.** Vector and full-text recall take `as_of` (0019) but no `at`; a chunk about 2025 answers a question about 2023 and the model is left to notice. Out of scope; noted so the benchmark's chat probe is read with that in mind.
- **Showing the anchor.** Whether the panel should say "attested 2024-02-20" beside a blank start, so a person sees why the slider hides the edge before then. Deferred until the first cut has been looked at.
