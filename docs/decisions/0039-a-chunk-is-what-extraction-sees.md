# 0039 · A chunk is what extraction sees

- **Status**: Implemented, cut 1 · the parser's Markdown is read into top-level blocks
  (`blocks.rs`), the packer works over blocks with a token budget (`chunker.rs`): a table
  travels with its caption and header, a table too wide for one chunk is split by rows and every
  continuation repeats caption and header, headings become a breadcrumb prefix and the
  `chunks.heading` column, a table split by a page break is joined back · not in this cut:
  an external parser behind the block model (cut 2, Docling for PDF layout and scans), evidence
  that points at a table cell or an image region, a budget measured for its own sake
- **Written**: 2026-09-13 (conventions in the [README](README.md))
- **Related**: [0006](0006-ontology-scale-and-the-prompt.md) sets the other half of the prompt
  budget; [0033](0033-rss-source-summaries-are-source-scoped.md) is the recall bench's home;
  the recall bench (`scripts/bench/recall.mjs`) is the instrument this record was measured with

> Four SEC filings, a 52-item truth table read off the originals by a person. Two rounds on
> `dev` on 2026-09-13 scored 43 and 45. Relation edges: 12/12 both times. Every miss was a
> literal — a price, a date, a title, a vote count — and the misses barely overlapped between
> rounds. The extractor was not failing to read the sentences; it was being handed tables it
> could not read.

## The problem

The chunker was twenty-nine lines: a character budget of 1200, an overlap of 150, boundaries
chosen by a generic text splitter that knows about paragraphs and sentences and nothing else.
Three things followed, all visible in the chunks stored for the bench corpus:

**A table's continuation lost its header.** Of the earnings release's 35 chunks, 13 began in the
middle of a table. The chunk that holds the non-GAAP EPS opens with `| | Non-GAAP* | | $ | 2.22 |
| | $ | 1.87 | …` — five dollar figures, no column names. Which quarter is $2.22? The model
guesses. Sometimes it guesses right, which is why that item was a miss in one round and a hit
in the next, and why the score moved by two between rounds with the same code.

**A table lost its caption.** "The results of the voting were as follows:" sat at the end of one
chunk and the votes it introduced started the next. The sentence that says what the numbers are
was not in the chunk with the numbers.

**A page break split a table, and the splitter took the break as a boundary.** The 8-K's HTML has
an `<hr>` where the paper page turned, mid-table for Stephen C. Neal's votes. The converter
renders it as a thematic break, the splitter prefers thematic breaks as split points, and the
second half — abstentions and broker non-votes — became a table with no name on it.

Underneath all three: the budget said 1200 characters and the comment beside it said "about
1000+ tokens". That is true of Chinese. English is about 300 tokens in 1200 characters, so a
wide table never fit, and the same document got a quarter of the context in one language that
it got in the other.

## Dead ends

**A smarter splitter alone.** `text-splitter` has a `MarkdownSplitter` that ranks headings above
paragraphs above sentences. It was the obvious first move and it does not do the thing that
matters: it ranks a table row at the same level as a table, so a table that exceeds the budget
is split between rows exactly as before, and nothing repeats the header. It also keeps thematic
breaks as a preferred boundary, which is the Neal failure. What it does give — heading
attachment, token sizing — this cut takes, but as a fallback for one oversized paragraph, not
as the chunker.

**Overlap as context.** The old 150-character overlap was meant to carry context across a
boundary. For prose it carries the tail of a sentence; for a table it carries the tail of the
previous row, which is the wrong information. Context is a header and a caption, and those have
to be chosen, not copied from wherever the previous chunk happened to end.

**An external document parser now.** Docling and Unstructured parse a document into structured
elements and their chunkers repeat table headers on continuation. For HTML, Markdown, DOCX and
spreadsheets our parser already produces the structure; adding a Python service to a Rust
pipeline for a rule we can write in fifty lines buys nothing here. Where those parsers earn
their cost is PDF layout and scanned pages, and that is cut 2 — behind the same block model,
so the chunker does not change when the parser does.

