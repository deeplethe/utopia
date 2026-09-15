# 0044 · A document is read in stages

- **Status**: Proposed · nothing built · the next step is an offline prototype of stages 0 and 3, measured on five benches against today's extractor · prompt rules for today's extractor are frozen until it reports
- **Written**: 2026-09-15 (conventions in the [README](README.md))
- **Related**:
  - Evidence that motivated this record:
    - [0006](0006-ontology-scale-and-the-prompt.md): the ontology slice in the prompt.
    - [0039](0039-a-chunk-is-what-extraction-sees.md): chunks as the unit of extraction.
    - [0041](0041-a-name-is-a-claim-about-an-entity.md): names as facts and the identity bench. Its cuts 2–4 fold into decision 7.
    - [0022](0022-an-unknown-date-is-not-an-open-one.md): the temporal engine this record keeps.
    - #701: the per-chunk ontology budget.
    - #705, closed: sentence citations; names carry recovery.
    - #709: cross-script name vectors.
  - Records this one extends:
    - [0025](0025-governance-reads-the-ledger-before-it-decides.md) and [0043](0043-every-review-queue-is-governed.md): governance of every queue. Decision 8 changes what the queues hold.
    - [0003](0003-ontology-growth-loop.md): the ontology grows out of the corpus. Decision 5 is where it grows from.

> A lease signed in 2016 is amended eleven times. Each amendment cites the lease and its earlier amendments by title and date. Today every 1,000-character chunk of every amendment asks one model call for all of the following at once:
>
> - the entities, their names, aliases and types, chosen from 200 candidate classes;
> - the facts, their predicates chosen from 180 candidate relations, and their direction;
> - values, units, validity dates, quotes and the words that name each side.
>
> The chunk cannot see the rest of the document. "The Fifth Amendment dated February 18, 2020" comes back as its own entity, with its date inside its name. A cited list of amendments comes back as "the Ninth Amendment is an amended version of the Fifth".

## What the extractor produces today

Measured on 2026-09-14 and 09-15 with DeepSeek-V3.2, on bases built by today's `dev`. The audit query is in the PR that adds this record.

| base | entities | facts | facts not bound to the ontology | distinct predicates |
|---|---|---|---|---|
| Blackbaud lease chain (lease bench, 17/18) | 47 | 154 | 70 (45%) | 70 |
| ai-timeline (8 encyclopedia articles, schema.org) | 1,117 | 2,451 | 1,440 (59%) | 1,093 |
| identity bench (12 Chinese notices, forward) | 19 | 25 | 7 | 9 |

- **Descriptions become entities.**
  - ai-timeline: 242 of 1,117 entity names are descriptions ("a datacenter engineer at Google", "academic neuroscientists", "$1.5 billion settlement").
  - The lease has "approximately 12.98 acres of real property located in Berkeley County, South Carolina". Tenant and landlord edges bind to it, with quotes that do not mention it.
  - 144 ai-timeline entities hold nothing but their names.
- **Citations leak into names, and aliases are wrong.**
  - 11 of 12 lease amendments carry an alias "… dated 2020-xx-xx", and one entity is named "this Seventh Amendment to Lease Agreement".
  - ai-timeline records "OpenAI's", "ChatGPT apps", "ChatGPT and other chatbots" and "Microsoft Azure infrastructure" as aliases.
  - It also records "deep reinforcement learning" as an alias of "reinforcement learning", which is the pair the governance bench keeps merging wrongly.
- **One thing splits by type label.** AlphaGo is three entities (movie, project, software application); so are Anthropic, GitHub and University College London.
- **Relations are misread.**
  - "BLACKBAUD `tenant` 'Lease Agreement'" is written as a literal, backwards.
  - Landlord edges point at an amendment.
