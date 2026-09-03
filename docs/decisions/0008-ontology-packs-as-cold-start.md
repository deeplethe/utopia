# 0008 · Ontology packs as the cold start

- **Status**: Built · five packs embedded in the binary (gzip, 1.7 MB → 316 KB),
  multi-select at KB creation with schema.org checked by default, 22-row static alignment
  table · a new KB seeds no relations: the packs are the whole ontology · the three open
  questions stay open; the Chinese-label one got worse (2026-09-02 check)
- **Written**: 2026-08-30 · condensed into English 2026-09-03
- **Related**: [0001](0001-ontology-import-and-governance.md) criteria and IRI/key split;
  [0006](0006-ontology-scale-and-the-prompt.md) prompt budget;
  [0007](0007-who-decides-what-becomes-a-relation.md) growth loop;
  [0009](0009-no-type-is-a-type.md) the empty start;
  [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) the packs on a real corpus

## The problem

The ten seed relations a new KB used to receive (since removed) had no domain or range. The
extraction prompt supports signatures (`- buys_from (employee|team → *)`), but with none to
feed it each predicate degraded to prose, and prose cannot fix direction: `produces` — "an
organization or project makes or releases the object" — hints at the subject's type without
constraining it, so a model reading "Anthropic's Claude" has nothing machine-checkable to
say which side is the subject. Only `part_of` had a counter-example written in, after a
mishap; fixing direction with prose costs one mishap per predicate and holds only under that
line. Ten predicates do not cover real documents either (most facts degraded to
`related_to`), and widening to dozens does not fix direction.

schema.org turns direction into structure: 1010 classes, 1521 properties, **1488 (97.8%)
with both domain and range**. With `schema:manufacturer Product → Organization`, `Claude
manufacturer Anthropic` is legal and the reverse is stopped at domain validation before it
reaches the graph.

## Candidates

Counted from the official release files of 2026-08-30 (not from the dev database: a mock
corpus shows the shape of errors, not real ratios). Criteria: readable by the current
importer (Turtle / RDF-XML), open license, has domain/range.

| Pack | Size | Classes / properties | Fills |
|---|---|---|---|
| schema.org | 1078 KB | 1010 / 1521 (the shipped pack counts 1676) | general: people, organizations, products, events, creative works |
| W3C Org | 82 KB | 13 / 34 | what `schema:Organization` lacks: Unit, Post, Membership, Role, tenure, reporting lines |
| PROV-O | 110 KB | 62 (shipped: 49) / 69 | provenance: Activity, Agent, wasDerivedFrom |
| FOAF | 43 KB | 12 / 62 | social relations; 21% overlap with schema.org and the least unique value — a candidate, not a default |
| IOF Core | 394 KB | 294 / 75 | industrial manufacturing |

Overlap by `key_from_iri()` (keys collide, not IRIs) is about 20 words in total: org ∩
schema 6, foaf ∩ schema 15, prov ∩ schema 4. True synonyms are mapped (`foaf:Person ≡
schema:Person`); same name with a different meaning stays as two (`org:role` is a post,
`schema:role` a part in a creative work; `foaf:status` is chat presence, `schema:status` an
order status).

## Decisions

1. **Multi-select at KB creation, no bundles.** Alignment is declared pairwise —
   `foaf:Person ≡ schema:Person` holds whatever else is selected — so multi-select adds no
   alignment cost, and fixed bundles are guesswork that never matches: fintech wants IOF
   plus a future FIBO, a consultancy wants schema plus Org plus PROV. The UI shows each
   pack's size and lets the user weigh it.
