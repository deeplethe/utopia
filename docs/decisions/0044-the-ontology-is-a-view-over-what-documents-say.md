# 0044 · The ontology is a view over what documents say

- **Status**: Proposed 2026-09-16 · replaces the staged-reading draft of this record (skim card, graded mentions, statements bound at write time), which the prototype below did not bear out · nothing built in the product · prototype scripts and measurements from 2026-09-15 are summarised in [What the prototype measured](#what-the-prototype-measured)
- **Written**: 2026-09-16 (conventions in the [README](README.md))
- **Related**: [0022](0022-an-unknown-date-is-not-an-open-one.md) put a document's date in `attested_at` beside the world and record axes; [0025](0025-governance-reads-the-ledger-before-it-decides.md) and [0027](0027-an-automatic-merge-is-gated-by-what-it-can-undo.md) put agent decisions through a gate that weighs what they can undo; [0041](0041-a-name-is-a-claim-about-an-entity.md) made names facts and identity a matter of evidence; [0043](0043-every-review-queue-is-governed.md) sent every review queue through the governor; #714 found upload time used as the document date in extraction.

> The introduction to *The Eminem Show* says the album won the Grammy for Best Rap Album. Given the knowledge base's properties, the extractor records that the album's genre is Best Rap Album, and that it received Best Rap Album as an award. Asked to write what the introduction says in its own words, it records that the album won Best Rap Album.

## Context

Today extraction binds each fact to a property of the knowledge base's ontology at the moment it is written. The ontology arrives in the prompt, whole or trimmed, and a fact that finds no property is kept as an unbound proposal. The audit recorded in the first draft of this record found 59% of facts unbound on the timeline corpus, 1,093 distinct predicates, and a fifth of entities named by descriptions. The first draft answered with more stages around the same act of binding. A prototype built on 2026-09-15 tested the act of binding itself, and found that the errors live there.

## What the prototype measured

Setup: DeepSeek-V4-Flash with thinking off for extraction and agents; DeepSeek-V4-Pro as judge, calibrated against 24 hand-labelled facts (it agreed on 21: 8 of 9 correct facts accepted, 2 of 15 wrong ones passed). Corpora: 100 documents sampled from the Re-DocRED test set (Wikipedia introductions, 3,462 gold triples, 95 Wikidata properties), and the FDA, statistics-bulletin and SEC-filing corpora of `scripts/bench/recall.mjs`. Counts are single runs unless stated; runs of the same configuration varied by about two F1 points.

**1. Statements written in the document's own words are faithful.** Of 333 open statements from two extraction prompts, the judge found none that the document does not state, and 2% worded wrongly.

| On 10 Re-DocRED documents | Statements | Stated | Stated but worded wrongly | Not stated |
|---|---|---|---|---|
| Open statements, full extraction prompt | 192 | 97.9% | 2.1% | 0 |
| Open statements, compact one-call prompt | 141 | 98.6% | 1.4% | 0 |
| Facts bound at write time, property names only | 78 | 74.4% | 16.7% | 9.0% |
| Facts bound at write time, with definitions | 114 | 79.8% | 15.8% | 4.4% |

The bound facts that are not stated are induced by the list of properties: an award written as a genre, a county given a notable work, a seventeenth-century king given a modern citizenship.

**2. Binding after the fact loses more than it keeps.** Aligning open statements to an existing ontology bound 4 of 113 to 567 phrase groups per corpus against the imported schema.org properties (FDA, statistics, SEC), and scored F1 9.1 on Re-DocRED against its own 95 properties, where extracting with the properties in the prompt scored 21.8. The loss comes from implicit facts ("a 1952 British film" has a country of origin) that the open statements do not state as statements.

**3. Gold overlap understates precision.** Re-DocRED still omits many true facts. Strict precision against gold was 37.8% for the sliced extraction; the calibrated judge accepted 76.1%. A hand review of 30 unmatched facts found 9 correct, 5 with a near-synonymous property, 6 debatable and 10 wrong. Comparisons below hold under all three measures.

**4. A large ontology can be sliced per document, if its structure is complete.** With the 95 benchmark properties hidden among 872 schema.org properties, each document got a slice of about 133 properties that contained 86.6% of the relations its gold triples use. The slice was the union of properties whose domain and range meet the classes of the document's entities (34.0% alone), properties nearest to its statements' phrases (28.9% alone), and every property with no declared domain (the most frequent ones, country and located-in, have none). Without the class hierarchy and the Wikidata–schema.org equivalences, the structural part found 10%. Extraction with the slice scored F1 18.3 against 21.8 with the 95 properties alone; merging the 49 schema.org properties Wikidata declares equivalent to benchmark properties raised it to 20.2 and restored precision (34.3% → 37.8%, judged 71.2% → 75.3%).

**5. Open and bound extraction complement each other when they run independently.** Unioning the open statements with the bound facts linked 46.8% of gold entity pairs (open alone 38.4%, bound alone 34.5%) with F1 22.5. Giving the bound pass the open statements as context lowered recall to 42.9%.

**6. An agent can correct after extraction.** An errata agent reviewed all facts of 99 documents with tools to read the document, look up a property's definition, constraints, broader properties and inverse, and retract, revise or add. Judged precision rose from 76.1% to 89.5% and strict precision from 37.8% to 47.1%; strict recall moved from 13.8% to 13.6%. It retracted 278 facts, revised 42 and added 49; the judge's count of correct facts fell from 871 to 814, so about a quarter of what it removed was right. Agent-written usage notes learned from half the documents and given to extraction on the other half helped no more than a generic caution of the same length.

**7. Cost.** For a 165-word document the full prototype spent about 23,000 prompt and 7,200 completion tokens. A compact pipeline (entity scan, cached type mapping, one extraction call, errata only for flagged facts) spent 3,449 and 753, at F1 23.5 against 28.4 for the full prototype with errata on the same 10 documents. Mapping entity type words to classes is paid once per distinct word across a knowledge base: the second run over the same documents met 5 new words instead of 54.

**8. Operational ontology platforms do not induce ontologies from text.** Their object types, links and actions are designed per use case by builders and backed by curated datasets; language models extract into a table a builder has already shaped, and edits are replayed over the backing data.

## Decisions

### 1. Three layers

- **The open graph** holds what documents say: entities with their names and type words, statements whose relation is the document's phrase, with their participants, qualifiers, time mentions and quotes. It needs no ontology and is kept whatever the ontology becomes.
- **The ontology** holds object types, link types and properties, each with a definition, examples and regression cases, plus constraints and actions. It is small, shaped by the questions the knowledge base must answer, and versioned on the recorded axis.
- **The typed graph** holds facts expressed in the ontology. It is computed from the open graph and marked with the ontology version it was computed under.

The word ontology in this project means the second layer only.

### 2. Extraction writes the open graph and nothing else

One call per chunk, a compact contract (arrays, short keys, no repeated names), no ontology in the prompt. The extractor reports entities with a type word, statements in the text's words, figures and titles as attributes of the thing they describe, time mentions as they are written, and the other names a document gives. Code checks that names and quotes occur in the chunk.

### 3. The typed graph is materialised from the open graph

A binding is decided once for a signature: the phrase, the subject's classes and the object's classes. The decision (a property and a direction, or none) is cached with its confidence and the ontology version, and reused wherever the signature recurs, so its cost grows with the number of distinct phrasings rather than documents. Facts a reader draws without the text stating them (a place's country from its region, a film's country from its nationality adjective) come from derivation rules over the typed graph and are marked derived. When the ontology changes, only facts under changed signatures are recomputed. A property missing from the ontology does not stop the statement: it stays in the open graph and becomes evidence for a proposal.

### 4. The ontology is proposed by an agent and approved by people

An ontology agent reads the open graph and a set of competency questions, and proposes object types, link types and properties with definitions, examples and the signatures they would bind. People approve through actions. Each approved element carries regression cases drawn from the open graph; changing a definition reruns them. The ontology is judged by whether the competency questions can be answered correctly. Structure the slice depends on (class hierarchy, equivalences, domains, ranges) is part of approval, and duplicate properties are merged as part of governance.

### 5. Time is resolved the way identity is

- A time mention is a fact with its quote, attached to the statements it dates.
- Extraction carries a document time context from chunk to chunk: the document's own date, taken from its content or a source that dates it (never the time it was uploaded, #714), the anchors its narrative sets, and the calendars it defines (a fiscal year, a reporting period).
- The model returns an interpretation, not a date: absolute, or an anchor with an offset; a granularity; a point, an interval or an as-of. Code computes the interval.
- A mention whose anchor is unknown keeps its words and waits; a later anchor, in the document or in another, triggers recomputation.
- Three times stay apart: when a statement holds (valid time), when the document observed it (its own date), and when the ledger learned it (recorded time).

### 6. Identity across documents is decided on evidence

Every document's entities, merged within the document on name and stated aliases, are profiled by names, classes, attributes, neighbours and time span. Candidates come from name facts and name vectors across scripts. Deterministic evidence scores each candidate pair and settles what it can: agreeing names and neighbours raise the score, and a conflicting type, attribute or lifespan is a cannot-link. Only undecided pairs go to the adjudicator, which sees two profiles rather than two documents. Clustering respects cannot-links, so A≈B and B≈C never merge A and C against evidence. New evidence re-evaluates earlier merges, and a merge or a split is an action that can be reverted. A rename keeps one entity with names valid at different times; a role such as a company's chief executive is not an entity.

### 7. An errata agent reviews the typed graph after extraction

It reviews facts flagged by structure (a subject or object outside the property's declared kinds, a name not found in the document, a date property without a date) before sampling the rest, checks every fact it is given before adding any, works through a JSON action protocol with a per-document budget, and records each retraction or revision as an action with the document's words as evidence. Its measure is precision gained against correct facts removed.

### 8. Lexicons remain governed data

Alias tables, bindings, cannot-link lists and gazetteers are data with provenance, reviewed like facts, never lists in code.

## Not doing

- **Binding to the ontology at write time.** It produced the extraction errors measured above and ties every document to one version of the ontology.
- **Putting a general ontology in the prompt.** A knowledge base's ontology grows with use; the slice depends on the document, and binding happens on signatures.
- **Learning usage notes from part of a corpus to re-extract the rest.** Measured no better than a generic caution.
- **Dates computed by the model, or upload time used as a document date.**
- **Word lists in code** for names, suffixes, relation shapes or time expressions.

## Measurement

Every cut reports on at least three domains, twice per configuration, with the judge's calibration stated.

| What | Bench | Numbers |
|---|---|---|
| Open graph | Re-DocRED (100 documents) and the FDA, statistics and SEC corpora | statements not stated and worded wrongly (judge), entity-pair recall, tokens per document |
| Typed graph | Re-DocRED with its 95 properties as the approved ontology | strict, structural and judged precision, recall, F1; cost per document as the corpus grows |
| Ontology | FDA and statistics corpora with written competency questions | questions answered correctly; share of proposals people change |
| Time | `temporal.mjs`, the lease bench, SEC fiscal periods, statistics bulletins | normalised value and granularity, statement valid time, as-of answers |
| Identity across documents | `identity.mjs`; Linked-Re-DocRED documents that share non-location entities or namesakes of different types (GPL-3.0, kept outside the repository) | pairwise precision, recall, F1; wrong merges and missed merges apart; adjudicator calls per thousand entities |
| Errata | the typed-graph benches | precision gained, correct facts removed, tokens |

Thresholds to pass before a cut lands: the open graph at or under 2% not-stated with entity-pair recall no lower than today's; the typed graph at or above the full prototype's F1 on the same Re-DocRED documents (22.5 on the 100-document sample before errata) at under a fifth of its tokens per document.

## Cuts

1. The open graph in the ledger: statements, time mentions and names with provenance on both clocks; the compact extraction contract behind a flag.
2. Signature bindings: a table of signature → property, direction, confidence, ontology version; materialisation and recomputation of the typed graph.
3. Time context and code resolution of time mentions (closes #714).
4. Identity evidence: profiles, deterministic scoring, cannot-links, constrained clustering; name vectors from 0041 cut 2 (its migration renumbered from 0060).
5. The ontology agent and competency questions; regression cases on definitions.
6. The errata agent on the typed graph through the gate.

Today's extractor stays as it is until cut 2 passes its thresholds.

## Open questions

- How queries read a typed graph that is partly materialised, and when materialisation is eager.
- Whether derivation rules recover the implicit facts that write-time binding found (Re-DocRED's country and located-in relations are a fifth of its pairs).
- Competency questions for a new knowledge base that has none yet.
- How much of identity the deterministic evidence settles before the adjudicator is needed.
