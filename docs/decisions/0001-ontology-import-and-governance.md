# 0001 · Ontology import and governance

- **Status**: In progress. P0, P1, P2 and P2c built. P3's budget switch built
  (`deployment_settings.ontology_prompt_budget`, default 24,000 characters; over budget the ontology
  is retrieved per chunk, [0006](0006-ontology-scale-and-the-prompt.md)). P3a built but runs only by
  hand. P3b built in a different shape: the surface predicate lands on
  `fact_evidence.proposed_predicate`, and mapping back is `predicate_match` plus the adoption loop of
  [0003](0003-ontology-growth-loop.md). P4a built (`entities.type_source`, #114); P4b and P4c pending.
  P5 delivered by [0002](0002-reasoning-engine.md). The "argument order" half of criterion 2
  overturned by [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md). Checked against the code
  2026-09-02.
- **Written**: 2026-08-27 / 28 · condensed into English 2026-09-03
- **Related**: [0002](0002-reasoning-engine.md) replaces P5's schedule;
  [0003](0003-ontology-growth-loop.md) is what P3b and P4 became;
  [0006](0006-ontology-scale-and-the-prompt.md) holds the prompt budget;
  [0009](0009-no-type-is-a-type.md), [0010](0010-no-relation-is-no-relation.md) and
  [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md) each overturn one claim below;
  conventions in [README](README.md)

> Reasons outlive lists. The cross-cutting criteria come before the phases, and overturned judgments
> stay on the page.

## The problem

The product's differentiation is time and trust, and both sit downstream of the ontology: the
extractor decides what to extract and which class it lands in; the temporal engine reads `functional`
to decide what counts as a contradiction; resolution uses type to decide who may be the same entity.
A wrong ontology bends everything below it, silently.

Enterprises already have ontologies (FIBO, industry standards, Protégé models), and rebuilding one
class at a time in a UI is unrealistic. So the target is: import the enterprise ontology, extract in
its vocabulary, and let the governance work leave a trace.

Baseline, measured 2026-08-28 on `Industry Corpus` (18 NVIDIA blog posts + 10 Microsoft newsroom
items): 28 documents / 181 chunks, 917 entities / 922 facts, 5.07 entities per chunk, cross-document
resolution holding. Product was 40% of entities and about a quarter of a sample were generic phrases
("row power center"). `part_of` marked `functional` had produced 59 false conflicts. The bench has
since moved to `scripts/bench/` (0012); these numbers are real but no longer comparable.

## Decisions

### Cross-cutting criteria

1. **Keep the original; project what we consume.** Discarding is irreversible, storing is nearly
   free. The imported file is kept verbatim and the projection covers only what has a consumer
   today. Corollary: because the original is kept, the projection need not be semantically complete;
   a simplification is a UI or prompt trade-off, never data loss.
2. **The ontology guides which types take part.** A declaration can be wrong (`part_of` was), so the
   ontology shapes the prompt and candidate ranking and drives no discard, overwrite or rewrite: a
   wrong guide is overridden by the text, a wrong gate destroys data systematically. **Argument
   order is enforced** (0012): it is the key's encoding convention, no claim about the world. A
   subject that violates the domain while the object fits is swapped and marked
   `direction_corrected`; if the swap is also illegal the predicate is dropped and subject, object
   and evidence are kept. Five rounds measured: violation rate 57% → 4%, true reversals 39 → 0.
3. **Prompt size is decoupled from ontology size.** N classes in every chunk's prompt costs
   O(chunks × ontology), and a model choosing among 800 options chooses worse. Scale is solved with
   retrieval.
4. **A shared name carries no conclusion.** Same entity: one type already. Different entities: the
   name is exactly the signal we distrust (two people called 张伟). Context carries the information.
5. **Uncertainty surfaces to a person.** Tiered thresholds, gray zone into Review; same root as
   resolution's "keep apart when unsure".
6. **Governance experience is fixed in the schema.** Changing a class description is visible,
   reviewable and audited; a rule grown out of clicks is a mystery six months later.
7. **Uniqueness and identity are different things.** `key` is `UNIQUE (kb_id, key)` but derived from
   a label that changes; identity must hold across time, so it needs a "where from": the IRI. An IRI
   is a name and never an address: never fetch it, never normalize http/https or trailing slashes.

### P0 · Editable entities (built)

8. `PATCH /kbs/{id}/entities/{entity_id}` (Editor role) changes `type_id` and `canonical_name`; the
   audit ledger gets `entity.retyped` / `entity.renamed` with before and after snapshots; the entity
   panel has the edit entry. Before this the only repair was re-extracting the whole KB.
9. **A name collision is a prompt.** Two 张伟 of the same type are the intended product of "keep apart
   when unsure"; `entities_kb_type_name_idx` is a plain index and duplicates already exist. On rename
   or retype the UI says "an entity with this name exists, merge?" and lets the user continue. A
   unique constraint or a 409 would break resolution's ground.

### P1 · No silent drops (built)

10. The extractor used to drop facts with no trace, which contradicts an append-only ledger, evidence
    on every fact, and uncertainty in front of a person. Table `extraction_drops (kb_id, document_id,
    reason, detail, count, example, updated_at)`, primary key on the first four, cleared per document
    when extraction starts. Eleven reason codes today, including `subject_not_declared`,
    `attr_domain_mismatch`, `truncated_reply`, `malformed_item`, `not_an_entity_name` and
    `direction_corrected`; the Library shows "N facts did not land" per document.
11. It is separate from `ontology_misses`: misses say "your ontology lacks these" (read by whoever
    evolves the ontology), drops say "these facts did not land" (data completeness). Clearing per
    document also fixes a lifecycle bug: misses were cleared only on a full rebuild.

### P2 · OWL import (built)

12. **Three layers.** The file goes verbatim into the blob store (the content-addressed ingest path)
    and `ontology_imports` records kb, blob sha, filename, time and projection version. The
    projection into `entity_types` / `relation_types` is a re-runnable derivation, redone when a new
    consumer appears with nothing asked of the user. "We cannot express it" thereby becomes "not yet
    projected".
13. **Mapping.** `owl:Class` → entity type; `rdfs:subClassOf` → `entity_type_parents`; `rdfs:label` →
    `label` (`@en` / `@zh` preferred); `rdfs:comment` → `description`, load-bearing because it enters
    the extraction prompt and is what P3's retrieval matches on; `owl:ObjectProperty` → relation and
    `owl:DatatypeProperty` → attribute (`relation_types.kind`); `rdfs:domain` / `rdfs:range` →
    `relation_type_domains` / `relation_type_ranges`, stored as signals, never as gates;
    `owl:FunctionalProperty` / `InverseFunctionalProperty` → the two flags; the IRI → `iri` column,
    `UNIQUE (kb_id, iri) WHERE iri IS NOT NULL`. Every other axiom stays in the original and is
    reported as not yet projected (P5 has the current list).
14. **IRI is identity, key is the model's token.** `key` is `[a-z0-9_]`, at most 40 characters, what
    the prompt lists and the model returns; an IRI cannot fit there. A key cannot be identity either:
    on re-import `hr#Employee` relabelled "Staff Member" would orphan `employee` or be matched on the
    label that just changed; with the IRI it is one `WHERE iri = $1` and no entity moves. Keys from
    local names fail the same way (`foaf:Person` and `acme:Person` both want `person`). Hand-made
    types have `iri = NULL`.
15. **Multi-valued domain and range** live in link tables and are checked through the subclass DAG.
    RDFS treats several `rdfs:range` on one property as an intersection; the importer takes the more
    specific when one subsumes the other and otherwise reports it as not projected. Intersection
    itself is not implemented: it is the first step into class expressions, and the projection only
    serves prompt and UI (criterion 1).
16. **Domain and range are a type signature in the prompt**: `works_at (person → organization)`. It
    corrects "Alice works_at Seattle" at generation time. Only classes laid out in the prompt are
    named, a side with none selected falls back to `*`, and the signature uses keys, never labels: a
    Chinese KB's label 「人物」 would teach the model a type that does not exist.
17. **Preview before commit.** Upload → dry run (new / updated / not projected counts, which relations
    turn on conflict detection through `functional`, how many classes lack `rdfs:comment`) → confirm.
    A key collision between two IRIs is reported and left to the user; a local row without an IRI
    (seed or hand-made placeholder) is claimed by the vocabulary and takes its shape (#78, #145).
18. **Individuals (ABox) are never imported as facts.** Every fact needs an evidence chain; instances
    stay in the stored file as future background knowledge for the reasoner.
19. **Parser**: `oxttl` + `oxrdfxml` in `utopia-ingest`, Turtle and RDF/XML only; no `horned-owl`, no
    reasoning, no export. Validated on FOAF (635 triples) and DCTerms (700 triples). The file
    extension is the strong format signal; content overrides it only when it is clearly Turtle.
20. **Attributes** (`create_attribute_with_iri`): `rdfs:range` → `datatype` three ways. A mappable XSD
    type gets its datatype; no range → `text`, listed in the preview; a type we cannot express
    (`time`, `gMonth`, `duration`, several ranges, unknown IRIs) → `text` and reported; a type the
    extractor could never read out of prose (`base64Binary`, `hexBinary`, `XMLLiteral`, `QName`,
    `ID`, `IDREF`, `ENTITY`) is skipped and reported. The dividing line is "can this value appear in a
    sentence": skipping protects the prompt, where every attribute is a line paid per chunk. A domain
    pointing at a class that was not imported is skipped and counted.
21. **Multiple inheritance is real.** FOAF's `Person` is both `foaf:Agent` and `geo:SpatialThing`;
    keeping one parent makes attributes on the other branch fail their domain check.
    `entity_type_parents (child_id, parent_id, is_primary)` replaces the old single column; ancestry
    is breadth-first with a visited set; creating or editing a type checks for cycles; the left pane
    still draws a tree by primary parent, because a class shown twice reads as two classes.
    `update_relation_type` may change domain and range (a signature is no identity); `kind` stays
    immutable.

### P3 · Type resolution and the prompt budget

22. **A character budget decides how much ontology enters the prompt.** Under
    `ontology_prompt_budget` (24,000) the whole ontology is listed, which is how every small KB
    works. Over it, each chunk retrieves its top-K classes, relations and attributes (40 / 30 / 30,
    untested) and the ancestors of every hit are laid out with it. Attributes expand under their
    domain class, so a pruned class prunes its attributes. Details in 0006.
23. **P3a · Type resolution** (built; runs from the Ontology page as preview → apply → undo).
    Extraction returns a coarse type plus `specific_type`: free text, unvalidated, never written to
    the ontology. Without it 17 of 17 `proposed_type` were empty, because the list always had
    something close enough (`product` chosen, "vector database software" lost); with it the task
    becomes "which class has this name", and retrieval hits went from 4/17 to about 13/20.
    Candidates come from two routes, the profile against class descriptions and the context vector
    against typed entities whose classes vote, taken alternately and never merged by score. Tiering
    asks "is the chosen class inside the coarse type's subtree" (one level down → automatic; another
    axis → a person), acknowledged per `(coarse, target)` pair in `type_refinement_pairs`. Every
    `left_alone` carries a reason. A retype is an UPDATE on `entities` plus an `entity_retypes` row,
    and entity history reads `facts` only, so a wrong retype does not show itself; hence preview
    before apply.
24. **P3b · Surface predicates** (built, in a different shape). A predicate is the fact itself, so it
    cannot be deferred to "empty" the way a type can; it is deferred to the surface phrasing, stored
    on `fact_evidence.proposed_predicate`, one per observation, written on both paths (the phrase that
    failed to map, or the model's actual word on a hit, which may be an alias). `facts` deduplicates
    on `(kb_id, subject_id, predicate_id, object_id)`, so a column there would be first-writer-wins.
    Mapping back is `predicate_match` plus 0003's adoption loop; a phrasing that keeps missing stays
    an ontology signal.

### P4 · Governance as experience

25. **A human decision is never forgotten** (built, #114). `entities.type_source` is `extracted` /
    `human` / `inferred`, guarded on four paths: type-resolution sampling, class claiming
    (`adopt_proposed_types`), extraction upgrade, and retype (an `actor` makes it `human`). The real
    leak was type resolution's sampling rule "current class still has subclasses → include", which
    re-judged entities a person had settled; extraction was already guarded (`resolve_type_drift`
    upgrades only when `type_key.is_none()`). Since 0009 "no type" can itself be a human decision.
26. **Decision memory as retrieval** (pending). Store the retrieval vector and outcome of a human
    retype and hand the nearest past decisions to the judge as evidence, generalizing by context
    (criterion 4). `resolution_verdicts` is half of this pattern but exact on `pair_key`. Waits for
    an evaluation corpus: it is the one item whose benefit cannot be judged without one.
27. **Corrections aggregate into ontology signals** (pending). "Product → Concept 37 times in 30
    days" means Product's description is too loose; the Ontology page should show it with samples
    and "draft a stricter description", like the misses panel. Source is `audit_events`
    (`entity.retyped`, `ontology.refinement_approved`, both with an actor); `entity_retypes` is the
    undo ledger and includes the engine's own retypes.

### P5 · Reasoner

28. Delivered by 0002. Projected today: `TransitiveProperty`, `SymmetricProperty`,
    `AsymmetricProperty`, `IrreflexiveProperty`, `FunctionalProperty`, `InverseFunctionalProperty`,
    `disjointWith` (`entity_type_disjoint`), `inverseOf`, `subPropertyOf`. Still not projected:
    `equivalentClass`, `someValuesFrom` and class expressions, property chains. `disjointWith` is
    consumed only by R0's ontology self-check; `classify_type_drift` still uses the hardcoded
    `CONFUSABLE_TYPE_KEYS` (0009 named this).

## Dead ends

- **A unique index on entity names, 409 on rename.** Duplicates already existed, and two people with
  one name are what "keep apart when unsure" is for.
- **"Range validation on relations" as P1.** No relation had a range and only two had a domain; the
  UI never wrote one. Range can only come from import.
- **Auto-suffixing a colliding key.** The next re-import cannot tell `person_2` from `person`; the
  IRI argument in reverse. Report instead.
- **Sniffing the format from content.** Hand-written OWL samples all passed and real FOAF died on
  line one: the check was inverted and a leading `<!--` pushed `<rdf:` out of the window. The
  extension decides; content overrides it only when clearly Turtle.
- **Waiting for an IRI → id map before importing attributes.** `id_of` already existed from parent
  resolution; only the range → datatype mapping was missing.
- **Skipping attributes whose range we cannot map.** Two arguments fell in turn: "text would be
  blocked by `attr_datatype`" (text is never blocked) and "downgrading loses the declaration
  silently" (the preview reports it). A skipped attribute is never told to the extractor, so the
  knowledge is never captured; the end-to-end check produced `opens_at 10:00` (downgraded `xsd:time`)
  that the skip rule would have lost.
- **"List only the base classes" for large ontologies.** schema.org measured 1010 classes / 1676
  properties, 109,083 tokens per chunk, classes only 38% of it. Six base classes would remove a
  40-class user ontology (about 2k tokens, no problem) from extraction, hand the model an escape
  hatch as `related_to` did, and lose facts at extraction time that a later retype cannot recover
  (`attr_domain_mismatch: position@person`); coarser types also make entities look alike, and the
  resulting merges write the ledger.
- **A class-count threshold (about 30) for enabling type resolution.** Never existed; the axis is
  the character budget. "Seed base classes always present" died with the seeds (#128); ancestor
  fill-in replaced it.
- **Merging retrieval scores.** Distances are incomparable across entities (清华大学计算机系 →
  `computer_store` at 0.46 beat 星云科技 → `corporation` at 0.59), across the two routes, and between
  two queries on one route: a short name query scores systematically lower than a whole profile and
  pushed out three correct answers.
- **The model's self-reported confidence for tiering.** Bimodal (15 at ≥ 0.85, 4 null, nothing
  between): tone, no probability.
- **The subtree test as a risk measure.** On the second corpus 14 of 24 were "cross-axis" and all
  correct: the test measured whether seed classes were connected to the imported taxonomy
  (`location` had zero subclasses, schema.org's `Place` had 209). Per-pair acknowledgement mitigates;
  the cure is ontology alignment, larger than this step.
- **`profile_embedding` for class retrieval.** It is the centroid of the entity's chunks, "what
  documents it appears in", so a person in GPU launch posts smells of GPUs. Re-embed a synthetic text
  (name + predicates + coarse type).
- **Keeping `related_to` as honest vagueness.** "I don't know" is honest; a relation named "related
  to" on the graph is an assertion. Of 359 such facts 321 were the model choosing it from the list,
  so the hatch was removed from the prompt, then the relation deleted (0010).
- **Upgrading types on the extraction job chain.** Extraction only enqueues `bootstrap_ontology` and
  `adjudicate_entities`; the retrieval hit rate was too low to run unattended.
- **"Re-extraction silently eats governance" as P4a's premise.** Extraction was already guarded; the
  leak sat in type-resolution sampling. Looking in the wrong place cost more than looking nowhere.
- **Server-produced display text.** A Chinese fallback `（手工建的）` leaked into the English UI, and a
  frontend type declared `created_at` for the server's `imported_at` and rendered `Invalid Date`.
  Wording belongs to the interface; cross-language seams get checked with real data.

## Revisions

- 2026-09-02: assumed `type_id` was NOT NULL and the coarse type always the first type; 0009 made it
  nullable, and "no type" may be a person's decision.
- 2026-09-02: assumed argument order was guidance like everything else; 0012 made it enforced.

## Open questions

- Upgrade accuracy has no number; it needs a real enterprise ontology for a small-sample evaluation,
  and P4b waits behind it.
- The 24,000-character budget and the 40 / 30 / 30 per-chunk counts are untested; the measurement
  is to raise the inlined class count from 12 towards 968 and watch fact count and latency.
- Type resolution runs only when someone clicks: under a large ontology, refinement of new entities
  depends on a person remembering to.
- `disjointWith` pruning of merge candidates on the resolution side is not done.
- The `active` flag has only a governance use left (retired classes take no new entities); whether
  it is worth building waits for a real large ontology.