2. **No import undo.** Three guards — `ON DELETE RESTRICT` (`0003_graph.sql`),
   application-level counts in `ontology.rs`, and `NOT builtin` (now an empty condition) —
   already mean a type with entities cannot be deleted. That is right: the inverse of "the
   ontology guides, it does not enforce" — **knowledge can veto the deletion of ontology**.
   What "wrong pack" needs is a bulk view ("this import created 294 classes, 12 in use, 282
   empty — delete the empty ones"): simpler than undo, touches nothing that holds data. Not
   built yet.
3. **Static alignment table, no runtime inference.** About 20 rows by hand (22 built,
   `SameAs` and `Rename`, e.g. `org:Role → org_role`), partly copied from upstream: W3C Org
   declares its alignment with FOAF, PROV-O with FOAF and Dublin Core. No label similarity
   or embedding matching — the packs are ours, five or six of them, and the overlaps can be
   enumerated. An unbounded problem shrinks to a table, per 0001's criterion 6: governance
   experience goes into the schema, not into hidden rules.

## Dead ends

- **Three fixed bundles** (general / provenance-compliance / industrial) and
  **`undo_import`**, both in the first draft — rejected, see decisions 1 and 2.
- **Showing overlap with the packs already selected** was planned; `2fa8d57` removed the
  collision hint entirely. The alignment table handles the collisions, and reporting them
  asks the user to rule on something with nothing to rule on.
- **Packs that do not fit**: FIBO is modular, hundreds of files by catalog (finance is a
  project, not a pack); Brick (1438 classes, 25 properties) and QUDT are taxonomies, not
  relation ontologies; SAREF projects to 0 classes and 0 properties, its declaration form is
  not covered yet; SNOMED CT needs a license.

## Revisions

- 2026-09-02: the alignment table grew three things not foreseen: a third disposition
  `Aligned` (beside `Create` / `Update` / `KeyTaken`); direction sensitivity — rows are
  (incoming IRI, occupying IRI), so install order decides which row hits, hence the visible
  rule "check order is install order"; and a guard test that projects all five real packs
  and checks every IRI, against a silent typo and upstream prefix changes (schema.org moved
  from `http://` to `https://`).
- 2026-09-02: a failing pack does not roll back the packs already installed — the ontology
  is additive, and rollback is the undo this record refuses. And there is a start at **0**:
  no pack is legal, entities carry `type_id` NULL ([0009](0009-no-type-is-a-type.md)).
- 2026-09-02: "the ontology guides, it does not enforce" was half overturned. On a real
  corpus (0012) the vocabulary caught things — 215 facts with predicates, all 41 relations
  from packs — but not one declared direction was enforced: 102 of 130 checkable facts were
  stored reversed; the fix bends to the signature at write time and leaves a trace. The cost
  of packs is not prompt length but **choice**: many names are generic with narrow meanings
  (`affectedBy` is a medical test, `competitor` a sports event), and picking by name or
  vector will hit them.

## Open questions

- **Mixed-pack extraction accuracy is untested.** 0006 measured one ontology; all packs
  together are about 2400 classes, and chunk retrieval recalls `org:role` and `schema:role`
  side by side. Fitting the budget is not picking right. `run.mjs --packs` runs the real
  cold-start path, but `truth/` still holds only pharma and tech.
- **schema.org is web-facing** (Recipe, JobPosting, Event); enterprise "contract",
  "approval", "supplier qualification" are absent. 0007's growth loop stays, starting from
  1500 instead of 10; the README turns "packs by industry" into an issue-template link.
- **Chinese labels**, worse since the 14 Chinese-named seed relations left: a Chinese KB now
  gets a purely English ontology, `ontology_lang = zh` affects only terms the LLM proposes
  later, and nothing tells the user. 1500 properties cannot be translated by hand and should
  not be machine-translated — `rdfs:label` is a token the model reads, and a wrong label is
  worse than none.
- **Starting scale is a variable.** 0007's Snowball run showed that growing the vocabulary
  from 10 to 629 changes matching (49 recalls → 18). Nobody has studied what starting at
  1500 does to the adoption loop; `MIN_DOCS = 2` and `MIN_SIGNALS = 3` were never retuned.
