# 0040 · A chunk says where its words came from

- **Status**: Proposed · nothing built · cut 1 is the ledger shape (`chunks.origin`,
  `chunks.origin_model`, `chunks.anchor`, the packer rule, the confidence ceiling, the read
  contract); the media readers follow in the order at the end
- **Written**: 2026-09-13 (conventions in the [README](README.md))
- **Related**: [0039](0039-a-chunk-is-what-extraction-sees.md) (#633, not merged yet) puts Docling
  behind the block model as its cut 2 and leaves "evidence that points at a table cell or an image
  region" to a later record — this is that record; [0020](0020-an-auditor-reads-it-without-us.md)
  is the read contract an auditor relies on; [0022](0022-an-unknown-date-is-not-an-open-one.md)
  anchors a fact at the document that attests it; [0005](0005-alert-center.md) lists
  `document.no_text_layer` as unwired; [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md)
  is the other queue a fact can wait in

## Why a decision is needed

Everything after the parser eats text. Chunking, embedding, extraction, resolution, the temporal
engine and the derivations never look at a file; they look at `chunks.text`. That is why reading
audio and images is, at the wire, mostly configuration: a transcription endpoint speaks
OpenAI's `/v1/audio/transcriptions`, a vision model is a chat request with an image part, and
scans go through the Docling sidecar 0039 already plans. Point three more settings at three more
models and the pipeline runs unchanged.

That is also the problem. The pipeline will swallow a transcript, or a model's account of what a
bar chart shows, exactly as it swallows a sentence from a filing, and the ledger has no column
that could tell them apart afterwards. `fact_evidence` holds a `chunk_id` and a `quote`, and the
quote means one thing: *the document says this*. Text can now arrive four ways, and they are not
equally good evidence:

| origin | example | what the quote is | how a person checks it |
|---|---|---|---|
| stated | the body of a document | the words written | read the document |
| ocr | a scanned contract, a screenshot, a stamped page | the words written, as read (a misread digit is possible) | look at the page |
| transcribed | a meeting recording | the words of a transcript (names are the likeliest misses, and names are what extraction extracts) | listen to that stretch |
| described | a chart, a photo, a diagram | **a model's paraphrase of a picture — nobody wrote or said it** | cannot be checked word for word |

The first three keep the contract every quote in this base is held to. The fourth does not.

It matters more here than in a search index, because **this ledger acts on what it is told**. A
new fact closes the open interval it supersedes (`reconcile_new_fact`), a contradiction opens a
conflict, and derivations are built on premises. A vision model reading a value off a bar chart
gets digits wrong as a matter of course. Filed as an ordinary fact, that wrong value closes the
correct one it contradicts, and nothing in the stored rows says the closure rested on a picture.

And one part cannot wait for later: **an anchor that is not stored at ingest cannot be recovered**.
A transcript saved without its segment times can never again be tied to the moment in the
recording a person would need to hear.

## Decisions

### 1. A chunk has an origin

`chunks.origin` is one of `stated`, `ocr`, `transcribed`, `described`, `NOT NULL DEFAULT
'stated'`; every existing row is stated, which is true. `chunks.origin_model` names the engine or
model that produced the text and is null for stated chunks. The model is kept so that a base can
be re-read when a better model arrives, and so that misreads can be counted per model rather than
argued about.

The origin lives on the chunk, not on `fact_evidence`. Every evidence row that cites a chunk
shares that chunk's origin, and evidence already reaches its document through the chunk. The
chunk is also the unit extraction reads (0039), so it is the unit whose reliability extraction
inherits.

### 2. The packer never mixes origins in one chunk

0039 reads a document into blocks and packs blocks into chunks. Blocks carry an origin, and a
chunk holds blocks of one origin only. A described block is always its own chunk.

A description needs context to be read at all — a chart is meaningless without "Figure 3: revenue
by region" — so a described chunk carries the heading breadcrumb 0039 gives every chunk, and the
figure's caption. The whole chunk counts as described, caption included. That is deliberately
conservative: the caption is still stated, and it still appears in its own stated chunk, where
facts cited from it keep full weight.

Without this rule a fact extracted from a chunk that held a sentence and a description would cite
the chunk, and the description would borrow the credibility of the sentence beside it.

### 3. A chunk points back into the original bytes

`chunks.anchor` is JSONB whose shape is fixed by the origin and checked:

| origin | anchor |
|---|---|
| stated | null — `char_start` / `char_end` already place it in the parsed text |
| ocr | `{"page": n}`, with `"bbox": [x0, y0, x1, y1]` when the engine gives one |
| transcribed | `{"start_ms": n, "end_ms": n}`, with `"speaker"` when the endpoint gives one |
| described | `{"page": n, "image": i}` for a PDF, `{"part": "word/media/image3.png"}` for an Office file, `{}` for a standalone image file |

The original is already kept: the document's blob, content-addressed and versioned in
`document_versions`. An anchor addresses bytes the base holds anyway, so reading media adds no
storage and no copy that could drift from its source. Images inside a document are not extracted
into blobs of their own.

Segment times are required of cut 3, not a nice-to-have, for the reason given above. Video, when
it comes, is a time range plus a frame: the transcribed and described shapes with `start_ms`.
Deciding the anchor now is what makes deferring video cost nothing.

### 4. A described observation cannot move the ledger by itself

Facts extracted from a `described` chunk are inserted with a confidence below
`AUTO_CLOSE_MIN_CONFIDENCE`. The temporal engine already refuses to auto-close on such a fact:
where it would have closed the open interval, it records a `low_confidence` conflict and a person
decides. No new branch in `temporal.rs` is needed. The fact still enters the graph, search and
chat. It can be read, found and cited; it cannot, alone, rewrite what the ledger holds.

The conflict queue is the right place because it only sees the facts that would act. Most
described facts collide with nothing and never appear there.

Only `described` gets the ceiling. OCR and transcription are readings of words with an anchor a
person can check. Their typical error is a misspelled name, which arrives as a separate entity and
goes through resolution and review, not as a closure of someone else's fact. A description is an
interpretation with nothing to check it against. Whether a misread digit in either should be
capped too is open (below), and is to be measured rather than assumed.

**A consequence, checked in the code.** A fact's confidence is set when it is first inserted. When
the same fact is observed again from the same start, `insert_fact_inner` returns the existing row
and leaves its confidence as it was. So a fact first seen in a chart and later stated in a
document stays below the threshold, and the conflict it opened waits for a person. This is safe,
and it depends on arrival order. Raising a fact's confidence when a better origin corroborates it
is not decided here.

### 5. Each modality has its own model settings

`llm_settings` already separates `chat_*` from `embed_*`. It gains `transcribe_*` and `vision_*`
(base URL, key, model) in the same OpenAI-compatible shape: transcription through
`/v1/audio/transcriptions` asking for segment timestamps, vision through a chat request with
image parts. OCR is the Docling sidecar of 0039's cut 2.

These are not a reuse of the chat model. A recording of a board meeting or a scanned contract is
more sensitive than a paragraph of text, and a deployment will reasonably keep transcription and
OCR on local models while chat goes to a hosted one. One setting would force the most sensitive
material to wherever chat happens to live.

An empty setting turns that modality off. A file that needs it is not silently skipped: it fails
with an alert that names the missing model. For scans, that alert is the
`document.no_text_layer` 0005 still lists as unwired.

### 6. Reading media is a job

`parse` runs inside `process_document` under `spawn_blocking`, and it is pure local work. A model
call is not: it is rate-limited, times out, runs out of credit. A two-hour recording that fails at
ninety minutes must not start again from zero.

Media reading is therefore its own job, queued like extraction (#640), writing blocks as segments
complete and resumable from the last one written, feeding the same chunker. Images inside one
document are deduplicated by content hash across the base — a logo on fifty slides is one
description — and skipped below a size floor. A per-document call budget reports what it skipped
instead of dropping it quietly.

### 7. The read contract says it

The evidence API, the MCP tool results (`quote`, `document_id` and `filename` today) and the RDF
export (`utopia:quote` on each statement) gain `origin`, `origin_model` and `anchor`. An auditor
reading the export must be able to tell a sentence from a description without asking us, which is
0020's whole premise. A chat source cited from a described chunk is labelled as a description.

The interface — the origin on each evidence row, and the anchor that opens the page, the image or
the recording at `start_ms` — is a separate cut after the capability, not part of it.

## Order of cuts

1. **The ledger shape.** `origin`, `origin_model`, `anchor`, the packer rule, the ceiling, the read
   contract. No media reader yet; everything that exists is stated, and says so.
2. **Scans and document images through Docling** (0039's cut 2): `ocr`. The highest value —
   scanned contracts, stamped approvals, invoices, all of which fail today with no text layer —
   and the modality that keeps the verbatim contract.
3. **Recordings**: `transcribed`, segment times required, speakers where the endpoint labels them.
4. **Charts, photos, diagrams**: `described`, under the ceiling.
5. **Video**: its audio track through 3, sampled frames through 4.

## Open

- **Who said it.** Whisper-compatible endpoints do not separate speakers. Without speakers, "Zhang
  San said he would deliver in Q3" and "Li Si said Zhang San would deliver in Q3" can be the same
  transcript line, and a meeting ingested that way attributes commitments to the wrong people —
  worse than extracting nothing. Cut 3 does not ship until this is decided: require an endpoint
  that labels speakers, or treat an unlabelled transcript's facts differently.
- **Corroboration.** Whether a stated or OCR observation of a fact first seen in a description
  should raise its confidence (decision 4).
- **Digits in OCR and transcripts.** Whether low engine confidence on a segment (Whisper's
  `avg_logprob`, an OCR word score) should cap facts from that segment the way `described` is
  capped. The recall bench needs a scanned and a recorded sample before this is decided.
- **What gets embedded.** An image reference or a long URL inside a chunk's text skews its vector.
  Whether the embedded text strips references is 0039's to answer.
- **A recording's date.** A container's creation time as a new `doc_time_source`, and how it sits
  with 0022's rules, is decided with cut 3.

## Alternatives

| Alternative | Why not |
|---|---|
| Put the origin on `fact_evidence` | Every row from one chunk would repeat it, and a chunk holding mixed origins could not be described honestly either way. The chunk is what extraction reads. |
| Treat every non-stated chunk as low confidence | Throws away OCR of a clean scan — verbatim and checkable — to guard against a risk that belongs to descriptions. |
| Keep descriptions out of the graph and in search only | Loses the chart that is the only place a figure is written down. The ceiling keeps them in without letting them act. |
| Route described facts to `pending_facts` (0015) | That queue waits for a nod on sentences a person said to the base. A single slide deck would bury those nods under hundreds of image facts, and a described fact is not anyone's assertion. |
| Copy images out as blobs of their own | The original file already holds them, content-addressed and versioned. An anchor into it costs nothing and cannot drift. |
| Reuse the chat model for vision and transcription | Sensitivity differs by modality, and one setting sends the most sensitive material wherever chat is hosted. |
