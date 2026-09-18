# Time has two clocks and every read says which

Records: [0019] (the record axis), [0022] (unknown bounds, anchors, the temporal engine), [0024]
(precision), [0031] (events and eternal facts), [0045] (time mentions, proposed), with [0013] d6,
[0002] d8, [0021] d4, [0027] revised, [0043] d5, [0044] d5. The columns live in [ledger](ledger.md).

## What it does today

**Two clocks, never one control.** `valid_from` / `valid_to` say when a fact held in the world;
`recorded_at` / `invalidated_at` say when the base held it. Every graph read takes `at` (world) and
`as_of` (record) separately; `at` absent means every moment, `as_of` absent means now [0019, 0022].
`record_axis::held_at(T)` and `world_axis::facts_hold_at` / `derived_hold_at` are the only places
the two predicates are spelled; write paths keep the hard-coded current-row filter because a
correction is never made as of March [0019, 0022]. Entities rewind through `entity_merges`,
derivations through `derived_at`, retrieval through superseded chunks that keep their vectors;
full-text search is "now" only [0019].

**Precision.** The world ladder runs year, month, day, hour, minute, second, plus `unknown` on an
end; a stored value is truncated to its precision by CHECK on `facts`, `pending_facts` and
`derived_facts`; one list (`WORLD_PRECISIONS`) feeds the parser, the renderers, the export and the
evaluator; a clock time without a zone is a date; world time renders in UTC, record time in the
viewer's zone [0024, 0013]. GitHub and Jira timestamps are written to the second [0013].

**An unknown bound reaches as far as the evidence.** A missing start is not "since always": the lower
bound is `valid_from` or else `attested_from`, the earliest document date among the row's evidence.
An ending with no date is not "still holds": `valid_to_precision = 'unknown'` with `attested_to`,
the date of the document that said it was over. An open end still reads as holds until told
otherwise [0022]. Anchors are copied by every superseding writer and moved only earlier.

**The temporal engine** (`temporal.rs`) works only on `state` relations declared functional or
inverse-functional. A new value closes the open one before it; an undated or dated ending closes
the open row it ends through a superseding row; a start-less row is ordered by its earliest dated
evidence (`doc_time_source` content or source only) and closes its predecessor as ended-unknown
there; ends the engine drew are marked `end_derived` and recomputed in one pass whenever a timeline
changes, so the result depends on which rows exist and not on arrival order; a relation unique on
both sides sits on two timelines under one lock; a deadline stated relative to an event is stored as
written and flagged `relative` [0022 revised, #679]. **A successor takes over on how its start was
got, not on a number** [0045 cut 3]: `valid_from_grade` carries the resolution grade from the time
mention to the statement and on through materialisation to the typed row, and a successor whose
start could not be anchored (grade C) closes nothing and opens no conflict, because its place on the
axis comes from its document's date rather than from the sentence. A successor read off a picture
closes nothing either and opens a `described_evidence` conflict, which is what a misread chart is
for [0040 d4]; that guarantee used to ride on the confidence ceiling and now stands on the chunk's
origin. `AUTO_CLOSE_MIN_CONFIDENCE` is gone and the engine reads no confidence at all; the number stays on the
row for review queues and display. Confirming a fact changes the value and recomputes its
timelines [0040, 0043 d5]. Two values that still hold at one moment are
a conflict; a succession is not, for the consistency check and for the merge gate alike [0017,
0027 revised].

**Events and eternal facts.** `Validity::under` normalises every write by the predicate's
`temporal`: an event is one moment written at both ends (a span collapses to its start, an unknown
ending is dropped); an eternal fact has no dates; a state keeps its interval. Reads give an event
the bucket its precision names, an undated event no moment, an eternal fact every moment; a fact
with no predicate reads as a state [0031]. Rows written before the rule are read correctly and not
rewritten. The `[event]` / `[eternal]` prompt marks of 0031 d4 and the zoned-time sentence of 0024
belonged to the typed contract and are gone with it; the write rule in the store is what remains.

**Derived rows** store the interval as read: the intersection of premise read intervals, an anchored
bound with no precision, precision otherwise from the premise that set it [0022, 0024, 0002].

**Open statements carry no dates.** An open row has no `valid_*`; its time words are
`time_mentions` (verbatim text, chunk, offset) and its `attested_at` comes from the document's date
only when that date came from the content or a dating source, else null [#731]. Interpretation is
0045's and not built, so the slider shows nothing for a freshly extracted base.

## Why

- **"What did we believe in March" was unanswerable** through any shipped query while the rows were
  on disk; a hard-coded `invalidated_at IS NULL` sat at 54 read sites [0019].
- **Folding the two clocks into one slider** answers a different question than the one asked, and
  both look plausible on screen [0019].
- **A defence spread across read sites fails where one is missed**, and neither SQL nor `cargo check`
  says a word, so each predicate lives in one place with a database test in both directions
  [0019, 0022].
- **Backward continuation has no corrector**; forward continuation does (a later document, a
  person). A raise approved in a 2024 note is not placed in 2023, and a "no longer" dated 2025
  bounds the fact there [0022].
- **The document's date, not `recorded_at`**, because a back-filled corpus would put every undated
  fact in the year of ingest; and the anchor is a column because the refinement path copies evidence
  [0022].
- **The engine orders by the same anchor the reads use**, or a landlord is closed before the lease
  existed; an end the engine drew must move with the value that follows [0022, #679].
- **The ladder stops at the second** because no source states less, and a value is truncated so
  storage, display and export cannot disagree [0024].
- **An event written at both ends fails safe** in a reader that does not consult the predicate; a
  NULL end reads as "still holds" [0031].
- **Confidence is part of the timeline**: confirming a doubtful successor decides whether it takes
  over [0043].

## Proposed and not built

- **A time mention is resolved against its document** [0045]: the model returns shape (point,
  interval, as-of, duration, ending without a date), a reference (absolute, or an anchor plus offset,
  or none) and granularity; code computes the interval; a document carries its own date, calendars
  and narrative anchors from chunk to chunk; an unanchored mention waits and is recomputed when an
  anchor arrives; `attested_at` is null for an undated document (revising 0022 d3, which anchored it
  at the moment of recording; #731 already writes null on open rows); no time words in code.
  Cuts: interpretation columns; context and code resolution (closes #714); grades **(built)**;
  re-resolution and a time-anchor review queue.
- Thresholds before cut 2: 95% on absolute and 85% on anchored mentions across corpora; the lease
  bench no lower than 17 of 18 on both runs [0045].
- The `as_of` control on the graph page; the anchor shown beside a blank start; an event drawn as a
  point; retrieval that takes `at` [0019, 0022, 0031].

## Open questions

- Record-axis reads of evidence with no attestation: every moment or none [0045].
- A document with two calendars; a period-valued attribute as one statement with an interval or a
  value with a period mention [0045]; zone-less clock times and a document's own time zone [0024].
- The end convention for states: "until 2024-07" ends at the start of July today [0031].
- The histogram bucket for a sub-day fact on the canvas [0024].
