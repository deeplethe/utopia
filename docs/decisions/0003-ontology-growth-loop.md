# 0003 · The ontology grows out of the corpus

- **Status**: Built and running (#44; switch `knowledge_bases.auto_extend_ontology`, default on). Its
  starting point has moved: `related_to` is deleted ([0010](0010-no-relation-is-no-relation.md)) and
  the seed ontology retired (#125, #128), so a new KB starts from an ontology pack
  ([0008](0008-ontology-packs-as-cold-start.md)) and this loop fills the gaps a pack leaves.
  "Dismissal has memory" redone per [0007](0007-who-decides-what-becomes-a-relation.md). Two of three
  known gaps closed (`adopt_proposed_types` + `entity_retypes`; `ontology_proposals`, #112); the "new
  phrasings since you last looked" reminder still pending. Checked against the code 2026-09-02.
- **Written**: 2026-08-28 · condensed into English 2026-09-03
- **Related**: [0001](0001-ontology-import-and-governance.md) P3b and P4 are the plan, this is what
  grew; [0007](0007-who-decides-what-becomes-a-relation.md); [0008](0008-ontology-packs-as-cold-start.md);
  [0010](0010-no-relation-is-no-relation.md); [0012](0012-the-ontology-is-a-contract-not-a-suggestion.md)

## The problem

The extractor reads "《星球大战：零号连队》可在 GeForce NOW 上玩", the ontology has no "playable on", and
the fact lands as `related_to`, which says nothing. On real corpora 40.5% of edges were like that, and
every new KB began with ten seed relations nobody had chosen. The extraction side (0001 P3b) removed
the escape hatch and kept the original phrasing on `fact_evidence.proposed_predicate`. This record is
the other half: turning kept phrasings back into relations, with a person somewhere in the loop.

That world is gone: an unmapped fact now has a null predicate and shows its phrasing through
`fact_surface_predicate()`, and a new KB has no relations until a pack is installed. 0012 measured
25.6% null predicates on ai-timeline-ends × schema.org, all 41 relations from the pack and none grown
by this loop. The loop itself is unchanged and still on by default.

## Decisions

1. **Adoption rewrites the facts waiting for it.** Add used to create the relation type and leave the
   facts saying "related to". Now it also rewrites every fact whose phrasings all fall inside the
   adopted set (a fact with several phrasings across chunks is left alone; under 1% of cases), and
   the proposal says "will reclassify N" with the phrasings merged. The rewrite is an append (new row
   with `supersedes`, old row invalidated), the same path as human correction, so entity history
   reads "recorded as related to, refined to available on, by whom". `predicate_match` runs first: an
   equivalent relation already in the ontology (including `_by` inverses) absorbs the group with
   subject and object swapped (#109). The canonical key is the group's most frequent real phrasing.
2. **Undo is per batch.** `fact_adoptions` records batch, predicate, old fact, new fact, and whether
   the new row superseded or merged into an existing assertion. Undo invalidates the new rows and
   revives the old; the relation type stays (facts pointed at it, and both the adoption and the undo
   happened). Light confirmation, no typed unlock.
3. **Three tiers**: Add one (a person, picking), Add all (a person, one decision), automatic (the
   switch).
4. **The switch governs adoption, never attention.** Off, unmatched phrasings still accumulate in the
   Unmatched panel as proposals, and the copy must say so, since "stop extending" reads as "stop
   noticing". Default-on is justified because every automatic action is visible and reversible: the
   Ontology page banner (`last_auto_extension`) shows what was added, how many facts changed, and
   undo. The audit ledger is for later verification, never a notification.
5. **No axiom is set automatically**, `functional` first and now all of them (`Axioms::default()` on
   the cold-start path). Criterion: a reversible action may be automatic; one that triggers cascading
   writes may not. `functional` drives auto-closing and conflicts, and undoing that means unpicking
   supersede chains; `part_of` mislabelled once produced 59 false conflicts from 28 press releases.
6. **Dismissal remembers an attitude, and counts go on.** `ontology_misses.dismissed_at`;
   `record_miss` keeps counting; suppression happens on read, with dismissed entries listed
   separately with current counts, revocable (0007).
7. **Automatic adoption needs a phrasing in at least two documents**, `MIN_SIGNALS = 3` counted over
   predicate and type signals together (counting predicates alone skipped a corpus that lacked only
   entity types). The reason is quality: the ontology feeds back into the extraction prompt, so one
   document's accidental wording would become a standing instruction.

## Dead ends

- **Opposing automation.** Synonym explosion (four "available" phrasings, four relations), generic
  verbs (`is`, `has`), frequency taken for meaning, a guessed `functional`. The arguments stand; the
  hidden premise "being wrong is expensive" does not, because adoption supersedes and destroys
  nothing. The axis is the cost of an error. What gets automated is approval; synonyms and verbs
  call for clustering, and merging stays human.
- **"Has the ontology been touched" as the trigger.** Inferring intent from behavior fails both
  ways: one click on Add stops the loop, and once false it never turns true again, freezing the
  vocabulary on the first batch while RSS keeps delivering. The declared switch (a user's
  suggestion) removed a bad mechanism.
- **`dismiss` as `DELETE FROM ontology_misses`.** The next extraction re-inserted it. The first fix,
  "stop counting after dismissal", was a one-way door: a judgment made at count 1 applied to twenty
  later documents, count frozen, invisible.
- **A merge branch that made history lie.** When the target assertion already existed, the old row
  was invalidated with no successor and entity history rendered "retracted" for a fact merged intact.
  `fact_adoptions` exists because of this.
- **Triggering when no other document is in flight.** True at once for a batch of one, so a single
  document could set the whole ontology; the two-document threshold fixes it.
- **Automatic merging of phrasings.** A suggestion folded `optimized_for` into `runs_on` ("optimized
  for RTX" is not "runs on RTX"); the merged phrasings were visible in the tooltip and a person
  caught it. This is the direct evidence for keeping merges human.

## Revisions

- 2026-08-30 (#112): proposals persist in `ontology_proposals`; what used to be lost was the
  clustering, the only thing that lets a person check a merge, never the raw misses.
- 2026-09-02: the measured rows (49 rewrites for `available_on`, 24 undone for `supports`, a cold
  start growing 5 relations and rewriting 19 facts) rest on `related_to` and the seeds; history, no
  longer a baseline.

## Open questions

- With the switch off there is no "88 new phrasings since you last looked"; the index
  `ontology_proposals_open_idx` was laid for it and the reminder never built. The only banner today
  is `last_auto_extension`, which reports the last automatic run.
