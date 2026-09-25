# A source hands over its history

Records: [0013] (connectors), [0023] and [0033] (RSS), [0039] (chunks), [0040] (origin and media),
[0035] (vector index), [0036] d7 (schema documents), [0005] (sync alerts), [0022] and [0045] d3
(document dates), [0016] E.

## What it does today

**Ingest.** A file or a synced item goes into a content-addressed blob store; `documents` and
`document_versions` keep every version; `ingest_item()` owns identity (`external_key`), sha256 dedup
and versions; a document is `ready` for search once embedded while extraction queues behind it
[0001, 0013, [pipeline](../pipeline.md)]. `documents.doc_time` with `doc_time_source` (content,
source, upload_time, none) is the document's own date; upload time is recorded time and never a
document date, so an upload-dated document counts as undated for attestation and ordering
[0022, 0045, #714].

**Connectors.** GitHub issues, Jira, Notion, RSS, WebDAV, S3 / Azure / GCS, and custom push with an
`ingest_token`; the kinds come from one `SourceKind` enum that the allowlist, the dispatcher and the
web list derive from; credentials never leave the server (`SOURCE_SECRET_KEYS`) and a blank field
keeps the stored value; a truncated round says so [0013]. A ticket is one document whose body
carries its history as dated declarative sentences and a `## History` list; GitHub fetches events
per ticket; timestamps are written to the second [0013].

**Statements.** A `statements` source takes the open extraction contract itself (`e`, `s`, `n`) on `POST /sources/{id}/statements`, with the `api` push's identity, versions, tombstones and run history; the payload is the document and its single chunk, and extraction parses the chunk instead of prompting a model, then runs the unchanged path, so a pushed statement is an open statement with the item as its evidence (null offsets) and reaches the typed graph only through alignment. The contract has no slot for a property or a class and unknown keys are refused; a second push under the same identity marks the earlier statements stale rather than closing them; tables belong on a mount, not here [0054].

**RSS.** Observations are not documents: one table with baseline, candidate and no_source rows;
jobs own attempts, documents own identity; an entry without a GUID or an article link is skipped;
purge and reappearance are fenced by database time; one Readability extractor serves linked pages
[0023]. The source list aggregates only the listed source's current generation with a lateral join,
and the API carries a typed nested `rss_full_content` summary [0033].

**Parse and chunk.** The parser's Markdown is read into blocks (heading, paragraph, table with header
and rows, rule); the packer fills a token budget (300 by default, cl100k, `UTOPIA_CHUNK_TOKENS`); a
table travels with its caption and header and a continuation repeats both; a caption attaches to
every table that follows it; a row is never split; headings become a breadcrumb prefix and
`chunks.heading`; a thematic break is not a boundary and a table it split is joined back; no
overlap; chunk text is verbatim slices so a quote is found by offset; a document is re-chunked on
its next reprocess [0039].

