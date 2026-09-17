# Extraction writes what the document says

Extraction turns a chunk into open statements. This file says what it reads, what it writes, what it
refuses, and how it is measured. Records: [0044] (the contract), [0039] and [0040] (the chunk it
sees, in [sources](sources.md)), [0041] (names), [0001] (drop signals), [0015] (memory documents),
and the superseded typed contract of [0006], [0012], [0031], [0037]. Cut 1 landed in #731, memory
documents joined in #735, and the typed path is deleted in #736.

## What it does today

**Input.** One call per chunk, at temperature 0, with the document's filename, the things already
accepted earlier in the document, the document's opening (its first 1,500 characters, #680) and the
chunk text. No ontology, no document date, no request for dates [0044 d2, #731]. The chunk is the
whole context: what its boundary cut away cannot be recovered by the prompt, so the chunker's rules
are extraction's rules [0039].

**Output, the compact contract** (`utopia_extract::open`). `e`: things, each a name or a
description, the document's kind word, and whether it is named or described. `s`: statements, each
naming its subject and object in words, the relation phrase as written, an object name or a literal
value, qualifiers keyed by the document's role words, `when` / `ended` time words, and its own
verbatim quote. `n`: other names the document gives a thing. Statements name their sides in words
and carry their own quote because a numbered contract mixed the ids up on dense passages (misworded
10 to 14% with ids, 2 to 7% with names) [#731]. Parsing is per item: a malformed item is counted and
a truncated reply is repaired to its last complete item.

**What is a thing, what is a statement** [#743, prior-work items 1 and 6]. A thing is named when its
name is a proper name or a fixed term, one that means the same thing in any document (a person, an
organization, a product, a place, a document, a law, an event; a disease, a drug, an industry, a
product category, an indicator); it is described when it is a role or a generic phrase whose
referent the passage decides ("the company", "patients", "各部门"), however particular it is
there. Named things resolve across documents by name [0041]; described things exist only in their
document and only when a statement points at them. A phrase is the verb with the words that belong
to it, so that subject, phrase and object read as a sentence on their own, never a bare verb cut
from a longer verb phrase; several verbs sharing one object are one statement, one verb with several
objects is one statement per object. The object is what the verb acts on; where, how, why, with
what and for whom go in qualifiers under the passage's own role word, a named thing mentioned there
is still listed and the qualifier names it, and a generic phrase that appears only there stays
words in the qualifier. In a table, a cell is a statement about its row's thing whose phrase is
the column heading; when the heading names a time it is `when` instead, the statement is about
the thing the caption names and the phrase is the row label with its section path, and a unit the
caption gives is a qualifier [#744]. These are contract rules, not server checks: the shape checks
below look at structure, never at vocabulary.

**Server checks, each a drop reason in `extraction_drops`.** Every quote must occur in the chunk
(`quote_not_in_chunk`); every name in its quote (`name_not_in_text`); a time mention only when its
words occur in the statement's own quote (`time_not_in_quote`), because the model otherwise attaches
one time to unrelated sentences; every subject must be a listed thing (`unknown_ref`); an unlisted
object becomes a literal value with a signal (`object_undeclared`); a string one entity already
claims in the document is not a name of another (`name_claimed_by_another`). Offsets are computed on
the server by locating the quote, never taken from the model. The remaining codes are
`malformed_item`, `truncated_reply` and `object_missing`; a phrase that is the value itself or the
subject's own name is kept and counted (`phrase_is_value`, `phrase_is_subject`, the two shapes a
model writes for a table row that lost its heading or its caption; the subject and the value are
there, so nothing is lost, #744); the 28 codes of the typed path went with it [#736]. A drop is a
row, never silence; the table is cleared per document when extraction starts
and shown per document in the Library, apart from Review [0001, 0005].

**What it writes** (see [ledger](ledger.md)). Open statements as `facts` rows with `layer = 'open'`,
the phrase, no predicate and no `valid_*`; `statement_qualifiers` keyed by role word;
`time_mentions` as verbatim words with an offset; evidence with `quote_start` / `quote_end`; a
described thing as an entity with `description` and no name fact; name facts on `known_as` [0041];
`attested_at` from the document's date only when that date came from the content or a dating
source, never the upload time (#714). Dedup is on (base, subject, phrase, object), which closes the
NULL-predicate dedup hole of [0010].

**What it skips.** The ontology lists and per-chunk retrieval, predicate matching, the temporal
reconcile, signature checks, type resolution and ontology bootstrap [#731, #736]. Identity still
runs per handle (`resolve_handle`, namesake reviews, the govern and adjudicate enqueues); a chunk of
origin `described` keeps its confidence ceiling [0040]. Until alignment, a base holds open
statements and no typed facts, so timelines, conflicts and signature queues stay empty.

**Memory documents** take the same path; their statements wait in `pending_facts` with phrase,
qualifiers, time words and quote span, and a nod writes an open statement [0015, #735].

## Why

- **Statements in the document's words are faithful**: 0 of 333 not stated and 2% misworded in the
  prototype, against 4 to 9% not stated when bound at write time [0044].
- **No ontology in the prompt**: binding happens per signature and the ontology grows with use; a
  1,010-class ontology cost 108k tokens a chunk and made the model choose worse [0044, 0006].
- **No dates from the model**: arithmetic inside the model leaves no trace, and "today" resolved
  against the upload day; time is words now and an interpretation later [0045, #714].
- **Code checks structure, never vocabulary.** A name is kept because it is in the text, not because
  a list says it is a name; the same rule for quotes and time words [0041, 0044 d8].
- **A described thing is an entity** and gets no name fact, so a description never bridges two
  documents [#731].
- **Named sides and per-statement quotes** cost tokens and buy the not-stated rate [#731].
- **Every drop is a row** because an append-only ledger with evidence on every fact cannot drop
  silently [0001].
- **Temperature 0** because the endpoint default swung one paragraph between 8 and 31 statements
  [#731].

## Benches and thresholds

`scripts/bench/recall.mjs` (SEC filings, pharma, ai-timeline; Re-DocRED fetched by script and never
used as prompt examples) scores entity-pair recall; `judge_open.mjs` has a judge model read the
chunk and reports stated, misworded and not stated, and separately whether the statement reads on
its own without the document (`alone`, the uninformative-phrase class of [prior-work](prior-work.md)
item 1); `identity.mjs`, `govern.mjs`, `temporal.mjs`
and the lease bench cover the other domains. Every cut reports at least three domains, two runs per
configuration, with the judge's calibration stated; no F1 against Re-DocRED, whose gold omits true
facts [0044]. Thresholds: not stated at most 2%, entity-pair recall no lower than before, prompt
tokens per document reported. Measured at cut 1: not stated at or under 2% in 16 of 18
corpus-rounds; NVDA full documents 44 to 46 of 52 (one standard deviation about 2.7) against 43 and
45 before [#731, 0039].

## Superseded

The typed contract: the ontology laid out under a 24,000-character budget or retrieved per chunk
with ancestor completion [0006]; `[event]` / `[eternal]` marks and the zoned-time sentence [0031,
0024]; qualifiers listed after a relation and keyed by a declared attribute [0037]; direction
corrected by signature at write time [0012]; `specific_type` and `proposed_type` for type
resolution [0001]; the model's `relative` flag on a deadline [0022]; the word lists for entity names
and clauses [0012]. The ledger rules those records made stand (see [ledger](ledger.md)).

## Proposed and not built

- Alignment producing typed facts from statements [0044 cut 2]; time interpretation and the document
  time context [0045]; identity profiles [0044 d6]; a layer marker in the interface (an open row
  renders as an unnamed relation labelled by its phrase).
- A second pass asking which figures and dates are not yet in a statement (dense sentences drop the
  amount); the period column of a financial-table cell [#729].
- The errata agent over the typed graph [0044 d7].

## Open questions

- Chinese chunks get less context at 300 tokens than at 1,200 characters; unmeasured [0039].
- Whether a stated observation should raise the confidence of a fact first seen in a description
  [0040].
- Recall of facts that need more than one sentence waits for derivation rules [0044].