**Chunking the parser's output as blocks, without the join.** Reading the Markdown into blocks
fixes the header and caption. It does not fix Neal: the parser had already produced two tables,
the second with a separator row under a row of data, and a block reader faithfully reports two
tables. The join is a separate rule and it is deliberately narrow (below).

## Decisions

### 1. A chunk is what extraction sees, so its boundary is part of the extraction contract

The model is handed one chunk and nothing else; what the boundary cut away cannot be recovered
by the prompt. So the chunker's rules are written from the extractor's side: never split a table
row; never separate a table's header from its body; never separate a caption from its table;
never separate a heading from the first block under it. These are not typesetting preferences.
Each one, when broken, produced a specific miss in the bench.

### 2. The parser's text is read into blocks, and the packer works over blocks

`blocks.rs` reads the parser's Markdown with `pulldown-cmark` into a sequence of top-level
blocks — heading, paragraph, table (header lines and body rows as separate ranges), rule, other
— each carrying its byte range in the source. `chunker.rs` packs blocks. The split into two
layers is what makes cut 2 cheap: an external parser produces blocks and the packer does not
know or care.

Chunk text is assembled from verbatim slices of the source. Nothing is rewritten, so a quote the
model gives back is still found in the chunk by `span_in_quote`.

### 3. A table travels with its caption and its header, and a continuation repeats both

A short paragraph immediately before a table — at most 200 bytes, or ending in a colon — is the
table's caption and moves with it. A table that fits goes in whole. One that does not is split
between rows, and every piece is caption, header lines, then as many rows as fit. A row is never
split; a piece that is caption plus header plus one row and still over budget is emitted anyway,
because there is no better cut. This is the change that addresses the 13 headerless chunks
directly. The rest of this record makes it possible; this is the point.

### 4. The budget is tokens, not characters — and it is 300, because 1000 was measured and lost

> **Revised 2026-09-13, the same day, by the first bench round.** The paragraph below chose
> 1000 to honour the old comment. One round at 1000 scored 36/52 against 43 and 45 before, and
> the acquisition 8-K showed why: nine chunks became two, and from 4,700 characters the model
> wrote seven facts — the address, the phone number, "published in the SEC" — and not one about
> the acquisition. The extractor's output per call does not grow with its input; handed more, it
> picks the easiest few. The default is now 300, which is what the old character budget came to
> in English and the condition the 43/45 rounds were measured under. Chinese gets less context
> than it did; that is unmeasured and open. The budget is a knob (`UTOPIA_CHUNK_TOKENS`) so the
> next measurement does not need a rebuild.

Counted with `tiktoken` cl100k, which is embedded in the crate and needs no network. It is not
the extraction model's tokenizer — that is DeepSeek's — but it is a BPE with a similar ratio,
and the budget is a ceiling, not an accounting. 1000 is the number the old comment said the
character budget was meant to be. Chinese keeps roughly the context it had; English gets three
times as much, which is what lets a wide table fit at all.

Budget size is a knob with its own trade-off — fewer, larger calls against more facts per reply
— and it was not measured on its own in this cut. The bench comparison below therefore compares
structure and budget together. Measuring the budget alone is open.

### 5. Headings are a breadcrumb, written into the chunk and into `chunks.heading`

The heading path a chunk sits under is prepended to its text, as the heading lines themselves,
and stored as plain text in `chunks.heading`, a column that had existed since the ingest
migration and was never written. The prefix is skipped when the chunk's body already begins with
that heading. A heading never ends a chunk on its own; it attaches to what follows.

### 6. A thematic break is not a boundary, and a table split by one is joined back

The old splitter preferred to break at a horizontal rule. In this corpus a horizontal rule is a
page break, and a page break is where a table is most likely to be cut in half. Rules are dropped
from the block sequence and do not influence packing.

Separately, in `blocks.rs`: the sequence *table, rule, table* where the two tables have the same
column count and the second's "header" row begins with the same two words as one of the first's
body rows — `Number of shares Abstaining` after `Number of shares For` — is one table. The
second's header row becomes a body row of the first, and its separator line is dropped. The rule
is narrow on purpose. A wider one, "same width and the header holds numbers", would join two
financial tables whose headers are years, because `2025 | 2024` are numbers too. A test holds
both shapes.

