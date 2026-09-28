# 0031 · An event holds at the moment it names

- **Status**: Implemented (#486, #593) · open: the UI cut
- **Written**: 2026-09-08 (conventions in the [README](README.md))
- **Related**: [0003](0003-ontology-growth-loop.md)'s graph migration gave a relation three temporal semantics and gave the engine one. [0022](0022-an-unknown-date-is-not-an-open-one.md) put the world-axis read in one place; this record adds two branches there and nowhere else. [0024](0024-the-world-axis-reaches-the-second.md)'s ladder is what "the bucket it names" is measured on. [0013](0013-a-source-should-hand-over-its-history.md) hands ticket events over at day precision; those were the first rows this rule was wrong for. From #486.

> `relation_types.temporal` has been `state`, `event` or `eternal` since the first graph migration, and the ontology page offers all three — "State (has interval)", "Event (point in time)", "Eternal (timeless)". Only `state` did anything. "Nova acquired Orion on 2024-03-15" landed as *holds from 2024-03-15, still going on*; "Paris is the capital of France" picked up whatever year sat in the sentence and, having no start, held from its first evidence and not before. The ledger promised a moment and a timeless fact, and wrote and read both as intervals.

## What the ledger did

Three consumers read the column, and each read one value of it.

- **The temporal engine** reconciles only `state` (`reconcile_new_fact`, `reconcile_predicate`). Right — a moment is not closed by a later moment — but that was the whole of it.
- **The world-axis reads** (`world_axis`, the evaluator's `read_span`) treated every row as an interval: a NULL end read as *still holds* (0022 keeps that for states, on purpose), and a NULL start read as *from the first evidence*. An event with a date therefore held forever after it; an eternal fact held from the day of the document that stated it.
- **The prompt** never mentioned it. The model filled `valid_from` / `valid_to` for an event the way it does for a state, and for an eternal relation copied whatever date the sentence carried.
- **The panel** hid dates on `eternal` and nothing else.

So choosing `event` was the same as choosing `state` without a uniqueness axiom, and choosing `eternal` was a display hint.

## Decisions

### 1. An event is one moment, written at both ends

`Validity::under(temporal)` runs on every write, keyed on the predicate's declared semantics. For an event: the start if the text gave one, else the end (only an end means *that is when it happened*), and a span collapses to its start — an acquisition does not run until next year. Both ends get that value and that precision. No date means no dates; "ended, date unknown" means nothing for a moment and is dropped, so an event row never carries `'unknown'` and never gets an `attested_to`.

The rule sits in `insert_fact_inner` and `correct_interval`, not in the writers. Extraction, the pending-facts nod (0015), a person's own fact from the API and a person's edit of an interval all pass through those two doors, and nothing outside the store has to know the rule exists.

2026-09-27: under a property declared as a state, the same rule now refuses a row whose two ends are equal, and a single date bound to a state is read by what its binding says it marks. See [0053](0053-a-phrase-decision-records-the-inputs-it-considered.md)'s revision of that date (#966).

Both ends carry the value because that is the one shape the row can explain on its own. A reader that does not consult the predicate — an old client, an export consumer, a hand-written query — sees a fact that held on one day. It cannot see *holds from that day on*, which is what an open end would have told it. The representation fails safe.

An undated observation of an event merges into an existing dated one, the mirror of the refinement path that already lets a dated observation supersede a bare row: "acquired in March" followed by "acquired" is one acquisition, not two.

### 2. An event holds through the bucket it names, and an undated event at no moment

A day-precision event on 2024-03-15 holds at every instant of 2024-03-15 and at no other: `[valid_from, valid_to + one unit of its precision)`. The precision already says "the day is known, the instant is not" (0024); reading the row as holding through the day is what that sentence means. A month-precision event holds through the month.

An event with no date holds at no moment. The read interval is `[attested_from, attested_from)`, empty. This is 0022's rule for the unknown, applied to a moment instead of a bound: a state with no start holds from its first evidence because the document asserts it as current; a document that says "Nova acquired Orion" asserts that it happened, not that it is happening. The row stays on the entity's panel under "undated", and a chain of derivations through it derives nothing, because `overlap` refuses an empty intersection.

The read lives in `world_axis::facts_holds_to` beside the two rules 0022 put there, and `read_span` mirrors it for the evaluator. The predicate's semantics come from a correlated subquery on `relation_types` — a row cannot say what it is, but its predicate can — and a fact with no predicate (0010) reads as a state, the one value that loses nothing, the same judgment the ontology import makes when it writes `state` for every OWL property.

### 3. An eternal fact has no dates and every moment

Under `Eternal`, `Validity::under` clears both ends whatever the text said. A date in a sentence about the capital of France is about something else — when the document was written, when the author learned it — not about when the relation held. And the read opens both ends: `facts_holds_from` is NULL for an eternal row, so the evidence anchor no longer gates it. 0022's anchor answers "since when do we have grounds to say this holds"; for a fact that holds at every moment the question has no bearing on the answer.

### 4. The prompt says what to write, and only when it matters

A relation in the list gets ` [event]` or ` [eternal]` after its signature, and one rule (3b) says what those marks mean for `valid_from` and `valid_to`: an event's date goes in `valid_from` and `valid_to` stays null; an eternal relation gets no dates. A base whose relations are all states sees neither the marks nor the rule — the same discipline as the type-signature note, which also costs nothing to a base that declared none. The instruction is about what to *write*, not what the relation *is*: told only that a relation is a point in time, a model still fills a start the way it does for a state, and the write rule then collapses a span it never needed to produce.

### 5. Rows written before this are read, not rewritten

An event row from before this record has a start and an open end. `facts_holds_to` reads it as its start's bucket — the end falls back to the start, the precision to the start's — and `read_span` does the same. No migration touches the ledger: the rows are what they were, and the new reading is right for them. A base that wants the stored shape can re-extract.

## Dead ends

- **Store the bucket end.** `valid_to = valid_from + one unit` with the existing read predicate unchanged. Nothing to add to `world_axis` — and an auditor reading the export (0020) sees `validFrom 2024-03-15; validThrough 2024-03-16`, a one-day state, for a fact the text placed at a moment. The row would say something the source did not.
- **Leave the end NULL and let the read handle it.** The smallest write. It is also the shape that reads as *still holds* in every consumer that does not check the predicate — the one that was wrong before this record. A representation that is only correct through the predicate is one forgotten join from being wrong again.
- **A read rule that ignores the predicate**: "a row whose two ends are equal holds through that bucket", for every row. It would give events the right reading without the subquery. It would also change the meaning of a state row that starts and ends in one bucket, and it sits beside a convention this record does not touch: a state ending "2024-07" has always read as ended at the start of July, not the end. Changing that is a different record, about states.
- **An `attested_from = -infinity` sentinel for eternal rows.** One column, no subquery — and a magic value in a column whose meaning (0022) is "the earliest evidence", which `attest_earlier` and any future "attested" display would have to know to skip.
- **A migration that rewrites old event rows.** The read is correct for them as they are (§5), and rewriting a row's stated interval to match a rule it predates is the ledger editing history.

## Open questions

- **A point on the panel.** `fmtInterval` prints an event as `2024-03-15 ~ 2024-03-15` and the timeline draws it as a bar of no length. The point rendering, and a line on the ontology page saying what the three values do, are the UI cut.
- **The wording the tools give the model.** `time_text` phrases an event's interval the way it phrases a state's. Whether "on 2024-03-15" reads better than "from 2024-03-15 to 2024-03-15" for a timed answer is a prompt question, to be looked at with the tool traces.
- **An attribute taken at a moment.** Attributes are created as `state` and the UI does not offer otherwise (0021's readings are states that a later reading closes). A reading that is a measurement *at* a time rather than a value *from* a time is not expressible today; nothing has asked for it.
- **The end convention for states.** Noted above. "Until 2024-07" ends at the start of July under the current read; the bucket reading this record gives events would end it at the start of August. Left alone here. 2026-09-27: one case is settled in [0053](0053-a-phrase-decision-records-the-inputs-it-considered.md)'s revision of that date. An ending that closes an open row starting inside the period it names closes it at the period's end.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> implemented (#486) · `Validity::under` normalises every write by the predicate's `temporal` — in `insert_fact_inner`, so extraction, the nod and a person's own fact all pass through it, and in `correct_interval` · `world_axis` reads an event as the bucket it names and an eternal fact as open at both ends, and the evaluator's `read_span` says the same · the prompt marks `[event]` and `[eternal]` relations and tells the model what to write, for a base that has any · the export carries `utopia:temporal` on a property that is not a state · no schema change, and a row written before this reads correctly · the panel still prints an event as `T ~ T` and the ontology page still does not say what the three values do — that is the UI cut · 2026-09-10: corpus-grown relations now take `temporal` from the proposal instead of `state` for all (#593, on [0007](0007-who-decides-what-becomes-a-relation.md)) · the five packs were measured for the same gap and there is none to fill: of 946 schema.org object properties, 60 in PROV-O, 32 in ORG and 73 in IOF-core, **no object property is an event** — standard vocabularies reify an event as a class (`PublicationEvent`, `prov:Generation`) and their object properties are states pointing at it, so `create_relation_types_bulk` writing `state` is right by construction, not by omission; IOF's `…AtAllTimes` family is the one `eternal` candidate and is left as `state` until a base needs it; a person changes any of them on the ontology page and the read side picks it up at once, since `world_axis` looks the predicate's `temporal` up at read time