- **Figures about unnamed things are lost.** The FDA recall bench lost trial sizes and effect percentages in both baseline runs (31/45): the trials have no name, and the span check (#582) drops a subject that is a description.
- **Dates are missing.** 34 of the lease's 154 facts and 167 in ai-timeline have no start although the quote holds a year. Not all of those years are the fact's own.

The rules the prompt carries have grown to 1, 1a, 1b, 2, 3, 3a, 3c, 4 to 8, 8a to 8h, 9, 10 and 11. Each was written for one failure on one corpus, and they now pull against each other:

- #705 removed names from facts to save output. The lease bench fell from 13 and 17 to 8 and 8: names were how the server recovered a fact whose handle the model got wrong.
- The span check that keeps "former OpenAI personnel" from becoming OpenAI is what drops every figure about an unnamed trial.
- The server's checks keep growing to catch what one call cannot do: span verdicts, description checks, opening-leak checks, name checks.

A stronger model does not settle it. With thinking off, on the same code:

| bench | DeepSeek-V3.2 | deepseek-v4-flash |
|---|---|---|
| FDA approvals recall | 34/45 | 41/45 |
| Chinese statistics and policy recall | 44/50 | 36/50 |
| identity F1 (forward / reverse) | 0.80, 0.77 / 0.69, 0.63 | 0.67 / 0.59 |
| governance agreement, wrong merges | 97.8%, 1 | 97.2%, 2 |

One run each. The stronger model reads English regulatory text better and is no better at Chinese notices or identity.

## What mature systems do

Surveyed on 2026-09-15: GraphRAG, LightRAG, Graphiti, KGGen, iText2KG/ATOM, neo4j-graphrag, LlamaIndex PropertyGraphIndex, LangChain LLMGraphTransformer, Cognee; the canonicalisation literature (EDC, SPIRES/OntoGPT, CESI, CMVC, AutoSchemaKG, ODKE+); entity resolution and linking (GLiNER, ReLiK, Splink, LLM entity matching, ACE/ERE guidelines); temporal extraction (TimeML, HeidelTime, ATOM, Graphiti's timestamps).

1. **Entities before relations.** An inventory comes first, and a relation's ends must be members of it. KGGen enforces this with a `Literal[...]` over the inventory in structured output ([source](https://github.com/stair-lab/kg-gen/blob/main/src/kg_gen/steps/_2_get_relations.py)); Graphiti drops edges whose ends are not in the node list.
2. **Extraction is loose; convergence happens after.**
   - None of these systems holds predicates down with prompt rules.
   - EDC extracts open triples, writes a definition for each distinct relation, then canonicalises it against a schema by retrieval and an LLM choice that may answer "none" ([paper](https://arxiv.org/abs/2404.03868)). It reports 0.956 precision against CESI's 0.724.
   - SPIRES grounds strings to ontology IDs with a retriever and annotator: 98 of 100 correct, against 3 of 100 when the model writes the IDs itself ([paper](https://academic.oup.com/bioinformatics/article/40/3/btae104/7612230)).
3. **Resolution is layered.** Exact match first, then character or embedding similarity, and an LLM only for what stays ambiguous (Graphiti's MinHash with an entropy gate, [source](https://github.com/getzep/graphiti/blob/main/graphiti_core/utils/maintenance/dedup_helpers.py)).
   - Deciding identity from names alone fails structurally. On a cross-script name benchmark, name-only judgements got all 25 namesake cases wrong and 24 right once evidence was given ([paper](https://arxiv.org/html/2608.23507)).
4. **Names and descriptions are separated by structure.**
   - ACE and Rich ERE grade mentions as NAM, NOM or PRO. A nominal needs an article, quantifier, possessive or modifier to reach an individual, and an appositive that only states a property is attributive ([ACE guidelines](https://www.ldc.upenn.edu/sites/www.ldc.upenn.edu/files/english-entities-guidelines-v6.6.pdf)).
5. **Time is normalised outside the extraction call.**
   - Graphiti moved date resolution into its own step in v0.29.
   - Splitting text along its timeline before extraction took temporal fact F1 from 14.43 to 66.71 ([paper](https://arxiv.org/html/2405.10288)).
   - HeidelTime covers 13 languages including Chinese.
6. **Long context does not replace chunking.** Every model in Chroma's context-rot study degrades as input grows ([study](https://www.trychroma.com/research/context-rot)). What chunked systems add is document context: section paths (LightRAG), prior episodes (Graphiti), a per-chunk context line ([Anthropic](https://www.anthropic.com/engineering/contextual-retrieval)).
7. **Temporal invalidation stays local.** Graphiti searches the whole group for contradicted facts. On one production graph, 41% of about 3,950 facts carried `invalid_at`, and three of four sampled were wrong ([issue](https://github.com/getzep/graphiti/issues/1728)). Our engine closes values along one timeline per functional predicate (0022) and stays as it is.

## Decisions

### 1. Extraction becomes a pipeline of stages with narrow contracts

| stage | produces | done by |
|---|---|---|
| 0. Skim | a document card (decision 2) | one LLM call per document, or per major section for long ones |
| 1. Mentions | graded mentions, in-document coreference, the document's entity inventory with names and aliases (decision 3) | LLM per section, with the card |
| 2. Statements | statements in the text's own words, with ends constrained to the inventory, raw time expressions and an evidence sentence (decision 4) | LLM per chunk, with the card and the section path |
| 3. Normalisation | bindings of phrases and type words to the ontology, and proposals for what does not bind (decision 5) | retrieval, then an LLM choice once per distinct phrase per base, then governance |
| 4. Time | validity intervals from time expressions and the card's anchors (decision 6) | deterministic code |
| 5. Identity | entities across documents (decision 7) | layered candidates, scoring, an LLM for the grey zone, batch clustering |
| 6. Ledger | facts, evidence, both clocks, the temporal engine, governance | unchanged |

- Each stage's output is stored with its evidence, so a later stage can be rerun without rerunning an earlier one.
- A stage that fails leaves the document partly read, with the reason, rather than a document that looks read.

### 2. The skim produces a document card, not a summary

The card has fixed fields, and every value carries an evidence span that the server checks against the text:

- the document's own date and effective date;
- the parties, and the defined terms and who they denote ("Landlord" is HPBB1, LLC; "the Lease" is the agreement dated May 16, 2016);
- the chain of documents it amends or cites, each with its title and date;
- the section tree and its tables;
- the canonical names of the main entities it concerns.

How the card is used:

- **It is data.** A document's date, parties and amendment chain go into the ledger from the card. Nothing is prised out of names.
- **It is context.** Stages 1 and 2 receive the card as a shared prefix, so prefix caching applies. It replaces the 1,500-character opening (#681) and the 1,200-character list of known entities.
- **The text prevails.** A chunk that contradicts its card is extracted as the chunk says, and the contradiction is recorded.
- **It is bounded.** It holds these fields and no "key points". A long document gets a document card plus a card per major section.

### 3. Mentions are graded; descriptions never become entities

- **Grades.** Stage 1 grades every mention as a name, a description or a pronoun, following ACE/ERE.
- **Descriptions.** A description ("a datacenter engineer at Google", "the second trial", "approximately 12.98 acres of real property") never becomes an entity. What it says becomes a role or attribute of the named entity it belongs to, or, for an unnamed thing, of the named entity it concerns. The trial's size belongs to the drug it tested; the land's acreage belongs to the lease that leases it.
- **Coreference.** It runs within the document.
- **Aliases** come only from name mentions in the same coreference chain, and structure rules the rest out:
  - a possessive ("OpenAI's") is not an alias;
  - a change of head ("ChatGPT apps", "Microsoft Azure infrastructure") is not an alias;
  - a modifier that narrows the reference ("deep reinforcement learning") is not an alias;
  - citation text ("dated …", "this …", "as amended") is not part of a name.
- **What the server checks.** It keeps its structural span verdicts (#582) and applies them to entity names and aliases too. It keeps no word lists.

### 4. Statements use the text's words and name their ends from the inventory

- **Output contract.** Each statement has:
  - its ends, as inventory handles or a literal value;
  - the relation in the text's own words, with a one-line definition;
  - the raw time expressions it states;
  - its evidence sentence.
- **Enforced by structure.** The contract is enforced by a strict tool call whose handle fields are enumerations over the inventory. The schema stays shallow: one statement per array element, so truncation loses a statement rather than a chunk.
- **Reasoning first.** A short reasoning field comes before the answer fields.
- **No ontology in the prompt.** Stage 2 does not see the ontology. A type signature hint may list the few most frequent predicates for the entity types in the chunk; EDC's schema retriever was worth about 0.04 F1.

### 5. Normalisation happens once per distinct phrase, and what does not bind becomes a proposal

- **Key.** A binding is keyed by (phrase, definition, type signature) per base and cached.
- **Choice.**
  - Candidates come from embedding the definition against the ontology's relations, with each candidate's inverse listed explicitly.
  - An LLM answers one of: this key; this key reversed; none.
  - Types bind the same way, per entity rather than per mention.
- **History.** A binding is a record with both clocks. A later binding supersedes it, and the facts it covers are rebound; they are not re-extracted.
- **What stays unbound.**
  - A phrase that binds to nothing stays on its facts.
  - Unbound phrases are clustered, and a cluster becomes an ontology proposal (definition, domain and range, examples, frequency) in the governance queue.
  - Proposals are checked against existing properties first, as Wikidata's property process requires.
  - A decided proposal writes a binding.
- **Pitfalls to design against:**
  - merging adjacent or opposite relations (CESI clustered place of birth with place of death);
  - systematic asymmetry in judging inverses;
  - lower F1 when the ontology may grow unreviewed.

### 6. Time expressions are copied by the model and resolved by code

- **Copying.** Stage 2 copies time expressions verbatim with their spans.
- **Resolution.** Stage 4 resolves them to intervals with precision, anchored on the card's document date and effective date. English uses a rule-based normaliser; Chinese uses one that handles relative expressions and 上年末-style anchors.
- **Three dates.** A document's date, a clause's effective date and an event's date are separate fields. The document date is an anchor for relative expressions, never a default start.

### 7. Identity is decided on evidence, independent of arrival order

- **Candidates** are the union of:
  - exact normalised names;
  - character n-gram similarity;
  - romanisation keys;
  - cross-script name vectors, mutual nearest across scripts (#709);
  - abbreviation keys (the shorter name's characters in order within the longer, numerals equal).
  - A type label is a weak feature, never part of the key.
- **Scoring.**
  - Fellegi–Sunter-style scoring with term-frequency adjustment lowers the weight of a common name ([Splink](https://moj-analytical-services.github.io/splink/topic_guides/comparisons/term-frequency.html)).
  - Its features are context, co-occurring entities, roles and dates.
  - Conflicting functional values in overlapping time are cannot-link constraints. So is being declared apart in one response.
- **Grey zone.** It goes to the governor as a choice among candidates, with evidence required ([ComEM](https://arxiv.org/abs/2405.16884)). A name alone never merges.
- **Clustering.** Pair scores are stored as evidenced edges and clustered over the whole base in batch, so the result depends on which documents exist, not on the order they came in. Online resolution attaches provisionally, and the next batch may split it.
- **Measurement.** The identity bench reports the mean and range over shuffled orders.

### 8. What people and agents review are decisions, and every decision writes back

The queues change from items to decisions, each shown with its impact:

| decision | from stage | impact shown |
|---|---|---|
| a phrase's binding, reversal or proposal | 5 | the facts it covers, and the answers that used them |
| an identity cluster and its cannot-links | 7 | the entities and facts it moves |
| a document card's chain, terms or dates | 2 | the documents and facts that rely on it |
| a statement without enough evidence, or one that conflicts | 4, 6 | the fact and its timeline |

- **Agents first.** The governor decides first (0025, 0043). A person sees what the governor could not decide, what has large impact, and where agents disagree, ordered by impact.
- **Write-back.** A decision writes back as input the pipeline uses deterministically: a binding into the alias table, a split into a cannot-link, a correction into the card. The same question does not return in another shape, and precedents are decisions rather than closed cards.
- **Data model.** `agent_decisions` (target kind, detail, undo) extends to these kinds.
- **Interface.** The interface follows once the prototype shows the volume and shape of real decisions.
- **Calibration.** Calibrating an LLM judge against people happens on labelled bench samples, not in the live queue.

### 9. Model use follows the stage

- **Thinking mode** only where reading is hard and the call is rare: the skim.
- **Stages 1 and 2** run with thinking off.
- **Per-stage settings.** A model setting can carry request options such as disabling thinking; deepseek-v4-flash spends about 800 reasoning tokens on a trivial reply. This also serves #690.
- **Local models.** Small local models serve as recall cross-checks, not as the main reader: GLiNER for English mentions; specialised Chinese IE models only as references. Their licences decide whether they can ship at all: ReLiK, REBEL, Maverick and IEPile are non-commercial, HanLP's Chinese models are research-only, LTP is paid for commercial use, and Zingg is AGPL.

### 10. Today's extractor is frozen while the prototype is measured

- No new prompt rules land on the current extractor.
- Structural fixes already merged stay: the per-chunk budget (#704) and governance changes.
- Open cuts that add prompt rules wait: an unnamed trial's figures, grant receivers, the romanisation rule in #709.

## Not doing

- **Untyped relations with descriptions only** (GraphRAG, LightRAG). They avoid predicate explosion by giving up the ontology, which is what this product governs.
- **Graphiti's group-wide contradiction search.** The temporal engine stays local to a timeline.
- **Grammar-constrained decoding against the whole ontology.** It forces wrong choices, favours empty output and is slow ([GenIE analysis](https://arxiv.org/html/2305.13971v6)).
- **One-shot whole-document extraction on long-context models.** Quality decays with length, and the card gives stage 2 the context without it.
- **A lexicon of honorifics, suffixes or citation words.** Decisions 3 and 7 use structure; semantics stay in prompts.

## Migration

1. **An offline prototype** runs stages 0 and 3 as scripts against today's extraction output on five benches:
   - lease chain;
   - FDA approvals;
   - Chinese statistics and policy;
   - NVDA filings;
   - ai-timeline.

   It uses the same model endpoint, with no server changes.
2. **Stages 1 and 2** join the prototype. Stage 0's card feeds them.
3. **In the server, behind a base setting** (old path by default), in order: card and time (0, 4), normalisation (3), mentions and statements (1, 2), batch identity (7).
4. **The decision queues and their interface** come last.

Each step lands only if every bench holds within its variance, and the audit metrics below improve.

## Measurement

- **Recall benches.** Every stage is measured on the five recall benches (`recall.mjs --corpus`, `lease_bench.py`), the identity bench over shuffled orders, and the governance bench.
- **Audit metrics** on the resulting bases:
  - distinct predicates per fact, and the unbound share;
  - description entities and names-only entities;
  - aliases that fail the structural checks;
  - entities split across type labels;
  - facts without a start whose evidence states a date;
  - statements whose ends do not appear in their evidence.
- **Normalisation bench.** Gold (phrase, context) → ontology key, reversed, new, or none. It measures binding precision, binding rate and direction accuracy, reported by frequency band.
- **Precision.** Precision needs sampled human labels. An LLM judge is used only after its agreement with those labels is known.
- **Cost.** Tokens and latency per document per stage, against today's single call.

## Open questions

- **Chinese coreference and time.** The mature open tools are English-first. Whether stage 1 coreference and stage 4 normalisation for Chinese are LLM calls or rule libraries is measured on the Chinese benches.
- **Atomic facts.** ATOM's decontextualised atomic facts raised fact recall by 31% and cost 9% precision ([paper](https://arxiv.org/html/2510.22590v2)). Whether stage 2 needs that layer is measured, not assumed.
- **XBRL.** Figures in SEC filings exist as structured facts; whether filings bypass stage 2 for them is a separate record.
- **Card cost on long documents.** It is measured on the 136-chunk earnings release before a section-card threshold is chosen.
