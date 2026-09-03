# 0009 · An undecided type stays empty

- **Status**: Implemented · `entities.type_id` and `entity_retypes.from_type_id` are
  nullable; the nine builtin classes and the seeding function left with the seed relations
  (#110 / #125 / #128 / [0011](0011-a-mapping-is-not-a-fact.md)) · `owl:disjointWith` now
  reaches resolution (0016 B3): a declared disjointness, inherited down both hierarchies, keeps
  same-name entities apart ahead of every heuristic; since #226 same-name entities of kin classes
  (ancestor, descendant or a shared non-root ancestor) go to Review; `CONFUSABLE_TYPE_KEYS` stays
  as the fallback when nothing is declared · `metric` /
  `dimension` are created on demand by mapping exploration (#231); a semantic-layer pack is
  still planned
- **Written**: 2026-08-30 · condensed into English 2026-09-03
- **Related**: [0008](0008-ontology-packs-as-cold-start.md) makes a real vocabulary the
  optional start; [0001](0001-ontology-import-and-governance.md) IRI/key split behind the
  collision argument; [0010](0010-no-relation-is-no-relation.md) the twin on the relation
  side

## The problem

A new KB used to seed nine entity classes: `person organization project metric dimension
product event concept location`. None carried a signature (0008), and once schema.org is
installed `Person` / `Organization` / `Product` / `Event` / `Project` are claimed by key, so
the builtin set was a placeholder. `location` was worse: schema.org calls it `Place`, the
keys differ, so `City` / `AdministrativeArea` hung under a separately created `place` — a
name of our own cut the geographic subtree in two.

`concept` was different: it was **control flow**. `extraction.rs` required it ("Ontology
missing the 'concept' type") as the landing place for entities whose type is not in the
whitelist — a state that exists under any ontology — and as the candidate pool for type
resolution (`entities_for_type_resolution(kb, DUMPING_GROUND, …)`). A "not decided yet" sat
in the ontology as a class, as if someone had decided it.

## Decisions

1. **Remove all nine classes; a KB without packs is truly empty.** Extracted entities get
   `type_id` NULL, facts and evidence land as usual; install a pack later, rerun type
   resolution, and they are assigned — `entities_for_type_resolution` now takes `type_id IS
   NULL`. "Build the KB first, model later" is a supported path; schema.org is pre-checked
   in the create dialog. Landing added `entities.type_source` (extracted / human / inferred)
   to separate "not decided" from "a human decided there is none".
2. **`entities.type_id` becomes nullable, no sentinel row.** The Rust compiler lists every
   consumer of the new `Option`. SQL does not: `NULL <> uuid` evaluates to NULL, so `AND
   type_id <> $2` selects **no rows and raises nothing**. Hit twice on the main path —
   `adopt_proposed_types` and `retype_entities`, whose inputs are almost all `type_id IS
   NULL` — running silently empty with green tests. The fix is `IS DISTINCT FROM` (measured:
   `<>` selects 0 rows, `IS DISTINCT FROM` 1). The real cost was rereading every comparison
   in 26 SQL strings; the change touched 14 files, `+316 / -369`.
3. **Untyped same-name entities may coexist.** The unique index `(kb_id, type_id,
   lower(canonical_name)) WHERE merged_into IS NULL` does not stop two untyped "张三", because
   NULL ≠ NULL. Intended: the index allows same name across types (0001 P0: two 张伟 must be
   storable apart), and untyped we know even less, so there is even less reason to merge.
   Recorded so nobody files it as a bug.
4. **Drawing it.** The node query switched from `JOIN entity_types` to `LEFT JOIN` — an
   inner join makes an untyped entity vanish from the graph while its facts stay, the
   hardest data loss to notice. `key` and `label` stay NULL; color and shape get defaults
   (gray dot: the canvas must receive something). Review items likewise.
5. **First typing is a completion.** Type resolution treats "chosen class outside the
   current subtree" as reclassification and sends it to a human; an untyped entity has no
   extraction judgment to overturn, and judging it cross-axis would put every entity in
   front of a person. `entity_retypes.from_type_id` is nullable accordingly — the commonest
   retype is none → some, and that table is the only basis for undo.
6. **`CONFUSABLE_TYPE_KEYS` stays** (`["organization","project","product"]`): the keys still
   exist after schema.org, from import rather than seeding; without packs the tier never
   hits and every cross-type same-name pair is `Disjoint` — stricter, no wrong merges. The
   proper fix is to read `disjointWith` from the ontology.

## Dead ends

- **Keep the sentinel, rename its key to `_unclassified`** (first draft). It would have
  held: `key_from_iri` never emits a leading underscore (measured: `skos:Concept → concept`,
  `http://x#_Concept → concept`, `http://x#__unclass → unclassified`), so the namespace is
  unreachable by import. And a rename was necessary: the sentinel had no IRI, import claims
  IRI-less placeholders, so `skos:Concept` would take over the sentinel and every undecided
  entity would silently become a real `skos:Concept`. But renaming fixes collision, not
  **leakage**: the sentinel must be filtered from every consumer — ontology page, prompt,
  candidate lists, legend, export, statistics — and one forgotten `WHERE key NOT LIKE '\_%'`
  fails silently. A naming convention in place of a type guarantee, the defect class
  `ontology_index.rs` warns about ("self-heal, don't hang hooks").
- **"NULL cannot be forgotten — SQL and the type system will tell you."** Half right; the
  SQL half is decision 2.

## Revisions

- 2026-09-02: `owl:disjointWith` now has a table (`entity_type_disjoint`), import and an
  edit endpoint, but its only consumer is R0's ontology self-check; `CONFUSABLE_TYPE_KEYS`
  is still three hard-coded keys (0016 B3).
- 2026-09-03: resolution reads the table (0016 B3). `declared_disjoint_from` collects the
  classes declared disjoint with the mention's class or any ancestor, expanded to their
  descendants, in one recursive query; both cross-type paths (type drift and containment)
  treat a hit as `Disjoint` before the kinship check and the hard-coded list. Nothing
  declared → today's behavior, unchanged. The list is now a fallback, and decision 6 reads
  accordingly.
- 2026-09-02: `metric` / `dimension` had a visible cost — mapping exploration looked up
  `entity_types.key IN ('metric','dimension')` and `continue`d when missing, so a KB without
  those classes silently produced zero mappings. 2026-09-03: exploration now creates both as
  builtin types before exploring (#231); a semantic-layer pack with IRIs is still planned
  (0016 D2).

## Open questions

- **Resolving untyped entities.** The profile has no type dimension, so
  `classify_type_drift` lacks one criterion. Two untyped same-name entities go `Recall` and
  rely on profile similarity; whether that is enough is untested.
- **Where `metric` / `dimension` belong.** Today they are builtin-on-demand. A "Utopia
  semantic layer" pack would move the last builtin classes out of code into something
  optional, with IRIs, replaceable (0016 D2).