**Tables** [#744, #750, prior-work item 26]. An HTML table is rendered from the DOM before the Markdown
converter sees it, by structure alone: cells hidden by style are skipped, spanning cells are laid on
a grid, columns empty in every row go, a cell holding only a symbol ("$", ")") rejoins the number
beside it. Rows are classified by what they hold: a lone cell before any header or data is a
caption line, a row of words with no label column is a header row, a lone label after the headers
is a section, a label followed by figures is data. Header rows are stacked per column into one
heading ("Three Months Ended July 26, 2026"); sections fold into the labels of the rows under them
("Current assets › Accounts receivable, net"), with indentation read from leading empty cells or
the label's padding, and a section ends at a row of its own depth only once it has had a deeper
child; caption lines become a paragraph before the table that ends with a colon. The chunker then
does what it already did: the caption and the header travel with every piece, and a run of up to
five short paragraphs before a table is its caption, so a statement's chunk names the report, the
unit and the period. Nested tables and the tables of other parsers still take the converter's
path with the first row promoted to header. Measured on the NVDA earnings release: 52 chunks
became 32, none of them a table without its header. DOCX tables, spreadsheets and CSV files are laid on the same grid by their parsers (#750): a DOCX
cell keeps its column span and the indent of its first paragraph, a sheet's or a CSV file's first
row with two or more cells is its header even when the headings are years, and a table with no
figure in it is read as a header row followed by records; the text layer of a PDF carries no table
structure.

**Origin and anchor.** `chunks.origin` (stated, ocr, transcribed, described), `origin_model`, and an
`anchor` whose shape is checked per origin (page and box; start, end and speakers; page and image or
part); the packer never mixes origins in one chunk; a described block is its own chunk with the
breadcrumb and caption. Images and recordings are recognised by header or extension and a PDF with
an empty text layer is a scan; without the reader the document fails once with `reader_needed` and
a `document.needs_reader` alert, and saving the setting queues it again [0040]. Scans and images go
to a workspace's MinerU service (`llm_settings.ocr_*`), one segment per page, the job waiting with
`Deferred` on the remote task recorded on the document; recordings go to a diarizing transcription
model (`transcribe_*`) and a transcript without speaker labels is refused; speakers are written into
the text as turns [0040 cuts 2 and 3]. Facts from a described chunk enter below the auto-close
threshold [0040 d4].

**Embedding and index.** `chunks.embedding` and `entities.profile_embedding` are `vector` with no
fixed dimension; the first write of a dimension requests a partial HNSW expression index built by a
`build_vector_index` job outside any transaction (512 MB `maintenance_work_mem`); reads write the
dimension as a literal, set `hnsw.iterative_scan = relaxed_order` with a raised scan memory, and let
the planner choose; above 2000 dimensions stays exact [0035]. Superseded chunks keep their vectors
so retrieval rewinds [0019].

**A schema document is a search corpus, not a source of facts**: a mounted database's schema is
indexed and never extracted (`sources.config.extract`) [0036 d7].

## Why

- **A source is worth connecting if it has real timestamps, can contradict itself, has a stable
  identity and holds enterprise knowledge**; the valuable part of a ticket is its timeline, and most
  systems keep it and only need to be asked [0013].
- **Sentences with dates extract to facts with a start; key-value pairs make the model guess**
  [0013].
- **Fixtures come from real responses**; hand-written JSON proves only the imagined shape [0013].
- **No provider abstraction until a third source shares a shape** [0013].
- **A feed item can vanish before acquisition and a summary is not full content**, so observations
  are retained apart from documents [0023].
- **The chunk boundary is part of the extraction contract**: 13 of 35 chunks began mid-table without
  a header and every miss was a literal; a 1,000-token budget lost (36 of 52) because the extractor's
  output does not grow with its input [0039].
- **The pipeline eats text and cannot tell a paraphrase of a chart from a sentence**; the ledger acts
  on what it is told, so a description may not close a correct fact on its own [0040].
- **Per-modality settings** because a board recording is more sensitive than chat and may stay on a
  local model [0040].
- **A job, not a migration, for the index**: the dimension is known only when vectors arrive and
  `CREATE INDEX CONCURRENTLY` cannot run inside a migration; `ivfflat` loses recall quietly [0035].
- **Truncation and partial rounds are reported**, or "sync complete" misleads [0013].

## Proposed and not built

- Descriptions of charts and photos under the ceiling (cut 4) and video (cut 5); the origin and
  anchor in the interface, opening the page, image or recording [0040].
- Feishu and Confluence connectors with a block-tree renderer and sampled versions [0013].
- An external parser behind the block model for PDF layout; evidence naming a table cell or an image
  region; real Markdown tables from the DOCX, PDF, spreadsheet and CSV parsers, rendered the way
  HTML tables now are [0039].
- Backup and restore (#712), a 100k-document benchmark (#713) [0016 E].

## Open questions

- Whether low engine confidence on an OCR or transcript segment should cap facts [0040].
- The header the parser hands over, and the budget measured on its own [0039].
- A recording's date as a `doc_time_source`; what gets embedded when a chunk holds references
  [0040].
- Recall on two large tenants of different subject matter; an old dimension's index when a
  workspace changes model [0035].
