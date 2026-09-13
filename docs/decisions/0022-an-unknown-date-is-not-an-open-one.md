# 0022 · An unknown date is not an open one

- **Status**: implemented in two cuts · #394: `world_axis` beside `record_axis`, `facts.attested_at` set by every writer and backfilled, every server read and both client filters on the read interval, `holds_from` / `holds_to` on edges and entity facts, and an undated ending **closes** the dated open row it ends (`close_with_unknown_end`) · second cut: the evaluator intersects premise intervals as read (`read_span`), a derived bound that came from an anchor carries no precision (migration 0031 loosens the derived CHECK), and the violations list judges "still open" by the read end · third cut (#393): a second anchor, `attested_to`, so a bare open row closes too · fourth cut (2026-09-11): a **dated** ending closes the open row the same way, and every superseding row carries the edge's qualifiers · fifth cut (2026-09-14, #679): the temporal engine places a value in its timeline by the same anchors the reads use — a row with no stated start is ordered by its earliest dated evidence, and a predecessor it supersedes closes as ended-unknown anchored there; a deadline stated relative to an event is stored as written, flagged `relative`, and closes the dated one before it the same way
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

Two anchors, one per end (the first cut had one, `attested_at`, and #393 showed why that is one too few — see below). `facts.attested_from TIMESTAMPTZ NOT NULL`: the earliest date among the documents whose observations were merged into the row. `facts.attested_to TIMESTAMPTZ`: the date of the document that said the fact was over, present exactly when the end is `'unknown'` (a CHECK ties the two). The rule:

- the lower bound is `valid_from` when stated, else `attested_from`;
- the upper bound is `valid_to` when stated; `attested_to` when the precision says `'unknown'`; open otherwise.

```sql
COALESCE(f.valid_from, f.attested_from) <= $at
AND CASE WHEN f.valid_to IS NOT NULL            THEN f.valid_to    > $at
         WHEN f.valid_to_precision = 'unknown'  THEN f.attested_to > $at
         ELSE TRUE END
```

The asymmetry between the two ends is kept on purpose. An open end still reads as *holds until told otherwise*: forward continuation is a convention the ledger can afford, because endings arrive as records — a later document, a person's correction — and close the row. A missing start does not read as *since always*, because backward continuation has no corrector; nothing will ever arrive to say "and in 2023 it had not started yet". So a fact holds from the moment there is evidence for it, and before that the honest answer is none: a raise approved in a note dated 2024-02-20 was approved no later than that, and nothing places it in January 2023 (#352). An ending the text states without a date bounds the fact from above at the document that states it (#345): the row says "does not hold by 2025-10-15", and now the reads say the same.

A row with no stated start that an undated ending closes holds from its first evidence to the ending's document — `[attested_from, attested_to)`; that is what the second anchor buys. A fresh row that only says "no longer holds X", date not recorded and start never given, has the same document at both ends and holds at no moment: it is a closing statement, and reading it as one is right; the panel still lists it under history, marked ended.

Why the document's date and not `recorded_at`: back-filled corpora. The benchmark ingests documents about 2023–2025 in one evening; anchored on `recorded_at`, every undated fact would appear in 2026 and at no historical moment. `recorded_at` is also the other clock — 0019 separated the two and this record does not leak one into the other. `documents.doc_time` is world-axis (published, modified, or given on ingest).

Why a column and not a join over `fact_evidence` at read time: the slider filters a payload, not a table; every read site would carry a correlated subquery; and the time-refinement path in `insert_fact` copies a superseded bare row's evidence onto the dated row, so `min(doc_time)` over evidence would anchor a refined ended-unknown row at the document that said it *held*. The anchor is set when the row is written, moved only earlier, and inherited by every superseding row.

Why not the document's date in `valid_from` / `valid_to` with a marker precision: 0003 refused this for the end — the indeterminate instant — and the reason holds for the start. Every reader of a stated column would have to check the precision before believing the date, and the panel would print "since 2024-02-20" for a fact the text only places *by* then. The anchor lives in a column that says what it is.

> **Revised 2026-09-14 (#679, #681 §1): the engine orders by the same anchor the reads use.** This decision gave the *reads* a rule for a row with no stated start — it holds from its first evidence — and left the temporal engine on stated starts alone. The engine closed a start-less open row at whatever new start arrived. That gave two wrong timelines on the Blackbaud HQ lease chain. The 11th amendment (2020-08-13) says the landlord is BBHQ1 without saying since when; when the 2016 lease's HPBB1 arrived later, BBHQ1 was closed at 2016, a landlord before the lease existed. And a start-less deadline kept overlapping the dated ones around it.
>
> The engine now orders a start-less row by its earliest evidence date, and closes the row before it the way this record already writes an undated ending. The predecessor gets `valid_to` NULL, precision `'unknown'`, and `attested_to` set to that date, so the reads return it until that date and the start-less row from it: they abut and do not overlap. A document date is still never written into `valid_from` or `valid_to`. When a stated start for the row arrives later, the ordinary placement takes over.
>
> The ordering date is `min(doc_time)` over the row's evidence, counting only documents whose own date is known (`doc_time_source` of `content` or `source`). It is not `attested_from`, for the reason section 3 gives in reverse. An undated document anchors `attested_from` at the moment of recording, and ordering by that would read every start-less row as holding now, turning an ordinary "X, then Y from 2021" into a conflict. A row with no dated evidence has no place in the timeline and goes to a person, as before. The concern above about copied evidence does not bite here: a superseding row's evidence is the same documents, and the earliest of them is still the earliest date the row is known to hold.
>
> A value the text gives only relative to an event, such as a deadline of "45 days after the Trigger Date", has no calendar date. The extractor marks it `relative` (the model decides; the server matches no wording), and the server stores it as written. It is still the current value, so it closes the dated value before it in the same way, anchored at its own document. It is never compared or sorted as a date. An unflagged value that is not a date is still dropped as `attr_datatype`.

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

1. Schema (`attested_at` and its backfill), every writer, `world_axis`, every server read, `holds_from` / `holds_to` on edges and entity facts, the two client filters, the benchmark's two questions un-gapped. One database-backed test carries the table of cases in both directions: a no-start fact absent the day before its document and present on it; an ended-unknown fact present the day before its document and absent from it; a stated interval untouched; the both-unknown row absent at every moment yet listed on the entity.
2. Derived rows: the evaluator on read intervals, precision-less anchored bounds (which is where the derived CHECK loosens — it ships with its first writer, not ahead of it), and a test that a chain through an ended-unknown premise ends at that premise's document.

## Dead ends

- **Anchor on `recorded_at`.** No column, no migration — and wrong for every back-filled base, see above. It also reads the record clock into a world-axis answer.
- **Compute the anchor from evidence at read time.** Correct until the refinement path copies evidence, and the client cannot do it at all.
- **Exclude every unknown-bounded fact from timed reads.** A quarter of the benchmark's facts have no start; the slider would show a near-empty graph at every position and hide facts there is evidence for at that very moment.
- **Three-valued reads** — holds / does not / unknown — surfaced to consumers. Honest in a different way, and every consumer (canvas, tool, export, rules) would have to carry and render the third value. This record collapses "unknown" into "not known to hold", which is what a graph that stays silent means. Revisit if someone needs to tell "no" from "don't know" at a moment.
- **A marker precision on the stated columns.** 0003's objection, again: a definite-looking date every reader has to distrust.

## Open questions

- **An ending does not close the dated row it ends.** ~~`reconcile_new_fact` returns early for any fact that has ended, so the "no longer holds" row from #352 sits beside the 2023-06-01 row that still says "holds".~~ Settled in the first cut, because the benchmark showed the read rule alone leaves #345's question failing: the ended-unknown row read correctly, and the dated open row beside it went on holding. An undated ending that meets an open row **with a stated start** of the same assertion now closes it — a superseding row with `'unknown'` at the end, anchored on the ending's document, the old row invalidated (`temporal::close_with_unknown_end`). A repeated ending reuses the closed row and only moves its anchor earlier.
- **A bare open row could not be closed the same way** (#393). ~~With no stated start the one anchor would have to serve both ends — held from the first note, ended by the second. The exact-duplicate path in `insert_fact_inner` merged the ending into the bare row and dropped it.~~ Settled with a second anchor: `attested_at` became `attested_from`, and `attested_to` carries the date of the document that stated the ending, present exactly when the end is `'unknown'`. `close_with_unknown_end` now closes any open row of the assertion — stated start or not — keeping `attested_from` and setting `attested_to`; a repeated ending only moves either anchor earlier. Every superseding writer copies both; a person's own edit that marks a fact ended-unknown anchors the end at now.
- **A dated ending did not close the open row either** (2026-09-11). ~~"李文博于 2024-04-30 辞去董事职务" arrived as a state observation with no start and a stated end; `insert_fact_inner` had a path for an *undated* ending (above) and none for a dated one, so the ledger held `2020-01-10 → open` beside `open → 2024-04-30` and read the first as still holding. The same for a company taken off a court's list of defaulters.~~ Settled: a state observation with no start and a stated end meets the open row of the same assertion whose start is not later than that end and closes it through `close_superseded` — the same superseding path, the end dated and truncated to its precision; a repeated dated ending reuses the closed row and only moves its anchor earlier. The same cut made every superseding writer (`close_superseded`, `close_with_unknown_end`, `correct_interval`, the refinement in `insert_fact_inner`) copy the edge's qualifiers along with its evidence — before it, closing a directorship dropped its title ([0037](0037-a-relation-carries-its-own-attributes.md)).
- **Retrieval is untimed.** Vector and full-text recall take `as_of` (0019) but no `at`; a chunk about 2025 answers a question about 2023 and the model is left to notice. Out of scope; noted so the benchmark's chat probe is read with that in mind.
- **Showing the anchor.** Whether the panel should say "attested 2024-02-20" beside a blank start, so a person sees why the slider hides the edge before then. Deferred until the first cut has been looked at.
