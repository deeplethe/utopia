# 0045 · A time mention is resolved against its document

- **Status**: Accepted 2026-09-17 · cuts 1 and 2 built (#740): a document is dated from its own text, each time mention is interpreted by the model and computed by code, upload time is used nowhere · cut 3 (grades replace the confidence gate in the temporal engine) and cut 4 (re-resolution when a person sets a document's date, the time-anchor review queue) not built · current state in [design/time](../design/time.md) · expands decision 5 of [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) and revises decision 3 of [0022](0022-an-unknown-date-is-not-an-open-one.md) for undated documents · #714
- **Written**: 2026-09-16 (conventions in the [README](README.md))
- **Related**: [0019](0019-the-second-clock-can-be-rewound.md) and [0022](0022-an-unknown-date-is-not-an-open-one.md) gave facts a world axis, a record axis and an attestation; [0024](0024-the-world-axis-reaches-the-second.md) fixed the precision ladder; [0031](0031-an-event-holds-at-the-moment-it-names.md) made a predicate's temporal kind normalise what is written; #679 made a timeline independent of arrival order; #680 read the opening of a document into every chunk; #688 accepted spelled-out dates; #714 found upload time used as the document date; [0041](0041-a-name-is-a-claim-about-an-entity.md) is the pattern this record copies for time.

> A press release opens with "Today, the U.S. Food and Drug Administration approved…". The document was uploaded on 2026-09-14, so the prompt tells the model the document is dated 2026-09-14 and asks it to resolve "today" against that. The approval is recorded on the day of the upload. A statistics bulletin writes "比上年末增长" in every paragraph; each chunk resolves it alone, and three chunks give three different years.

## What extraction does with time today

The engine side is sound and stays: two axes, `valid_from` and `valid_to` with a precision ladder from year to second, `attested_at`, an ending with an unknown date, timelines of functional predicates recomputed as a whole, corrections as superseding rows. What this record replaces is how a date gets from the text into those columns.

1. **The model computes the date.** The contract asks for `valid_from` and `valid_to` as `YYYY`, `YYYY-MM`, `YYYY-MM-DD` or a zoned clock time, and `valid_to: "unknown"` for an ending without a date. Rule 4 of the prompt says "Document date: {t}. Resolve relative time expressions (e.g. "last year", "this March") to absolute dates using it as the reference." The arithmetic happens inside the model and leaves no trace.
2. **The reference is the upload time when the document has no date.** `documents.doc_time` falls back to `now()` with `doc_time_source = 'upload_time'`; extraction passes it as the document date, `validity_of` writes it into `attested_at`, and name facts take it too (#714). Every relative expression in an undated document resolves against the day it was uploaded.
3. **Each chunk resolves alone.** A chunk sees the document's opening (#680, 1,500 characters) and the document date, nothing else. An anchor set two paragraphs earlier ("in 2019 … two years later"), a fiscal calendar defined in the opening ("fiscal 2027 ended January 25, 2026"), a reporting period a bulletin uses throughout, are not available where they are needed.
4. **Time is not a thing.** A fact carries two strings; the same expression is resolved again for every fact that mentions it, not always the same way; nothing points back at the words; nothing is re-resolved when an anchor turns up later.
5. **Precision comes from the shape of the string.** `read_time` and `written_date` (#688) infer year, month or day from how many parts the model wrote. Whether "2024" meant a moment, the whole year, or "as of" is lost, except for what the predicate's temporal kind (0031) implies.
6. **The gates run on self-reported confidence.** Rule 6 asks the model for 0.9, 0.7 or 0.5; `AUTO_CLOSE_MIN_CONFIDENCE` (0.75) then decides whether a later fact may close an earlier one. The number is not calibrated to anything.

Measured: on the lease bench (18 as-of questions) single runs of one configuration differ by three answers; the 2026-09-14 attribution found the engine ordering correctly and the losses in extraction. In the 2026-09-15 prototype, statements carrying any time were 12% on the FDA corpus, 48% on the statistics bulletins and 16% on the SEC filings; statements with an ending, 1 in about 1,900; the FDA approval dates were lost because the text says "Today" and the anchor was the upload day.

## Decisions

### 1. A time mention is a fact of the open graph

Extraction reports every time expression as a mention: the words as written, the chunk and offset. A statement's `when`, `ended` and `as_of` refer to mentions; one mention may date several statements, and a period heading a table column dates every value in the column. Code checks that the words occur in the chunk, as it does for names (0041).

### 2. The model reads; code computes

For each mention the model returns an interpretation, never a computed date:

- **shape**: a point, an interval, an as-of, a duration, or an ending whose date is not given;
- **reference**: the value as written when it is absolute; otherwise an anchor (another mention, the document's date, or a period the document names) with an offset (count, unit, direction); or none when the text gives nothing to anchor to;
- **granularity**: the rung of the ladder the words state (year, month, day, hour, minute, second), separate from the reference.

Calendar arithmetic, fiscal periods to intervals, truncation to granularity, and the interval a shape implies are computed in code from those fields. An interpretation with a wrong anchor or offset is visible and correctable; a wrong date written by the model was not.

### 3. A document carries its time context from chunk to chunk

The context holds the document's own date, the periods and calendars the document defines, and the anchors its narrative sets. It is built as extraction proceeds: the opening seeds it, every chunk reads it and may extend it, and it is stored with the document so a later re-resolution can use it.

The document's date comes only from the document's content or from a source that dates it (a filing date, a publication date, a feed's publish time). The time of upload, synchronisation or extraction is recorded time and never enters the context. `doc_time_source = 'upload_time'` is read as **no date**.

### 4. What cannot be resolved waits

A mention whose anchor is unknown keeps its words and interpretation and dates nothing; the statements it should date carry no valid time, and their attestation says only that the document observed them. When an anchor arrives — a later chunk, another document that dates the same event, a person setting the document's date in review — resolution recomputes the dependent intervals, as new evidence re-evaluates an identity (0041). A guessed date is never written.

### 5. Three times stay in three columns

- **valid** time: what resolution computed for the statement;
- **attested** time: the document's own date, null when the document has none — this revises 0022 decision 3, which anchored an undated document at the moment of recording; a read that finds no attestation treats the evidence as undated rather than as observed now;
- **recorded** time: when the ledger learned it.

### 6. Timelines close on resolution grade, not on confidence

Each resolved time carries a grade: **A**, an absolute date the text writes; **B**, a date computed from an anchor the document itself states; **C**, unresolved. A later value may close an earlier one on grade A or B, and grade C neither closes a timeline nor opens a conflict. `AUTO_CLOSE_MIN_CONFIDENCE` and the model's self-reported confidence leave the temporal engine.

### 7. A predicate's temporal kind and a mention's shape are both kept

0031 stays: `Validity::under` normalises a write by the predicate's kind (state, event, eternal). The mention's shape is recorded beside it, so an event dated "during 2019" and a state holding "as of March 2019" remain distinguishable after normalisation.

### 8. No time words in code

The interpretation is the model's; code does arithmetic on structured fields and checks the words against the chunk. No list of relative expressions, month names or period words is matched in code, in any language.

## Not doing

- **Dates computed by the model**, in any field of the contract.
- **Upload time as a document date or an attestation**, for any writer.
- **Regular expressions over free text** to find or normalise time expressions.
- **Per-fact date strings without a mention.** `read_time` keeps reading the absolute value field; the precision it infers must agree with the stated granularity or the mention goes to review.

## Measurement

Two runs per configuration; corpora across domains, as [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) requires.

| What | Bench | Numbers |
|---|---|---|
| Mention normalisation | `scripts/bench/temporal.mjs` corpus, the statistics bulletins ("上年末", "全年", "比上年"), the SEC filings (fiscal quarters, "three months ended"), the FDA releases ("today"), the lease chain (#681) | value and granularity both right; share of mentions at each grade; unanchored rate |
| Statement valid time | the same corpora with reference intervals | interval right within its granularity |
| As-of answers | `temporal.mjs` (world and record axes), the lease bench (18 questions) | answers right; variance over two runs |
| Endings | the lease chain and the SEC filings | endings recorded per statement that states one |

Thresholds before cut 2 lands: mention normalisation at or above 95% on absolute dates and 85% on anchored ones across the corpora; the lease bench no lower than the best measured (17 of 18) on both runs.

## Cuts

1. Time mentions and interpretations in the open-graph contract and the store, with 0044 cut 1.
2. The document time context and code resolution; `attested_at` null for undated documents; closes #714.
3. Grades replace the confidence gate in the temporal engine.
4. Re-resolution when an anchor arrives; the time-anchor review queue of the governed-review design.

## Open questions

- Reads on the record axis for evidence that has no attestation: whether an undated observation counts at every moment or at none.
- A document that defines more than one calendar (a company's fiscal year and a subsidiary's).
- Whether a period-valued attribute (a figure for a quarter) is one statement with an interval or a value with a period mention; 0031's event bucket suggests the former.
- Time zones for events stated with a clock time and no zone (0024 folds them to the day).