### 7. No overlap

Blocks are semantic units. Context is the breadcrumb, the caption and the header, chosen for
what they are, not the tail of the previous chunk copied for where it happened to end. Overlap
also doubled the facts the extractor was asked to produce on the overlapped span; the ledger
deduplicates, but the calls were paid for.

### 8. `char_start` and `char_end` name the body, not the prefix

The two columns point at the verbatim span of the chunk's body in the parsed text; the prefix
(breadcrumb, repeated caption, repeated header) is copied from elsewhere and is not part of that
span. Nothing reads these columns today except one test fixture; this is recorded so that the
first reader knows what they get.

### 9. Every document is re-chunked on its next reprocess, and nothing is done in advance

`replace_chunks` claims old chunks by exact text. New text for every chunk means every chunk is
superseded and re-inserted, and the document re-extracted. That is the existing reprocess path;
old chunks keep their vectors and their evidence links as before. There is no migration to
re-chunk the whole deployment: the old chunks are not wrong, they are worse, and the next
reprocess of each document is when they improve.

## What the bench found (2026-09-13)

Before, two rounds on `dev` unchanged: **43/52** and **45/52**; relations 12/12 in both; the
stable misses across both rounds were three, none of them present in `extraction_drops` or
`ontology_misses` — never extracted, not extracted and dropped.

After, with `--reprocess` so every chunk is rebuilt. At 1000 tokens: **36/52**, relations 8/12,
the acquisition 8-K 4/11 with seven facts from two chunks — the round that overturned decision 4.
At 300 tokens: **37/52**, but not a measurement: the extraction model's account ran out of
balance (HTTP 402) at 01:22 while the Ohio exhibit was being extracted, and its chunks 3–7 —
the paragraph holding all seven of its numbers — were never sent. On the three documents that
finished before the balance ran out the round scored 29/37, against 29/37 and 33/37 for the two
rounds before; within one standard deviation of both. The round is to be rerun once the account
is funded, and the number that matters is the Ohio exhibit's.

Seen in the rebuilt chunks, independent of the score: no chunk begins inside a table without
its header (was 13 of 35 in the earnings release); the Neal vote table is one table again with
no rule inside it; the chunk holding the non-GAAP EPS carries a header — the parser's layout row,
not the column names, which is the open question below.

## Open questions

- **The budget on its own.** 1000 tokens was chosen to honour the old comment, not measured.
  The same bench at 300 tokens (character parity for English) and at 1000 would separate the
  structural change from the size change. Two runs of twenty minutes each.
- **Evidence that points at a cell or a region.** A fact's evidence is a text span. For a table
  it should be able to name a row and a column; for a figure, a region on a page. That is the
  precondition for extracting from images at all and it is the same ledger discipline —
  document, evidence, fact — extended to two more kinds of evidence. Not this record.
- **The header the parser hands over.** The chunker repeats whatever the parser marked as the
  header. In the XBRL earnings release that is `| | NVIDIA CORPORATION | | … |` — a layout row
  promoted because the table had no `<th>` — while the rows that actually name the columns
  (`Three Months Ended | Six Months Ended`, then the dates) are the first body rows and stay in
  the first piece. Seen on the re-chunked corpus: the chunk holding the non-GAAP EPS now opens
  with a header and still does not say which quarter is which. The mechanism is right and the
  input is wrong; choosing the header rows is `promote_first_row_headers`' job and belongs with
  the `head_blank` rule already there. Not this cut.
- **DOCX and PDF tables.** The DOCX parser flattens tables to paragraphs; the PDF parser has no
  tables. Both arrive at the block reader as prose and get prose treatment. Spreadsheets and CSV
  come out as tab- and pipe-separated lines rather than Markdown tables and are likewise read as
  paragraphs. Emitting real Markdown tables from those four parsers is cheap for the first two
  and is cut 2 for the last two.
