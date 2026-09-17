# The ontology is a view over what documents say

The ontology is the vocabulary a base answers questions in: classes, relations, attributes and their
declarations. This file says where that vocabulary comes from, how it is governed, and how it binds
to what documents say. Records: [0001], [0003], [0006], [0007], [0008], [0009], [0012], [0036],
[0037], [0044]. The review side of proposals is in [governance](governance.md), the schema side of
mappings in [lakehouse-and-actions](lakehouse-and-actions.md), the extraction contract in
[extraction](extraction.md).

## What it does today

**Three layers, and "ontology" names one of them.** The open graph holds what documents say:
entities with their names and type words, statements whose relation is the document's phrase,
qualifiers, time mentions and quotes. The ontology holds object types, link types and properties,
each with a definition, examples and regression cases, plus constraints, versioned on the recorded
axis. The typed graph holds facts expressed in the ontology, computed from the open graph and marked
with the ontology version it was computed under [0044]. Since 2026-09-16 extraction writes the open
graph and nothing else (#731, #736); the ontology enters no extraction prompt; typed facts will come
only from alignment (0044 cut 2, not built). A base extracted today holds open statements and name
facts and no typed facts, so signature checks, timelines and proposals have nothing to read until
alignment lands.

**The tables.** `entity_types` and `relation_types` (`kind` is relation or attribute),
`entity_type_parents` for multiple inheritance with one primary parent for the tree view,
`relation_type_domains` / `relation_type_ranges` checked through the subclass DAG,
`entity_type_disjoint`, `inverseOf` and `subPropertyOf`, the `functional` and `inverse_functional`
flags, `temporal` (state, event, eternal) on every relation, `datatype` and `unit` on attributes,
and `relation_type_qualifiers` naming the attributes an edge of that relation may carry
[0001, 0031, 0037]. A class or relation imported from a vocabulary keeps its IRI (`UNIQUE (kb_id,
iri)`) and the export writes that IRI back; `key` (`[a-z0-9_]`, at most 40 characters) is the token
the API uses, unique per base [0001, 0020].

**Where a vocabulary comes from.** A new base is empty: no classes, no relations, and no pack unless
the person ticks one in the creation dialog [0008, 0009]. Five packs ship inside the binary
(schema.org, W3C Org, PROV-O, FOAF, IOF Core), multi-select, reconciled by a static alignment table
of about twenty rows, additive and without undo [0008]. An OWL file (Turtle or RDF/XML) is stored
verbatim in the blob store and projected into the tables by a re-runnable derivation that reports
what it did not project; a dry run precedes every import; individuals are never imported as facts
[0001]. The rest is typed on the ontology page or adopted from a proposal. The self-check of [0002]
runs after an import: eight defect kinds (a relation both symmetric and asymmetric, a subclass cycle,
an inverse that does not point back) are shown before any fact-level check, and a base without axioms
reports zero.

**A type on an entity is optional and a person's decision is final.** `entities.type_id` is
nullable; NULL means undecided, and `type_source` (extracted, human, inferred) keeps a person's
retype, including "no type", out of every automatic path [0009, 0001]. Same-name entities may coexist
across types and untyped [0009]. Type resolution (candidates from class descriptions and from the
classes of context neighbours, interleaved and never scored together; a class inside the current
subtree applied on its own, a cross-axis class sent to a person, each pair acknowledged once) runs
from the ontology page; extraction no longer enqueues it [0001, 0016, #736].

**A relation phrase binds to a property per signature** [0044 cut 2, #751]. A signature is a
phrase as the documents wrote it, the class of its subject and the class of its object, or "value"
when the object is a figure, a title or a status; the classes come from the kind-word bindings, and
a side whose kind word is bound to no class is its own signature. Each signature is decided once,
after the kind words, by the same shape as [#741]: candidates are the properties whose declared
domain and range admit the two ends in either direction (an undeclared end admits anything), the
model sees the signature with three of its statements and their quotes, and two votes with the
candidates in opposite orders must agree on the property and the direction (forward when the
statement's subject is the property's subject, reverse when its object is) for the signature to
bind. A signature the votes disagree on is `undecided` for the alignment queue of #725; one with no
fitting property is `none`, its statements stay in the open graph and it counts toward the
workbench's suggestions. Bindings live in `phrase_bindings` and go stale when the property they
bound to changes or a property is added; a person's decision is never overwritten by the agent. On
the 25-document batch with a hand-written ontology of 14 classes and 28 properties, and 60 of
400 kind words bound, 423 signatures cover 861 statements: 36 bind (184 statements), 110 bind to
nothing, 1 splits the votes and 276 have no admissible property because an end is unbound; a
judge reading the chunk finds 95% of the resulting typed facts stated by the document, 4% worded
wrongly and 1% not stated. Offering every property to an unbound end raised coverage to 85
signatures and dropped the judge to 87%, and on the NVDA releases, where 15 of 230 kind words
bind, to 59%: the generic "value" attribute swallowed every cash-flow row. Coverage therefore
follows the kind-word bindings and the ontology's size, not the binder. What still goes wrong is
a phrase that carries part of the value ("下降 1.4%" bound to a change property loses its sign)
and a table section read as a change ("changes in operating assets › accounts payable").

**A bound statement is a typed fact** [0044 cut 2, #PRN2]. The typed graph is computed, never
written by hand: after each phrase-alignment run, every live open statement under a bound signature
becomes one `layer = 'typed'` row whose predicate is the bound property, whose ends follow the
binding's direction, and whose value, world-axis interval, attestation and confidence are copied
from the statement; its evidence rows and role-word qualifiers are copied too, and
`from_statement_id` points back at the statement. A statement with a `mood` qualifier is never
materialised. The computation is a set operation and idempotent: rows whose source no longer holds
(the statement invalidated, the signature no longer bound, the property or direction changed) are
invalidated, rows that are due and missing are added, rows whose binding is unchanged keep their
id, evidence and recorded time. Typed rows that several statements produce for the same triple are
separate rows, one per statement. On the 25-document batch the 36 bound signatures give 184
typed rows.

**Argument order is enforced, participation is guided.** A declared domain or range shapes
candidates and never discards a fact; argument order is the key's encoding convention, so a fact
whose subject violates the domain while the object fits is swapped by the signature and marked
`direction_corrected` [0012]. The write-time swap left with the typed extraction path; what remains
is `ontology::judge_direction` on adoption (a predicate that fits neither way is left off) and on
merge (moved facts are re-checked and reported as `signature` violations) [0012, #736]. Under
alignment a signature's direction is decided once per signature [0044].

**Growth from the corpus.** Predicate words that matched nothing accumulate as proposals with their
counts; adoption is decided by counting (`MIN_DOCS = 2`, and `MIN_SIGNALS = 3` for an LLM run),
grouped by inflectional base, named by the most frequent phrasing, folded onto an existing
equivalent (`_by` inverses included) with subject and object swapped; the waiting facts are
rewritten as superseding rows, undoable per batch; dismissal is remembered and counting goes on; no
axiom is ever set automatically [0003, 0007]. The loop reads typed rows only, so an open phrase is
never auto-adopted, and with the typed path gone it receives no new input until alignment [#731,
#736]. `Metric` and `Dimension` are to retire: exploration inserts no class without a proposal
[0036, 0009].

**Language.** `label` is data and follows no switch; `key` is never translated; `description` follows
`knowledge_bases.ontology_lang` because it was the line the model read [0004]. With no ontology in
the prompt, a description is read by people and by the aligner.

## Why

- **Keep the original, project what has a consumer.** Discarding is irreversible; a projection can
  be redone when a consumer appears, so "not expressible" becomes "not yet projected" [0001].
- **IRI is identity, key is the token.** A key derived from a label breaks on relabel and collides
  across vocabularies; an IRI is a name and never an address [0001].
- **A declaration can be wrong** (`part_of` marked functional produced 59 false conflicts), so types
  guide and never gate; order is not a claim about the world, so it is enforced, measured 57% to 4%
  violations with reversals 39 to 0 [0001, 0012].
- **A placeholder in the ontology is control flow written as vocabulary.** `concept`, `related_to`,
  `mapped_to` and then `Metric` / `Dimension` were removed on the same argument; an undecided thing
  stays empty and readable [0009, 0010, 0011, 0036].
- **Packs give direction through structure**, which prose cannot; but a pack gives types, not
  predicates (81% of facts unbound before bootstrap), costs ten points of type accuracy to omit and
  buys an ontology in the corpus's own words, so none is the default [0008].
- **Counting decides adoption** because the model skipped `runs_on` (8 documents) and adopted a
  one-document verb, and a deterministic ontology makes the bench comparable; the model keeps only
  synonym merging, and merges stay human [0007, 0003].
- **Knowledge vetoes deleting ontology**: a type with entities cannot be deleted, so an import has
  no undo, only a bulk view of empty classes (not built) [0008].
- **A qualifier definition is an attribute definition** with a relation as its domain; a parallel
  type system was refused [0037].
- **The ontology is a view.** Facts bound at write time were induced by the property list (an award
  written as a genre): 4 to 9% not stated against 0 of 333 open statements; binding after the fact
  per signature costs distinct phrasings, not documents, and a changed ontology recomputes only the
  affected signatures instead of re-extracting [0044].
- **Standard vocabularies reify events as classes**, so every pack object property is a state; a
  person changes `temporal` on the page and reads pick it up at once [0031].

## Proposed and not built

- **Alignment** (0044 cut 2), the rest: implication rules proposed by the aligner, approved on
  the workbench, executed by code with cached readings (the sign of "下降 1.4%" is such a
  reading); a signature that tells a figure from words on the value side; merging the typed rows
  that several statements produce for one triple, with the timeline's refinement rules. The prototype aligner reached 14.7% and
  12.1% of gold recall in two runs against 15.5% for the withdrawn bound pass, so the bar for cut 2
  is parity over two clean runs [0044, #729].
- **The workbench** (0044 cut 5): the ontology page fed by suggestions from the open graph (frequent
  unbound signatures, type words in use, an ontology agent reading them against competency
  questions), by imported files, and by editing; every approved element carries regression cases;
  duplicate properties merged as governance.
- **Exploration proposes an alignment per table** through `ontology_proposals`, adopted as one
  thing; `Metric` / `Dimension` retire (#554, #556) [0036].
- Filtering reified-shell relations (`Action`, `Offer`) out of pack import [0012]; Chinese
  descriptions generated per `ontology_lang` [0016 C3]; a bulk view of an import's empty classes
  [0008].

## Open questions

- What becomes of `ontology_misses`, predicate adoption, type resolution and the bootstrap job once
  alignment lands: #736 leaves them idle; #725 proposes retiring the type machinery and adoption
  from `proposed_predicate` in favour of the ontology agent.
- Competency questions for a base that has none; when the typed graph is materialised eagerly
  [0044].
- Mixed-pack accuracy, Chinese labels on an English pack, and what a 1,500-term start does to
  adoption thresholds never retuned [0008, 0007]; narrative verbs and `_by` folding [0007].
