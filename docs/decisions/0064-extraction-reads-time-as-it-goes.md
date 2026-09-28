# 0064 · Extraction reads time as it goes

- **Status**: Accepted 2026-09-28 · nothing built · the time context is carried from chunk to chunk during extraction, as [0045](0045-a-time-mention-is-resolved-against-its-document.md) decision 3 wrote it and cut 2 did not build it; a statement without time words may take an as-of from the context; there is no single document date stamped on facts; a fact with no time holds at every moment
- **Written**: 2026-09-28 (conventions in the [README](README.md))
- **Related**: [#987](https://github.com/deeplethe/utopia/issues/987) (every relation of an entity reads "undated"); [0045](0045-a-time-mention-is-resolved-against-its-document.md) decisions 1–5, which this record keeps and whose cut 2 it replaces; [0022](0022-an-unknown-date-is-not-an-open-one.md) decision 3 and the lower bound of a start-less row; [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) cut 1 (open statements); [#714](https://github.com/deeplethe/utopia/issues/714); draft [#988](https://github.com/deeplethe/utopia/pull/988), which this supersedes

## Problem

0045 decided that a document carries its time context from chunk to chunk: the opening seeds it, every chunk reads it and may extend it. What was built is different. Extraction runs over the chunks first, with no time context at all; afterwards one call reads the first 3,000 characters and picks **one** date as the document's own; a third step interprets the time words extraction happened to record. Measured on dev on 2026-09-28 with documents written for the purpose:

| What | Result |
|---|---|
| A report whose opening states three dates (submitted, data as of, decided by) | the same one picked three runs of three |
| A policy with drafted, revised and effective dates | revised once, effective twice: the facts of one document move six weeks between runs |
| Minutes with the meeting date and the date they were sent | the sending date, three of three; what was said in the meeting is dated two days late, or a month when minutes are late |
| Seven time expressions in one weekly report | two dated. "去年" and "上市半年后" resolved; "三个月后", "上周", "明年第一季度" were not recorded as time words at all and sit inside the statement ("码表二代 — 发布 → 将在三个月后"); "2024年3月" was recorded in one of two runs |
| The same document extracted twice | 11 statements and 4 time words, then 7 and 1 |
| `attested_from` of every statement | the moment the document was processed |

Three things are wrong and they are one thing.

**The words never reach resolution.** A time expression the extractor leaves inside the phrase or the object is invisible to everything after it. The extractor is asked for `when` and `ended` per statement with nothing to tell it what "three months from now" is measured from, so it treats the words as content.

**One date is asked to mean everything.** "The document's date" has no answer for a policy with three dates, and the wrong answer for minutes. Whichever is picked is then the anchor for every relative expression and the attestation of every statement, and the others are dropped with their names.

**Upload time still enters world time.** 0045 decision 5 says attestation is null when the document has no date. `INSERT … COALESCE($7, now())` writes the processing moment, and reads take the lower bound of a start-less row from it. Draft #988 moved that column to the single document date; that is the design this record drops.

## Decisions

**1. The time context is built while extracting, chunk by chunk.** Chunks of a document are already extracted in order, each seeing the named things recorded before it. The time context travels the same way. Each extraction call receives it and returns, beside statements and time mentions, what the chunk adds to it. It holds four kinds of entry, each with the words as written and the chunk they came from:

- **reference points**: the moment the text speaks from ("提报日期 2026年9月4日", a dateline, "会议时间"), with the span it governs: the document, or the section under a heading;
- **named dates**: every date the text gives a name to ("数据统计截止", "生效日期", "下次评审"), kept with its name, none promoted over another;
- **periods and calendars**: as 0045 decision 3 has them;
- **anchors the narrative sets**: an event the text has dated ("上市" is 2024-03), so a later "上市半年后" or "验厂后两个月" has something to be measured from.

The separate dating call over the opening goes away. The context is stored with the document as today.

**2. There is no document date on facts.** A date a source system gives (a feed's publish time, a filing date) enters the context as a reference point like any other. Nothing picks one date for the document and writes it on every statement. `documents.doc_time` stays for sources that date documents and for sorting the library; it is not an attestation.

**3. Every statement is judged for time where it is read.** The contract keeps `when` and `ended` and gains `as_of`:

- a statement with time words refers to mentions, as 0045 decision 1;
- a statement with none may take `as_of` from the context, naming the entry: a figure under "数据统计截止 2026/08/31" is as of that date; what a meeting decided is as of the meeting;
- a statement the context does not govern has no time.

As-of is not a start. "码表所属市场为全球" in a report of 4 September says the market was global on that day, not since that day. It is stored as the statement's attestation with the name of the entry that gave it (`attested_from`, `attested_by`), which is what 0045 decision 5 meant the column to hold; `valid_from` is written only when the words state a start.

**4. Relative expressions are measured from the reference point in force.** The model still returns an interpretation and code still computes (0045 decisions 2 and 8). What changes is that the reference point of the chunk is in front of the model when it reads "三个月后", so the expression is recorded as a mention with its anchor instead of being folded into the object. With no reference point in force the mention is grade C and waits (0045 decision 4).

**5. A fact with no time holds at every moment.** This answers 0045's first open question. `attested_from` is null unless decision 3 gave one; the insert no longer falls back to `now()`; a read at a moment T keeps a row that has neither a valid time nor an attestation. The workbench says "undated" for it, "as of 2026-08-31 (数据统计截止)" for an attested one, and an interval for one the words dated. Rows written before this carry the processing moment; a migration sets those to null where no entry of the document's context accounts for them.

**6. Interpretation rides with extraction.** With the context present, the mention and its interpretation are returned by the same call. The batch interpretation step stays only for re-resolution when an anchor arrives later (0045 cut 4).

## Not doing

- A single date per document chosen by a model, for any purpose.
- Filling a start from an as-of.
- A list of time words in code (0045 decision 8 stands): recording is measured, not pattern-matched.
- A second pass over every statement. One call per chunk stays one call per chunk.

## Measurement

A bench before any change, `scripts/bench/timewords.mjs`: documents in Chinese and English written with known expressions and expected intervals (absolute, relative to the text's now, relative to a dated event, relative to an undated event, as-of from a named date, under a section heading, none), three runs each.

| Number | Today | Threshold for cut 2 |
|---|---|---|
| time expressions recorded as mentions | 2 of 7 on the weekly report | 90% |
| recorded mentions resolved to the right interval | 2 of 2 | 95% absolute, 85% anchored (0045's) |
| statements with no time words given the right as-of | none | 80%, and none given a start |
| the same document, three runs: statements dated the same | differs | 90% agree |
| tokens per chunk | measured with cut 1 | no higher than extraction + dating + interpretation today |

## Cuts

1. The bench and today's numbers. No behaviour change.
2. The context in the extraction call: reference points, named dates, periods, narrative anchors; mentions with interpretations from the same call; the dating call removed.
3. `as_of` from the context, `attested_by`, null attestation, reads that keep undated rows, the workbench line. Replaces #988.
4. Events without a date as anchors, resolved when the event is dated (with 0045 cut 4).

## Open questions

- A section heading that is a period ("2025 年第三季度"): whether the statements under it are as of the period's end or hold during it. 0031's event bucket suggests the second for events and the first for states.
- How much context a long document accumulates, and what is dropped first when it outgrows the prompt.
- Two reference points in force at once (a report quoting minutes).
