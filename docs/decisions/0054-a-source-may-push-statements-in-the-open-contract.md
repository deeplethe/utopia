# 0054 · A source may push statements in the open contract

- **Status**: proposed · cut 1 in this record's PR: the `statements` source kind, `POST /sources/{id}/statements`, deterministic extraction, the Library entry · no schema change
- **Written**: 2026-09-23
- **Related**: [0044](0044-the-ontology-is-a-view-over-what-documents-say.md) owns the contract this reuses and the rule that typed facts come only from alignment; [0001](0001-extraction.md) is why every statement has evidence; [0022](0022-a-fact-has-two-clocks.md) is the two clocks a pushed item lands on; [0036](0036-exploration-aligns-a-schema-to-the-ontology.md) is where structured *state* lives, which this record leaves alone; [0015](0015-recording-a-sentence-is-not-asserting-a-fact.md) is why a person's `remember` needs a nod and a source's document does not; #875 is the case that surfaced it.

> A robot's perception stack, an ERP's event bus, a sensor gateway: each already holds `{thing, relation, value, when}`. Today the only way in is to spell that into prose, push it as a document, chunk it, and pay a model call to read the prose back into a statement. The typed value becomes a sentence and then a guess at the sentence; the round trip is slow, non-deterministic and costs a model call per chunk for input that was never ambiguous. The base can read a table row without a model (#744). It cannot read a row that arrives on its own.

## What the ground already gives, and what it withholds

Four parts are reusable as they stand:

- **A push interface with identity.** `POST /sources/{id}/ingest` on an `api` source: a per-source bearer key, an `external_id` that makes a second push an update in place with a version recorded, a tombstone, and a run row per call (`ingest_item_with_outcome`).
- **The open contract and its parser.** `utopia_extract::open` defines the compact reply (`e` things, `s` statements, `n` names) and `parse_open_response` reads it without any reference to the model that produced it.
- **Everything after the parse.** `extraction_open::run_open` resolves names to entities, builds described things, records names as facts, writes each statement as an open fact with its evidence located in the chunk, keeps time words verbatim, hangs qualifiers on the edge, and counts what it dropped and why. None of it knows where the reply came from.
- **A deterministic extractor for tables.** A table row is read as statements about its row's thing with the column heading as the phrase, with no model in the loop (#744).

What it withholds: a way in that skips the model. Every pushed byte is read as prose, chunked on a token budget sized for a model's attention (`BUDGET_TOKENS = 300`), and handed to a chat endpoint that must be configured before anything reaches the graph.

## Decisions

**1. The body is the contract.**

A `statements` source accepts the open extraction shape verbatim: `e`, `s` and `n` arrays with the same positions the model is asked to fill, wrapped in the same envelope `api` pushes use (`external_id`, `doc_time`, `deleted`). There is no second representation. A client writes what the extractor would have written; the parser that reads it is the parser that reads the model. The reason is 0032's and 0044's: a representation beside the one that runs is a second source of truth, and this one would drift the day the contract changed.

**2. The payload is the document, in one piece.**

The `{e, s, n}` object is stored as the document's content and as its single chunk, verbatim. Identity, versions, tombstones and the run history are exactly the `api` source's; the document appears in the Library under its source like any other. The chunker is not consulted: its budget exists so that a model reads a passage it can hold, and no model reads this.

**3. No model, the same path.**

For a document under a `statements` source, extraction parses the chunk instead of prompting for it, then continues unchanged: identity resolution, described things, name facts, evidence, time mentions, qualifiers, drop signals. The job runs whether or not a chat model is configured. A pushed statement is therefore an open statement in every respect a document's is, and reaches the typed graph the same way: through alignment (0044 cut 2), never before it.

**4. The item is its own evidence.**

A pushed statement carries no quote: the passage that states it is the item itself. Its evidence row names the chunk and the phrase and has null offsets, which is what `fact_evidence` already means by "the quote was not located" (0061). Names are recorded when they appear in the payload text, which for a well-formed payload is always.

**5. There is no slot for a type.**

The contract has positions for a phrase, a subject, an object or a value, qualifiers and time words. It has none for a property, a class or a predicate id, and this record adds none. A payload with keys outside the envelope and the contract is refused at the door, not silently ignored, so that a client cannot believe it wrote a typed fact. For the same reason a statement whose subject, or a name whose thing, is not listed in `e` is refused at the door too: extraction would drop it silently as an unknown reference, and the client would believe it landed. An object not listed in `e` is not refused; it lands as a literal value, as it does for a model's reply.

**6. Events, not state.**

A `statements` push says that something was the case at a time. A table that *is* the current state of a system belongs on a mount and is read at query time (0036); pushing its rows as statements would copy state into the ledger and then let the two drift. The guide says this in one sentence, because the first person to try will try with a table.

**7. An update marks, it does not close.**

A second push under the same `external_id` supersedes the earlier chunk. The statements that stood on it become stale under the existing rule (`documents::delete`'s comment: "没再提 ≠ 不成立"): they are handed to review, not invalidated. Closing an interval because a later observation contradicts it is the temporal engine's and alignment's job, on the typed layer, and this record does not reach into it. For the robotics case in #875 this is the honest answer: "the object was on the table" stays true of the earlier moment; what changes is what still holds now, and that is a typed question.

## API

```
POST /api/v1/sources/{source_id}/statements
Authorization: Bearer <the source's ingest token>
Content-Type: application/json
```

```json
{
  "external_id": "obs-000412",
  "doc_time": "2026-09-23T08:14:03Z",
  "e": [["cup-7", "cup", true], ["kitchen table", "table", true]],
  "s": [[null, "cup-7", "is on", "kitchen table", null, {}, "08:14:03", null]],
  "n": []
}
```

- `external_id` is required and is the identity (`statements:{external_id}`); a second push with new content updates in place and records a version; `deleted: true` tombstones it.
- `doc_time` is the observation's own time and lands on the world axis; push time is the record axis (0022). Without it the item is undated, as an upload is.
- Each `s` item is `[quote, subject, phrase, object, value, qualifiers, when, ended]`; `quote` must be `null`. Each `e` item is `[name, kind word, named]`; each `n` item is `[entity name, other name, quote]` with `quote` null.
- Keys other than `external_id`, `doc_time`, `deleted`, `e`, `s`, `n` are refused with 422, as is a subject or an `n` entity not listed in `e`. A body over 64 KiB or with more than 200 statements is refused with 422; those are cut-1 limits, not contracts.
- The response is the `api` push's: `{"action": "created" | "updated" | "unchanged" | "marked_missing"}`.

## Not doing

- A batch endpoint. Identity, versions and runs are per item; a client that has a hundred observations makes a hundred calls, as `api` clients do.
- A typed write, a `predicate_id`, a `class` field, or any promise that a pushed statement binds before alignment reads it.
- A table importer. Tables are 0036's.
- Synthesising a quote so that offsets are non-null. The item is the passage; an offset into it would say nothing.
- A confidence per item. Every statement enters at 1.0, as a document's do; a pushed observation with an uncertainty is a qualifier (`{"confidence": "0.72"}`) the way any document's hedge is, until a record decides otherwise.

## Phasing

1. This PR: the kind, the route, single-chunk storage, deterministic extraction, the Library entry with the token dialog, the guide section, tests for the door and for a pushed statement reaching the open graph with evidence.
2. After 0044 cut 2 lands: measure that pushed statements bind under the same signatures as extracted ones, on a corpus where the same events are both pushed and described in prose.

## Open questions

- Whether a statement with no offsets should look any different on a Review card. Today it does not.
- Whether `when` should accept an RFC 3339 instant directly rather than time words, once the `instant` precision on the roadmap exists (0045).
