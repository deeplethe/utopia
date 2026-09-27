# 0037 · A relation carries its own attributes

- **Status**: In progress · cuts 1 and 1b built (#598, #600) · open: entity-valued qualifiers, disagreeing mentions, the canvas label
- **Written**: 2026-09-10 (conventions in the [README](README.md))
- **Related**: [0031](0031-an-event-holds-at-the-moment-it-names.md) is what makes the same
  edge repeatable — an event's key includes its moment — and this record leans on it for
  identity. [0022](0022-an-unknown-date-is-not-an-open-one.md) put the reading of a row's
  shape in one place; this record adds a table beside the row and changes no shape.
  [0010](0010-no-relation-is-no-relation.md) keeps the wording of an unadopted relation on the
  evidence; a qualifier on such an edge is out of scope here. #586 and #587 stopped a written
  quantity from becoming an entity, which is what exposed the gap below.

> A document says *Meridian Partners invested $4 billion in Kestrel Dynamics*. The extractor
> has one shape for a fact — a subject, a predicate, and either an entity or a literal on the
> other side — so the sentence has to lose one half. It keeps the edge, `Meridian invested
> Kestrel`, and the four billion is nowhere. In the next document, *Vega Capital invested
> $5 billion in Northwind Robotics*, the model keeps the amount instead: `Northwind
> investment_amount $5 billion`, on the company, with no way back to which investment it was.
> Northwind has two five-billion mentions in the corpus; the graph cannot say which is which.
> Before #586 the amount had a third fate: it became a node called `$5 billion`, shared by
> every company that ever raised that sum.

## The problem

A fact row is `(subject, predicate, object | value)`. The check on the table is an `OR`, so
both columns may be set, but nothing writes both and every reader picks one. The amount on
an investment is not the object of the edge and not the value of an attribute of the
company. It is a property **of the edge**, and the ledger has no place for that.

Everything else an edge might need is already there. A fact has an id. Evidence, conflicts
and derivations hang off that id. The RDF export reifies every fact as an `rdf:Statement` and
hangs time, confidence, supersession and the quote off the statement node. The edge already
has the standing of a node in every sense but one: it cannot carry an attribute.

## Dead ends

**The amount as a second object.** Put `$4 billion` in `object_value` next to `object_id =
Kestrel`. The database allows it. But `object_value` means *the literal this attribute
predicate takes as its object*, and every consumer reads it that way — the export chooses
the entity and drops the literal, the tools print one or the other, the datatype for the
literal is read off the predicate, which for a relation has none. It also holds exactly one
figure with no name: is it the amount, the stake, the price? Half a day was spent on this
branch before the question "what is the key?" showed it was not a shortcut.

**The event as an entity.** Make `Investment #1` a node with `investor`, `target`, `amount`,
`date`. It needs no schema change and inherits entity resolution for free, which is why it
looked right for an hour. It also puts a node on the canvas that no one wants to look at —
the plan already included collapsing it back into an edge — and it answers a question nobody
asked. Standard vocabularies do reify events as classes (`schema:PublicationEvent`,
`prov:Generation`); that is a reason their object properties are all states (see
[0031](0031-an-event-holds-at-the-moment-it-names.md), 2026-09-10 note), not a reason to
copy the shape.

**A qualifier as a new kind of thing.** A parallel type system for edge attributes — its own
table of definitions, its own datatypes, its own normalisation. The existing attribute
definition already has a key, a label, a datatype, a unit, `normalize_attr_value`, and an
IRI for export. The only thing a qualifier definition lacks is a domain that is a relation
instead of a class.

## Decisions

1. **A relation declares which attributes its edges may carry.** `relation_type_qualifiers`
   links a relation to attribute definitions (rows of `relation_types` with
   `kind = 'attribute'`). Same base, attribute kind, not itself: checked in the store, since a
   `CHECK` cannot see another row. The prompt lists them after the relation —
   `invested_in (organization → organization) [event] {amount: number $}` — and the model is
   told to write them under `"qualifiers"` on the fact, keyed exactly as listed, never as a
   separate fact and never dropped to keep the edge.

2. **A qualifier value lives beside the fact, not in it.** `fact_qualifiers (fact_id,
   qualifier_type_id, value | entity_id)`, one row per attribute per edge, the value in the
   same `{"value", "unit"}` shape as `object_value`, converted by the attribute's datatype at
   write time (`$4 billion` → `4000000000` with unit `$`). The `facts` row does not change.
   Readers load the rows they always did and attach qualifiers by fact id afterwards; the
   `FromRow` structs carry the field with `#[sqlx(skip)]`.

3. **A qualifier is not part of the edge's identity.** The dedup key of a fact stays
   `(subject, predicate, object, moment)` — [0031](0031-an-event-holds-at-the-moment-it-names.md)
   already makes two investments at different moments two rows, and a dateless re-mention
   fold into the known one. A second mention that adds an amount adds it to the same row. A
   second mention that gives a *different* amount for the same row does not overwrite the
   first; in this cut it is recorded in the drop report as `qualifier_conflict`. The ledger's
   answer to two observations that disagree is two rows and a `fact_conflicts` entry; wiring
   that requires a write path that can bypass dedup on purpose, and is the next cut.

4. **An entity-valued qualifier is reserved, not built.** "A invested in B *through* C" needs a
   qualifier that points at a node. The table has the column and the `CHECK` that says one of
   value or entity, so the second migration is never needed; nothing writes it yet, and the
   prompt does not offer it.

5. **Export needs no new vocabulary.** The statement node already exists; each qualifier is
   one more triple on it, predicate = the attribute's IRI, object = the literal typed by the
   attribute's datatype. RDF 1.2's reifier is the same shape.

## What the test waves found (2026-09-11)

Four waves on an isolated base, each document pressing one rule (repeatability ×3, the
rules corpus declared and undeclared, a Chinese corpus declared and undeclared):

- **Repeatable.** Three runs of the original corpus, both investment edges carry their
  amount every time; the model's `stake: "minority"` is refused by the number datatype and
  lands in the drop report, not in the graph.
- **A qualifier the relation never declared, but the base already defines** (`amount`,
  `stake`, `round` exist as attributes) is now declared from the corpus and written, under
  the same `auto_extend_ontology` switch as the rest of the growth loop. The declaration is
  additive (`add_relation_qualifier`): documents extract in parallel and a replace-all write
  clobbered one document's declaration with another's.
- **Currency.** The model normalises `€30 million` and `15亿元人民币` to a bare number or
  writes the currency as a sibling key (`"currency": "CNY"`). The prompt now asks for the
  figure as written, the scanner reads ISO codes, currency words and CJK magnitudes (万, 亿),
  a sibling `currency` key becomes the unit, and the attribute's default unit is used only
  when the text carries no unit token at all — a wrong currency is worse than none. The same
  rule (`unit_for`) now governs an attribute written on an entity, which used to stamp the
  declared unit unconditionally: `500 兆瓦` filed under 金额 came out as ¥500.
- **Two mentions of one edge in parallel can both insert.** The dedup in `insert_fact_inner`
  is a read-then-write with no unique index behind it; two documents describing the same
  `(subject, predicate, object, moment)` extracted at the same time produced two rows with
  different amounts and no conflict. The next cut (two rows plus `fact_conflicts` for a
  disagreement) has to close this first — a per-base advisory lock around the insert, or a
  partial unique index on live rows.
- **A superseding row carried its evidence but not its qualifiers.** `close_superseded`,
  `close_with_unknown_end`, `correct_interval` and the refinement path in `insert_fact_inner`
  all predate this record; closing a 董事 span on the day of the resignation produced a live
  row without its 职务. All four now copy `fact_qualifiers` the way `adopt` does (the
  2026-09-11 litigation rehearsal, with [0022](0022-an-unknown-date-is-not-an-open-one.md)'s
  fourth cut).
- The model dates "earlier this year" to a concrete day and so mints a moment the text never
  gave; identity follows the model's date. Visible on the timeline, not a qualifier defect.

## Open questions

- ~~Where does the amount go when the relation is not adopted yet?~~ Answered: a qualifier
  on a fact whose predicate is unknown binds to an attribute the base already defines (no
  ontology change, so no switch), and adoption (`adopt`) carries the qualifiers onto the new
  row and declares them on the relation. A qualifier that binds to nothing — the base has no
  such attribute, or the ontology is frozen — is written as a literal fact on the subject,
  worded `relation.key` on its evidence and recorded in `ontology_misses`, the shape rule 8a
  gives an unlisted figure. Nothing about an edge is dropped for want of a definition.
- The canvas. An edge label with the amount is a rendering change and belongs with the
  parallel-edge work; the timeline reading an event as a point is
  [0031](0031-an-event-holds-at-the-moment-it-names.md)'s UI cut.
- Whether a qualifier should be offered on a relation the ontology packs bring in. Their
  relations are states pointing at reified event nodes, so the natural home of an amount in
  schema.org is the event class, not the edge. No pack relation declares a qualifier by
  default.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> cut 1 merged (#598) · cut 1b (units, auto-declaration, sibling currency; #600) · `relation_type_qualifiers` and `fact_qualifiers` (migration 0049), a relation declares its qualifiers, extraction writes them, the panel and the export read them · not in this cut: an entity-valued qualifier (the column is reserved, nothing writes it), a second row plus a conflict when two mentions of one edge disagree (today the first value stays and the disagreement goes to the drop report), the canvas label, the bootstrap proposing qualifiers for a relation it adopts
