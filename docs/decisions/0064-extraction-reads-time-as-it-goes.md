# 0064 · Extraction reads time as it goes

- **Status**: In progress · accepted 2026-09-28 · cut 1 built (#990, the bench) · cut 2 built (#991): each chunk reports the dates its passage states with the headings they sit under, a relative expression is measured from the reference point of its own section, a heading that names one time dates the statements under it, a quarter and a week are read · cut 3 built (#995, migration 0100): a statement is attested by the moment the text of its own section speaks from, with that date's name; no attestation when the document states none, and such a row holds at every moment; the workbench says "as of" · open: cut 4 · decision 1 is revised below: the context does not depend on the order chunks are read in
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

**1. The time context is built while extracting, from what each chunk states.** Each extraction call returns, beside statements and time mentions, the dates its passage states about the document's own time. It holds four kinds of entry, each with the words as written and the chunk they came from:

- **reference points**: the moment the text speaks from ("提报日期 2026年9月4日", a dateline, "会议时间"), with the span it governs: the document, or the section under a heading;
- **named dates**: every date the text gives a name to ("数据统计截止", "生效日期", "下次评审"), kept with its name, none promoted over another;
- **periods and calendars**: as 0045 decision 3 has them;
- **anchors the narrative sets**: an event the text has dated ("上市" is 2024-03), so a later "上市半年后" or "验厂后两个月" has something to be measured from.

The separate dating call over the opening goes away. Cut 2 kept it as a fallback for documents where extraction reported no entry; measured on twenty Re-DocRED documents, sixteen took that fallback, each asking the question extraction had just answered with "none", about 940 tokens a document, and on the time bench it never found a date extraction had missed. It was removed (#998). The context is stored with the document as today.

*Revised with cut 2 (2026-09-28).* As first written this decision had the context travel from chunk to chunk in the prompt, the way the list of recorded things does. That ties time to the order chunks are read in, and #588 is moving the model calls of one document to run at once. What an entry governs is decided by structure instead: the chunker opens every chunk with the headings it lives under, so the chunk's own text says which section a date was stated in and which section a time word sits in. The scope of an entry is the heading path at its position; the reference point in force for a mention is the `now` entry with the deepest scope that is a prefix of the mention's path. Nothing is passed between calls, and the result is the same whether chunks are read one after another or together. Narrative anchors (the fourth kind) stay with the mentions of 0045: a dated mention is already an anchor for a later one in the same document.

**2. There is no document date on facts.** A date a source system gives (a feed's publish time, a filing date) enters the context as a reference point like any other. Nothing picks one date for the document and writes it on every statement. `documents.doc_time` stays for sources that date documents and for sorting the library; it is not an attestation.

**3. Every statement is judged for time where it is read.** The contract keeps `when` and `ended` and gains `as_of`:

- a statement with time words refers to mentions, as 0045 decision 1;
- a statement with none may take `as_of` from the context, naming the entry: a figure under "数据统计截止 2026/08/31" is as of that date; what a meeting decided is as of the meeting;
- a statement the context does not govern has no time.

As-of is not a start. "码表所属市场为全球" in a report of 4 September says the market was global on that day, not since that day. It is stored as the statement's attestation with the name of the entry that gave it (`attested_from`, `attested_by`), which is what 0045 decision 5 meant the column to hold; `valid_from` is written only when the words state a start.

*As built with cut 3.* The entry is chosen by structure, not named by the model per statement: a statement is attested by the `now` entry in force at the position of its quote, the same rule that gives a relative expression its reference point. The other named dates of a section (a data cut-off, an effective date) are kept in the context with their names and do not attest anything yet; choosing between them for one statement is a judgement about the statement and would cost a slot in the extraction reply. A statement a person nodded through is attested at the nod, as before: the person is the evidence.

**4. Relative expressions are measured from the reference point in force.** The model still returns an interpretation and code still computes (0045 decisions 2 and 8). What changes is that the reference point of the chunk is in front of the model when it reads "三个月后", so the expression is recorded as a mention with its anchor instead of being folded into the object. With no reference point in force the mention is grade C and waits (0045 decision 4).

**5. A fact with no time holds at every moment.** This answers 0045's first open question. `attested_from` is null unless decision 3 gave one; the insert no longer falls back to `now()`; a read at a moment T keeps a row that has neither a valid time nor an attestation. The workbench says "undated" for it, "as of 2026-08-31 (数据统计截止)" for an attested one, and an interval for one the words dated. Rows written before this carry the processing moment; migration 0100 moves open statements to their earliest dated evidence and sets the rest to null, and typed rows follow the statements they were materialised from. A row a rule implied (0073) follows the statements the rule read; one a kind word implied reads no statement and has no attestation. Migration 0101 moves the rule rows written before this. An event with no date holds at no moment (0022), attested or not. An ending said without a date needs an anchor by CHECK; with no attestation it takes the moment the ledger recorded it.

**6. Interpretation rides with extraction.** With the context present, the mention and its interpretation are returned by the same call. The batch interpretation step stays only for re-resolution when an anchor arrives later (0045 cut 4).

*Not built with cut 2.* The batch step stays as it is; it asks once more for the mentions a reply left out, and says in the log which item it could not read. Folding it into the extraction call is measured separately: the extraction prompt is the one every other number depends on.

## Not doing

- A single date per document chosen by a model, for any purpose.
- Filling a start from an as-of.
- A list of time words in code (0045 decision 8 stands): recording is measured, not pattern-matched.
- A second pass over every statement. One call per chunk stays one call per chunk.

## Measurement

A bench before any change, `scripts/bench/timewords.mjs`: documents in Chinese and English written with known expressions and expected intervals (absolute, relative to the text's now, relative to a dated event, relative to an undated event, as-of from a named date, under a section heading, none), three runs each.

| Number | Today | Threshold for cut 2 |
|---|---|---|
| time expressions recorded as mentions | 18–20 of 29 (the bench; the first hand check said 2 of 7 on one report and was too low) | 90% · cut 2: 27–29 of 29 |
| starts right | absolute 4 of 4, relative to now 9 of 15, relative to a dated event 3 of 3, on a heading 0 of 6 | 95% absolute, 85% anchored (0045's) · cut 2: 11 of 12, 42 of 45, 9 of 9, 18 of 18 over three runs |
| statements with no time words given the right as-of | none | 80%, and none given a start · cut 3: 28 of 30 over three runs, none given a start |
| the same document, three runs: sentences with the same outcome | 34 of 39 | 90% agree · cut 2: 34 of 39, the differences are sentences not extracted at all |
| tokens per chunk | 5.7k–7.3k | no higher than today · cut 2: 6.1k–8.5k, about a tenth higher |

## Cuts

1. The bench and today's numbers. No behaviour change.
2. The context in the extraction call: reference points, named dates, periods, narrative anchors; mentions with interpretations from the same call; the dating call removed.
3. `as_of` from the context, `attested_by`, null attestation, reads that keep undated rows, the workbench line. Replaces #988.
4. Events without a date as anchors, resolved when the event is dated (with 0045 cut 4).

## Open questions

- A section heading that is a period ("2025 年第三季度"): whether the statements under it are as of the period's end or hold during it. 0031's event bucket suggests the second for events and the first for states.
- How much context a long document accumulates, and what is dropped first when it outgrows the prompt.
- Two reference points in force at once (a report quoting minutes).
