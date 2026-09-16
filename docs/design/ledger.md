# The ledger appends and never rewrites

Records: [0001] P0 and P1, [0002] d3, [0003] d1 and d2, [0010], [0011], [0015], [0019], [0020],
[0022], [0023], [0037], [0040] d4, [0041] d1, [0044] cut 1 (#731, #735). Time columns are explained
in [time](time.md), derived facts in [rules](rules.md).

## What it does today

**A fact is a row that is never updated in place.** `facts (kb, subject_id, predicate_id NULL,
object_id | object_value, confidence, valid_*, *_precision, attested_from, attested_to, recorded_at,
invalidated_at, supersedes, end_derived, layer, phrase)`. A correction inserts a new row that
`supersedes` the old one and invalidates it; a retraction invalidates; `entity_history` reads the
chain back as "recorded as X, refined to Y, by whom" [0001, 0003]. Confirming a mapping by
`UPDATE ... confidence = 1.0` was the one violation and is why mappings left the ledger [0011].

**Two layers.** `layer = 'open'` rows are what a document says: the relation phrase in `phrase`, no
predicate, no `valid_*`, dedup on (base, subject, phrase, object), `proposed_predicate = phrase` on
the evidence so every read path that tolerates a NULL predicate labels the row by its phrase.
`layer = 'typed'` rows are facts in the ontology's vocabulary; today they are the rows written before
2026-09-16, and alignment will write new ones with `from_statement_id` [#731, 0044]. A predicate is
nullable on both: an unnamed relation is empty, `fact_surface_predicate()` shows the most frequent
source wording, the reader is told it is the source's word, and `no_predicate_still_shows.rs`
guards every read path [0010].

**Evidence.** `fact_evidence (fact, chunk NOT NULL, quote, proposed_predicate, quote_start,
quote_end)`, one row per observation; offsets are computed on the server. A chunk carries `origin`
(stated, ocr, transcribed, described), `origin_model` and an `anchor` into the original bytes, so
evidence says how the words were obtained [0040]. A fact from a described chunk is inserted below
`AUTO_CLOSE_MIN_CONFIDENCE`, so it opens a conflict instead of closing a correct fact [0040 d4].

**Beside the row.** `statement_qualifiers (fact, role, value | entity)` keyed by the document's role
word for open statements; `fact_qualifiers (fact, qualifier_type, value | entity)` keyed by a
declared attribute for typed rows, converted by the attribute's datatype, the unit read from the
text (`$4 billion` becomes 4000000000 with unit `$`) and the declared unit only when the text
carries none; a qualifier is not part of the edge's identity; every superseding writer copies
qualifiers with the evidence [0037, #731]. `time_mentions (fact, chunk, text, char_start)` hold
verbatim time words [#731]. Names are value facts on `known_as` [0041]. `entities.description`
marks a described, unnamed thing [#731].

**Derived facts live in their own table** (`derived_facts`, widened to literal objects, with
`fact_derivations` as a proof tree) and never enter `facts`; an asserted triple is never derived,
and a derivation that contradicts an assertion does not land [0002, 0021, 0030].

**Pending facts are their own table.** A memory document's statements wait in `pending_facts`
(chunk, phrase, qualifiers, time words, quote span, `proposed_by`) and are written to `facts` only
on a nod; rejections go to `rejected_facts` keyed by subject, phrase and object and block only that
phrase; a nod takes the extraction write path [0015, #735].

**The record axis.** `invalidated_at` is the only way a fact leaves; `record_axis::held_at` is the one
predicate; deleting a document invalidates the facts it was the last source of and leaves a
tombstone, so the state before is reachable `as_of` [0019]. Purge releases a document's key while
its deletion history keeps the external identity, so an old backlog cannot resurrect purged content
[0023]. Alerts keep 30 days; `audit_events` keeps forever [0005].

**Drops are rows.** `extraction_drops` records what did not land, cleared per document at the start
of extraction; signals that are not drops (`object_undeclared`) are recorded the same way so nothing
is silent [0001].

**Export.** Every fact is an `rdf:Statement` with world time (`schema:validFrom` / `validThrough`),
record time (`prov:generatedAtTime` / `invalidatedAtTime`), confidence, evidence with its origin,
qualifiers as triples on the statement node, and `prov:wasDerivedFrom` its documents; a fact
currently held and valid is also written as the plain triple [0020, 0037, 0040].

## Why

- **The change of belief is information**: a correction that overwrites loses what was believed and
  when; a superseding row keeps both and makes the second clock rewindable [0001, 0019].
- **A null predicate is honest; a relation named "related" is an assertion.** 533 facts displayed
  "related" while their real wording sat in the evidence [0010].
- **A status column on `facts` fails the wrong way**: 56 queries filter live facts by
  `invalidated_at IS NULL`, and one missed `AND nod <> 'pending'` leaks an unconfirmed fact; a
  separate table hides the queue instead [0015, 0002 d3].
- **A state encoded as a float lies twice**: it lands in the low-confidence queue and cannot express
  "in effect" [0011, 0015].
- **The edge already has the standing of a node** (an id, evidence, conflicts, a reified statement in
  the export); it lacked only a place for an attribute [0037].
- **The origin lives on the chunk** because every evidence row from one chunk shares it and the chunk
  is what extraction reads [0040].
- **An anchor not stored at ingest cannot be recovered**: a transcript without its times can never be
  tied to the recording [0040].
- **A name is a value, never a node**, so names add no edge, no count and no conflict [0041].
- **Reification, not RDF-star or named graphs**, because the file has to open in what the reader
  already runs [0020].
- **Every read path must tolerate a NULL predicate**: eleven inner joins became LEFT JOINs and the
  database caught three misses the compiler could not; SQL changes need DB tests [0010].

## Proposed and not built

- Typed rows carrying `from_statement_id`, marked implied when a rule produced them, and recomputed
  per signature [0044 cut 2].
- Two rows and a `fact_conflicts` entry when two mentions of one typed edge disagree on a qualifier
  (today the first value stays and the disagreement is a drop); an entity-valued qualifier (column
  reserved) [0037].
- A unique index or per-base lock behind `insert_fact_inner`'s read-then-write dedup, which let two
  parallel documents insert one edge twice [0037].
- Versioned proofs: `fact_derivations` is rewritten in place when a kept conclusion is reproved
  [0019, 0030].
- Conflict and review state, chunk identity behind a quote, and historical proof snapshots in the
  export [0020].

## Open questions

- Whether corroboration by a stated observation should raise a described fact's confidence [0040].
- Whether the anchor should be shown beside a blank start [0022].
- Retention of `action_runs` when actions land [0034].
