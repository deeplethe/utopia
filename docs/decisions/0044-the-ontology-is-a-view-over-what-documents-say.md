# 0044 · The ontology is a view over what documents say

- **Status**: Accepted 2026-09-17 · cut 1 built: extraction writes open statements (#731), memory documents take the same path (#735), the typed path is deleted (#736), kind words bind to classes with two votes (#741), the contract's rules for things, phrases, tables and moods (#743, #744, #745) · cut 2 in progress: bindings (#751), materialisation (0067, 0068), the human decision with its job (0051, #876), the decision basis (0053, #878), **implication rules with cached readings (migration 0073, PR #882)** — the parity run against the bound pass waits for the typed-graph bench (#880) to be run with a model; identity profiles (cut 4) in progress in a separate track; **the errata agent (cut 6) built: structural flags, a JSON action protocol with a per-document budget, actions through the 0027 gate (migration 0074; PR number added at merge)** — its measure (precision gained against correct facts removed, tokens) is the `--errata` run of the typed-graph bench, not yet run with a model · current state in [design/extraction](../design/extraction.md) and [design/ontology](../design/ontology.md) · replaces the staged-reading draft of this record (skim card, graded mentions, statements bound at write time), which the prototype below did not bear out · prototype scripts and measurements from 2026-09-15 are summarised in [What the prototype measured](#what-the-prototype-measured)
- **Written**: 2026-09-16 (conventions in the [README](README.md))
- **Related**: [0022](0022-an-unknown-date-is-not-an-open-one.md) put a document's date in `attested_at` beside the world and record axes; [0025](0025-governance-reads-the-ledger-before-it-decides.md) and [0027](0027-an-automatic-merge-is-gated-by-what-it-can-undo.md) put agent decisions through a gate that weighs what they can undo; [0041](0041-a-name-is-a-claim-about-an-entity.md) made names facts and identity a matter of evidence; [0043](0043-every-review-queue-is-governed.md) sent every review queue through the governor; #714 found upload time used as the document date in extraction.

> The introduction to *The Eminem Show* says the album won the Grammy for Best Rap Album. Given the knowledge base's properties, the extractor records that the album's genre is Best Rap Album, and that it received Best Rap Album as an award. Asked to write what the introduction says in its own words, it records that the album won Best Rap Album.

## Context

Today extraction binds each fact to a property of the knowledge base's ontology at the moment it is written. The ontology arrives in the prompt, whole or trimmed, and a fact that finds no property is kept as an unbound proposal. The audit recorded in the first draft of this record found 59% of facts unbound on the timeline corpus, 1,093 distinct predicates, and a fifth of entities named by descriptions. The first draft answered with more stages around the same act of binding. A prototype built on 2026-09-15 tested the act of binding itself, and found that the errors live there.

## What the prototype measured

Setup: DeepSeek-V4-Flash with thinking off for extraction and agents; DeepSeek-V4-Pro as judge, calibrated against 24 hand-labelled facts (it agreed on 21: 8 of 9 correct facts accepted, 2 of 15 wrong ones passed). Corpora: 100 documents sampled from the Re-DocRED test set (Wikipedia introductions, 3,462 gold triples, 95 Wikidata properties), and the FDA, statistics-bulletin and SEC-filing corpora of `scripts/bench/recall.mjs`. Counts are single runs unless stated; three runs of one configuration on 50 documents varied by four points of judged precision and one point of gold recall.

No F1 is reported. Precision against Re-DocRED's gold is understated by how many true facts the gold omits (item 3), and by a different amount for each approach, so a harmonic mean of it with recall compares nothing. Precision is the calibrated judge's; recall is the share of gold facts recovered, which is sound because every gold fact is true.

**1. Statements written in the document's own words are faithful.** Of 333 open statements from two extraction prompts, the judge found none that the document does not state, and 2% worded wrongly.

| On 10 Re-DocRED documents | Statements | Stated | Stated but worded wrongly | Not stated |
|---|---|---|---|---|
| Open statements, full extraction prompt | 192 | 97.9% | 2.1% | 0 |
| Open statements, compact one-call prompt | 141 | 98.6% | 1.4% | 0 |
| Facts bound at write time, property names only | 78 | 74.4% | 16.7% | 9.0% |
| Facts bound at write time, with definitions | 114 | 79.8% | 15.8% | 4.4% |

The bound facts that are not stated are induced by the list of properties: an award written as a genre, a county given a notable work, a seventeenth-century king given a modern citizenship.

**2. Binding after the fact loses more than it keeps.** Aligning open statements to an existing ontology bound 4 of 113 to 567 phrase groups per corpus against the imported schema.org properties (FDA, statistics, SEC), and recovered 5.6% of Re-DocRED's gold facts against its own 95 properties, where extracting with the properties in the prompt recovered 15.3%. The loss comes from implicit facts ("a 1952 British film" has a country of origin) that the open statements do not state as statements.

**3. Gold overlap understates precision.** Re-DocRED still omits many true facts. Strict precision against gold was 37.8% for the sliced extraction; the calibrated judge accepted 76.1%. A hand review of 30 unmatched facts found 9 correct, 5 with a near-synonymous property, 6 debatable and 10 wrong. The orderings below hold for strict and judged precision alike; the judged figures are the ones quoted.

**4. A large ontology can be sliced per document, if its structure is complete.** With the 95 benchmark properties hidden among 872 schema.org properties, each document got a slice of about 133 properties that contained 86.6% of the relations its gold triples use. The slice was the union of properties whose domain and range meet the classes of the document's entities (34.0% alone), properties nearest to its statements' phrases (28.9% alone), and every property with no declared domain (the most frequent ones, country and located-in, have none). Without the class hierarchy and the Wikidata–schema.org equivalences, the structural part found 10%. Extraction with the slice reached judged precision 71.2% and gold recall 12.4%, against 77.8% and 15.3% with the 95 properties alone; merging the 49 schema.org properties Wikidata declares equivalent to benchmark properties raised them to 75.3% and 13.8%.

**5. Open and bound extraction complement each other when they run independently.** Unioning the open statements with the bound facts linked 46.8% of gold entity pairs (open alone 38.4%, bound alone 34.5%) and recovered 16.0% of gold facts. Giving the bound pass the open statements as context lowered recall to 42.9%.

**6. An agent can correct after extraction.** An errata agent reviewed all facts of 99 documents with tools to read the document, look up a property's definition, constraints, broader properties and inverse, and retract, revise or add. Judged precision rose from 76.1% to 89.5%; gold recall moved from 13.8% to 13.6%. It retracted 278 facts, revised 42 and added 49; the judge's count of correct facts fell from 871 to 814, so about a quarter of what it removed was right. Agent-written usage notes learned from half the documents and given to extraction on the other half helped no more than a generic caution of the same length.

**7. Cost.** For a 165-word document the full prototype spent about 23,000 prompt and 7,200 completion tokens. A compact pipeline (entity scan, cached type mapping, one extraction call, errata only for flagged facts) spent 3,449 and 753. On the same 10 documents it reached judged precision 80.0% and gold recall 15.1%, against 85.9% and 18.9% for the full prototype's bound facts after errata. Mapping entity type words to classes is paid once per distinct word across a knowledge base: the second run over the same documents met 5 new words instead of 54.

**8. Operational ontology platforms do not induce ontologies from text.** Their object types, links and actions are designed per use case by builders and backed by curated datasets; language models extract into a table a builder has already shaped, and edits are replayed over the backing data.

## Decisions

### 1. Three layers

- **The open graph** holds what documents say: entities with their names and type words, statements whose relation is the document's phrase, with their participants, qualifiers, time mentions and quotes. It needs no ontology and is kept whatever the ontology becomes.
- **The ontology** holds object types, link types and properties, each with a definition, examples and regression cases, plus constraints and actions. It is small, shaped by the questions the knowledge base must answer, and versioned on the recorded axis.
- **The typed graph** holds facts expressed in the ontology. It is computed from the open graph and marked with the ontology version it was computed under.

The word ontology in this project means the second layer only.

### 2. Extraction writes the open graph and nothing else

One call per chunk, a compact contract (arrays, short keys, no repeated names), no ontology in the prompt. The extractor reports entities with a type word, statements in the text's words, figures and titles as attributes of the thing they describe, time mentions as they are written, and the other names a document gives. Code checks that names and quotes occur in the chunk.

### 3. Alignment produces the typed graph: bindings and implication rules

Alignment is an operation of the review workbench over the open graph, decided per signature (the phrase, the subject's classes, the object's classes), never per fact. It produces two things:

- **A binding**: this signature is this property in this direction, or none. Cached with its confidence and the ontology version, reused wherever the signature recurs, so the cost grows with distinct phrasings rather than documents.
- **An implication rule**: a statement of this shape implies a fact of that property, with the value taken from a side of the statement or read from the object phrase by a stated reading (the country a nationality adjective names, the year a phrase gives). The aligner proposes the rule, the workbench approves it, and code executes it; a reading is applied once per distinct phrase and cached. Facts the rules produce are marked implied.

Alignment reads the source. Deciding a signature and applying a reading both see the chunk the statements came from, so a statement whose object is a phrase that names a thing ("located in the Piedmont region of the Commonwealth of Virginia") binds to the property and to the thing, as a reader would. Alignment is a typed reading of the text organised by signature, with the open graph as its index and the text as its evidence; it differs from binding at write time in that it never writes a fact without an open statement or a rule behind it, and never re-extracts a document when the ontology changes.

This is how the facts a reader draws without the text stating them (a place's country from its region, a film's country from "British film") reach the typed graph without a second extraction pass bound to the ontology. The first prototype of alignment read only the statements: on 30 Re-DocRED documents it recovered 6.5% of gold facts by bindings and 10–12% with rules, against 15.5% for the bound pass, and the gap sat where the aligner had not read the text. Until alignment reaches the bound pass on two clean runs, a bound pass with the ontology slice remains an option for an approved ontology, and its facts must attach to an open statement or be marked implied.

When the ontology changes, only facts under changed signatures and rules are recomputed. A signature with no property stays in the open graph, loses nothing, and counts toward the workbench's suggestions.

**Revision 2026-09-23 (implication rules built):** a rule is keyed like a binding (a phrase signature) or by a kind word, concludes one property, and takes its object either from the statement's own object or from a *reading* of the object's words (`country_of_nationality`, `country_of_place`, `year_of_phrase`). The aligner proposes rules after it has decided the signatures of a run and for every kind word once; a person approves or rejects on the alignment queue; approval commits with its job (0051). Readings are model calls made once per distinct phrase by a `read_phrases` job and cached in `phrase_readings`, so `materialize` stays free of model calls: it reads the cache and produces facts marked `implied`, with the triggering statement's evidence, retired by source like any other typed row. What is not read cannot be implied: a phrase the model cannot place caches as "no answer" and is never asked again until it is asked about differently.

**Revision 2026-09-23:** [0053](0053-a-phrase-decision-records-the-inputs-it-considered.md) replaces "a binding goes stale by timestamp" with a recorded basis: candidates are admitted through the class hierarchy, a decision stores a fingerprint of the closures and candidates it saw, and it is stale when the current fingerprint differs. No-candidate and overflow become recorded outcomes; the requeue condition reads live signatures only (#807, #795).

**Revision proposed 2026-09-21:** [0051](0051-a-human-phrase-decision-carries-its-materialization-work.md) addresses delivery after a human binding commits beyond an older materializer’s final read. It proposes a decision and its own durable job in one transaction, while retaining the current projection semantics. The asynchronous HTTP/job/UI contract remains unimplemented.


### 4. The ontology is built on a workbench from three sources

The ontology page becomes a workbench. Its elements come from three sources: **suggestions from the open graph** (the most frequent unbound signatures, the type words in use, and an ontology agent that reads them against competency questions and proposes object types, link types, properties and rules with definitions, examples and the signatures they would bind); **an imported file** (a pack of 0008, schema.org, an OWL or JSON-LD file); and **online editing**. People approve through actions; every approved element carries regression cases drawn from the open graph, and changing a definition reruns them. The ontology is judged by whether the competency questions can be answered correctly. Structure the slice and the rules depend on (class hierarchy, equivalences, domains, ranges) is part of approval, and duplicate properties are merged as part of governance.

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
| Typed graph | Re-DocRED with its 95 properties as the approved ontology | judged precision with the judge's agreement on a hand-labelled set; gold recall, split into facts whose two ends share a sentence and facts that need more than one; cost per document as the corpus grows. Strict precision, recall and F1 only beside published results |
| Ontology | FDA and statistics corpora with written competency questions | questions answered correctly; share of proposals people change |
| Time | `temporal.mjs`, the lease bench, SEC fiscal periods, statistics bulletins | normalised value and granularity, statement valid time, as-of answers |
| Identity across documents | `identity.mjs`; Linked-Re-DocRED documents that share non-location entities or namesakes of different types (GPL-3.0, kept outside the repository) | pairwise precision and recall, with wrong merges and missed merges apart; adjudicator calls per thousand entities |
| Errata | the typed-graph benches | precision gained, correct facts removed, tokens |

Thresholds to pass before a cut lands: the open graph at or under 2% not-stated with entity-pair recall no lower than today's; the typed graph at or above the full prototype on the same Re-DocRED documents in judged precision (75.3% on the 100-document sample before errata) and in gold recall of facts whose ends share a sentence, each reported over two runs, at under a fifth of its tokens per document. Recall of facts that need more than one sentence is reported and waits for derivation rules before it becomes a threshold.

## Cuts

1. The open graph in the ledger: statements, time mentions and names with provenance on both clocks; the compact extraction contract behind a flag.
2. Alignment: a table of signature → property, direction, confidence, ontology version; implication rules and their cached readings; materialisation and recomputation of the typed graph.
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
