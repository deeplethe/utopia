# 0041 · A name is a claim about an entity

- **Status**: decision 1 settled 2026-09-13 (names are facts) · cut 0 built: `scripts/bench/identity.mjs`, baseline on `dev` forward F1 0.43, reverse 0.54 · cut 1 implemented (#670): migration 0055 and `names` in the store, the extractor's `names`, shared-name pairs to the adjudicator; forward 0.68, reverse 0.68, the two orders agree on all 210 pairs (one run each; two cut-1 runs differed by 0.07 forward) · decisions 2 and 5 revised by cut 1 · cuts 2–4 not started
- **Written**: 2026-09-13 (conventions in the [README](README.md))
- **Related**: [0009](0009-no-type-is-a-type.md) made an undecided type an honest state, and [0016](0016-close-the-open-seams-before-cutting-new-ones.md) B3 let a declared `disjointWith` keep names apart; #270 stopped a namesake tie from being settled by candidate order and #331 let facts break it; #428 and [0025](0025-governance-reads-the-ledger-before-it-decides.md) moved duplicates through a queue an agent works; #582 and #583 made the extractor copy the words that name each side of a fact; [0037](0037-a-relation-carries-its-own-attributes.md) put attributes on edges.

> A probe is written 海洋探测器1号 in its launch notice and 海探1 in every report after it. Two engineers at one company are both 张伟. Today the probe becomes two entities with nothing in Review. The engineers become one entity, or a tie that waits for a person forever, depending on which document arrived first.

## What resolution does today

`resolve_mention` in the store resolves one mention at a time, while the extraction job writes facts:

1. **Recall is an exact string match.** Candidates are entities of the mention's type whose `canonical_name` or `aliases` equal the mention's name or one of its `recall_keys` (the name with a generic suffix stripped, from `GENERIC_SUFFIXES_CJK` and `GENERIC_WORDS_EN`). No other entity is ever looked at.
2. **A context vector decides.** `profile_embedding` is the running mean of the vectors of the chunks an entity was mentioned in. A candidate at cosine ≥ `SIM_ATTACH` (0.55) is attached; below `SIM_NEW` (0.35) a new entity is made; in between, a new entity and a pair for the adjudicator. Two same-name candidates within `SIM_TIE_MARGIN` become a pair for a person (#270), unless `corroborating_candidate` finds the name of one candidate's object as a substring of the chunk (#331).
3. **Names other than the canonical one are learned only by merging.** `merge_entities` appends the source's names to the target's `aliases`, and `revert_merge` takes them out. Nothing the extractor reads becomes a name, including a sentence that states 简称海探1.
4. Around that: `containment_reviews` queues pairs whose names contain one another, for names of four characters and more; `resolve_type_drift` handles the same name under another type; `doc_cache` in the extraction job sends every later mention without a response handle of the same type and lowercased name in a document to the first result; with no embedding model a mention attaches to the candidate with the most facts.
5. In governance, `name_shape` classifies a pair from English affixes and prepositions (`CORPORATE_SUFFIX`, " of ", "'s"), and the gate will not merge a `Version` or `Phrase` shape.

## What goes wrong, and why it is one problem

- **An alias with no bridge is a silent second entity.** 海探1 in a document that never writes the full name matches no string, is under four characters for `containment_reviews`, and leaves no review row. The comment on `containment_reviews` names this hole and leaves it open.
- **A context vector records what a passage is about. It does not record who is in it.** Two namesakes described in the same kind of document have almost the same profile and tie for good. One person mentioned in a board memo and in a technical specification drifts until a later mention falls under `SIM_ATTACH`, and splits.
- **Evidence the extractor has already produced goes unused.** The response that names 张伟 also says `张伟 worksFor 财务部`. Resolution runs before that fact is written, and facts take part only through a substring search for object names in the chunk.
- **The outcome depends on arrival order.** Resolution is greedy and never looks back: the first document seeds the profile, later ones attach to it or split from it.
- **The word lists do not travel.** `GENERIC_SUFFIXES_CJK`, `GENERIC_WORDS_EN`, `CORPORATE_SUFFIX` and the preposition list in `name_shape` cover the phrasings someone thought of, mostly English. `name_shape` cannot see the shape of a Chinese name at all, so the gate's refusal to merge a version or a phrase does not exist for Chinese names.

All of it comes from one choice: **identity is keyed on the surface string, and a topic vector chooses among the strings that match.** A namesake is one string for several things and an alias is several strings for one thing; they are the two ways that key fails.

## Decisions

### 1. A name is a fact about an entity

Every name a text uses for an entity is recorded as a fact on a system attribute `name`, with the quote it came from, the document's times, and a world validity when the text gives one ("renamed in 2021" closes the old name). `entities.canonical_name` stays as the label the interface shows, and it is also one of the entity's name facts. `entities.aliases` is retired: the migration turns each existing alias into a name fact attributed to the merge that put it there.

A fact, because a name can be wrong, corrected, superseded and taken back, which is what the ledger already does for everything else it holds. As facts, names get both clocks ("what was it called in 2019", "when did we learn that 海探1 is 海洋探测器1号"), provenance, revert, and a place in the export as `skos:altLabel`.

**A name is a value, never a node.** A name fact keeps the string in `facts.object_value` and leaves `object_id` empty, the same channel a quantity uses since #586 and #587. The canvas draws only facts with an `object_id`, and the graph's fact count counts only those, so names add no node, no edge and no count there.

**The temporal engine leaves it alone by construction.** `temporal` closes an old value and records a conflict only for state relations declared functional or inverse-functional. `name` is declared neither, because namesakes exist, so a second name never closes the first or opens a conflict.

**The price** is every reader that lists an entity's facts: the entity panel shows names in their own list, beside the facts; `entity_fact_lines` on a Review card and the `degree` used in resolution leave names out, so a name does not pass for evidence or pad a count; `entity_facts` for chat and MCP returns names marked as names; the RDF export writes them as `skos:altLabel`.

Considered: an `entity_names` table with the same columns. It keeps conflicts and reasoning untouched, and rebuilds validity, provenance, invalidation and revert beside the ledger that already has them.

### 2. The extractor reports names, and the server checks only that they are in the text

Two sources, one contract:

- **Spans.** Every fact already carries `subject_span` and `object_span`, bound to a handle whose `name` is the full name. A span that differs from that name, occurs in the quote, and is marked by the model as a name, as opposed to a pronoun or a description, becomes a name fact with the fact's quote.
- **Stated names.** Each entity in the contract gets `names`: the other names the text gives it. The server keeps a name only when it occurs verbatim in the chunk. It does not recognise 简称, 又名, aka or formerly, and does not need to.

Whether a span names the entity or refers to it is the model's call, written into the contract. The server's check is whether the string is in the text. There is no word list.

> **Revised 2026-09-13 (cut 1, #670).** One channel, not two: `names: [{ref, name, quote}]`. Spans stay a verification signal only; a model that follows rule 1b already lists the shortened form it used, and a second source would repeat the same check. The server keeps a name when the name is in its quote and the quote is in the chunk, and drops it when the same response or an earlier chunk of the document declares that string for a different entity (`name_claimed_by_another`) — one string cannot name two things, and that needs no vocabulary. The name the model writes for an entity gets the chunk as evidence when the chunk contains it. **Known gap:** a description the model reports as a name without declaring it as an entity still passes ("海探1项目" became a name of 海洋探测器1号 in every cut-1 run).

### 3. Recall has three channels, and none of them decides

Candidates for a mention come from:

1. **Names.** Current and former name facts, equal after `normalize_name`. A former name stays recallable, because older documents still use it.
2. **Name vectors.** The nearest name facts by the embedding of the name string, within the same type family, top k. This is the channel that finds 海探1 for 海洋探测器1号, and a name written in two scripts.
3. **Neighbours.** Entities that share a distinctive neighbour with the mention's facts in the same response: the same predicate, the same resolved object, and an object the other candidates do not also point at.

Recall only proposes. `recall_keys`, `GENERIC_SUFFIXES_CJK` and `GENERIC_WORDS_EN` leave once the bench shows channel 2 finds what they found.

### 4. Evidence decides, read from the ontology and both clocks

For each candidate, evidence is read from the chunk's resolved facts against the candidate's current facts:

- **For the same entity**: a shared distinctive edge (as defined in decision 3) with overlapping validity; the same value on a predicate the ontology declares inverse-functional, with overlapping validity; a name the text states for this entity (decision 2).
- **For a different entity**: different values on a predicate the ontology declares functional, both held over overlapping validity; types declared disjoint, as today. An event relation contributes no conflict, since two dated events are not two values of one slot.
- **Neither**: disagreement on a predicate that may hold several values, since a person can work at two places; and a shared name, which says nothing in either direction.

The rule: evidence against a candidate rules it out. One candidate left with evidence for it is attached. Several left with evidence gives a new entity and a pair for a person, as #270 does today. No evidence at all takes today's path: a new entity, and a pair for the adjudicator when channel 1 or 2 found the candidate. The context vector breaks ties between candidates whose evidence is equal, and does nothing else. **When unsure, split and ask**, as before.

To have edges to compare, a chunk's mentions resolve after the response is parsed: sides with one candidate and no evidence against it first, then the ambiguous sides, using their edges to the sides already resolved. `doc_cache` keyed by name is removed. Handles unify mentions within a response, and evidence unifies them across chunks.

### 5. Arrival order stops mattering

> **Revised 2026-09-13 (cut 1, #670).** The first step landed with the names themselves, because without it cut 1 made reverse order worse: the abbreviation-only documents built 海探1 first, the document stating 简称海探1 arrived last and put the name on a new full-name entity, and nothing paired the two. Now a name that another entity of a compatible type already has queues the pair for the adjudicator (`shared_name|…`), never a merge, and the adjudicator and the Review card see an entity's non-canonical names as an `also known as:` line. Without that line the adjudicator kept the pair apart at 0.90–0.95. The canonical name is not listed, so two namesakes do not appear to share evidence. Re-evaluation on a new distinctive edge, and the periodic job, remain cut 4.

When an entity gains a name fact or a distinctive edge, the pairs it now recalls are evaluated again and queued, through Review and the governance gate, never merged in place. A periodic job does the same for every group of entities that share a name. The document that states 简称海探1 therefore finds the 海探1 created a week earlier, and a pair decided on thin evidence comes back when better evidence arrives.

### 6. `name_shape` gives way to evidence

The gate refuses `Version` and `Phrase` shapes because the model says *same* for Claude and Claude 4, and for "Sam Altman's efforts" and Sam Altman. Decision 4 separates the first by a functional conflict (two release dates), and the span check of #582 keeps the second from becoming an entity. `name_shape` and `CORPORATE_SUFFIX` leave the gate once the bench shows the gate catches nothing that evidence misses.

## Not doing

- **A lexicon of abbreviations, honorifics or company forms.** Every list on this path so far has covered the phrasings its author thought of.
- **A learned identity model.** The rule in decision 4 is small enough to read, and every attach or split can name the edge or the conflict that settled it. That is what a Review card and an auditor both need.
- **Merging on name similarity.** Channel 2 finds candidates. Evidence decides.

## Measurement: cut 0

An identity bench in `scripts/bench/`, beside `recall.mjs` and `govern.mjs`: a small bilingual corpus and a truth file that groups mentions by the entity they refer to, scored as pairwise precision and recall of *same entity*, plus the number of pairs sent to people.

The corpus covers two namesakes at one company across documents; two namesakes in one document; one person across unrelated documents; a full name and its abbreviation in one document, then the abbreviation alone in another, in both arrival orders; a rename with a date; one name in Chinese and in English; and look-alikes that must stay apart (two companies' 技术中心, a product and its next version, a company and a fruit). Every run is repeated with the documents in reverse order, and the difference between the two orders is reported as a number.

`ai-timeline.duplicates.json`, the truth behind `govern.mjs`, runs alongside as a regression.

## Cuts

0. The identity bench, and a baseline on `dev`.
1. Names become facts (decision 1), the extractor reports them (decision 2), and recall reads them (decision 3, channel 1). Merge and revert move name facts; `entities.aliases` is migrated and dropped.
2. Recall channels 2 and 3.
3. Evidence decides (decision 4); mentions resolve after the response is parsed; `doc_cache` by name goes.
4. Re-evaluation (decision 5); `name_shape` out of the gate and the generic suffix lists out of recall, each when the bench says so (decision 6).

The entity panel's list of names with their sources and validity, and the evidence lines on a Review card, follow once cut 3 lands.
