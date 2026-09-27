# 0024 · The world axis reaches the second

- **Status**: Implemented (migration 0033)
- **Written**: 2026-09-06 (conventions in the [README](README.md))
- **Related**: [0003](0003-ontology-growth-loop.md)'s graph migration gave each end of a fact its own precision and stopped the ladder at the day; [0019](0019-the-second-clock-can-be-rewound.md) keeps the record axis at the clock's own resolution; [0022](0022-an-unknown-date-is-not-an-open-one.md) made an anchor an instant. #351 and #414 made every printed time carry its own precision — and showed that the world axis had nowhere finer than a day to carry.

> A memory-log line is stamped to the minute, a ticket to the second, a memo says "the handover at three". The ledger writes all of them as the day: whatever a source states below the day is cut off at extraction, refused at the API, and never shown. The record axis, meanwhile, prints microseconds. Two axes, two ladders. One ladder, each axis stopping where its knowledge stops.

## What the ledger does today

`facts.valid_from` / `valid_to` are `timestamptz`; `valid_from_precision` / `valid_to_precision` say how much of the value is meant — `year`, `month`, `day`, plus `unknown` on the end (0003). The value is stored truncated to that precision: a year is 1 January 00:00, a day is midnight. `parse_time` in the extractor accepts `YYYY`, `YYYY-MM`, `YYYY-MM-DD` and nothing with a clock time; the API validation lists (`graph_routes`, `review_routes`) accept the same three; the renderers (`time_text`, `fmtTime`, `rdf::world_time`) know the same three.

So a source that knows more is made to know less. The memory log is the sharpest case: the sentence is stamped `[2026-06-01 14:32]` by us, and the fact extracted from it holds "from 2026-06-01". Two handovers on one afternoon become simultaneous, and the temporal engine files them as a conflict a person has to settle (`reconcile_new_fact`, the `simultaneous` branch) — a conflict the source never had.

## Decisions

### 1. The ladder runs year, month, day, hour, minute, second

`unknown` stays as the marker on the end. Nothing below the second: no person and no document states a sub-second time, and the question the world axis answers — when did this hold — is settled at the second for anything a source will ever say. The record axis keeps microseconds because it is our clock, not a statement. Both axes now use the same scale — the ISO 8601 truncations — and each stops at the rung it has evidence for. That is what "one unit" means here; making a memo pretend to microsecond knowledge would be the opposite.

### 2. A stored value is truncated to its precision

`date_trunc(precision, value) = value`, as a CHECK on `facts` and `pending_facts`, and on `derived_facts` wherever a precision is present (an anchored bound has none, 0022). The precision names are `date_trunc`'s own field names, so the constraint is one expression. Today's rows already satisfy it — `parse_time` and `parseDateInput` produce truncated values — and the migration normalises any that do not, once; that changes representation, not meaning, since the precision already declared those digits noise. With the invariant, storage, display and export cannot disagree: what is printed is what is stored is what was known.

### 3. A clock time without a zone is a date

A calendar date has no time zone; storing it at UTC midnight is a convention, not a claim. A clock time is different: "14:32" is not an instant until you know whose 14:32. So hour, minute and second are reached only by an instant with an offset — `Z`, `+08:00` — or by a timestamp a machine wrote (the memory log's own stamps, a ticket's `updated_at`, a feed's `pubDate`). A time the text states without a zone is recorded at day precision; the time itself stays in the quote. The extractor is told this in one sentence and told not to guess a zone; the interval editor and the Review inputs reject a zone-less time instead of assuming one.

### 4. The ladder is spelled in four places, from one list

`utopia_store::graph::WORLD_PRECISIONS` is the list; `truncate_to` is the invariant in Rust for the write paths that take a value and a precision (`insert_fact`, `correct_interval`, `close_superseded`, `close_with_unknown_end`). The four spellings: `parse_time` (extractor and `parse_when`), `time_text::world` with `fmtTime` beside it, `rdf::world_time` (sub-day values go out as `xsd:dateTime` with an explicit `utopia:*Precision` literal, since the XSD type alone cannot say "to the minute"), and `coarsest` in the evaluator, whose rank order extends. Sub-day values print in the reduced ISO forms — `2026-06-01T14Z`, `2026-06-01T14:32Z`, `2026-06-01T14:32:07Z` — the same string parsing back to the same value and precision.

## What a reader sees

A fact from the memory log holds from `2026-06-01T14:32Z` instead of from the day; two handovers an hour apart are two intervals, not a conflict. The graph slider, `entity_facts`, the export and the entity panel all show the minute. A fact from a memo that says "at three" still holds from the day, and the panel's quote still says "at three".

## Dead ends

- **Microseconds on the world axis, to match the record axis literally.** Same column type, so it costs nothing to store — and it would let a memo carry a precision it never had. The record axis's resolution is a property of our clock; putting it on the world axis makes a claim.
- **A zone-less time assumed to be UTC.** Silent, and wrong by whatever the author's offset was; the error is invisible in the row and only surfaces when two sources disagree by exactly eight hours.
- **Store the finer value, label the coarser precision.** "Keep the digits, they might be useful." Every reader would then have to decide whether to trust the label or the value, and display and export would each decide differently. The invariant settles it.

## Open questions

- **A document's own time zone.** A `documents.time_zone` — set from a source's metadata, or by a person for a folder — would let "at three" become an instant. Deferred until a corpus needs it; the rule above is safe without it.
- **Machine event logs as facts.** If log lines are ever extracted into facts with millisecond stamps, the second is where they land. Revisit only if a question needs the millisecond.
- **The duration of a sub-day fact's precision on the canvas.** The slider's histogram buckets by year, month or day; a minute-level fact is drawn at its day. Fine for now, and a UI question, not a ledger one.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> implemented in one cut · migration 0033 widens the ladder to the second and adds the truncation CHECK on `facts`, `pending_facts` and `derived_facts`; `WORLD_PRECISIONS` and `truncate_to` in the store, every writer truncating; `parse_time` reads zoned clock times and folds a zone-less one to the day; `time_text`, `fmtTime` and `rdf::world_time` write the reduced ISO forms; a derived bound takes the precision of the premise that set it
