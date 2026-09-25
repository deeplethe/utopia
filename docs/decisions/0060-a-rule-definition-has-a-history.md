# 0060 · A rule's definition has a history

- **Status**: implemented 2026-09-25 in the PR for #912 · `attribute_rule_versions` (migration 0076), a derivation names the version it was drawn under, the proof and the rules panel read it, `GET /kbs/{id}/rules/{rule_id}/versions` · the export of business-rule bodies that [0020](0020-an-auditor-reads-it-without-us.md)'s revision deferred can now follow
- **Written**: 2026-09-25 (conventions in the [README](README.md))
- **Related**: [0002](0002-reasoning-engine.md) made a derivation keep its record-time lifetime; [0019](0019-the-second-clock-can-be-rewound.md) is the record axis this record extends to rules; [0021](0021-a-rule-reads-attributes-and-concludes-a-type.md) built the business rule as one row; [0030](0030-a-rule-may-read-what-a-rule-concluded.md) keeps a kept conclusion's row and reproves it; [0020](0020-an-auditor-reads-it-without-us.md) (revision 2026-09-25) declined to export rule bodies for the reason this record removes. From #912, out of #902.

> A business rule was one row, and editing it was an `UPDATE`. A derivation pointed at the row. Change a threshold from 3000 to 3500 and materialize: the old conclusions are invalidated and the new ones land, which is right, but the invalidated rows point at a rule that now says 3500. The record axis kept "we concluded this, then it stopped holding" and lost "under which definition". The proof tree, the review card and the export could all say *which rule* and none could say *what it said at the time*.

## What exists

`attribute_rules` holds a business rule's subject class, conclusion, join predicate and, in `attribute_rule_conditions`, its conditions [0021, 0032, 0047]. `business_rules::update` rewrites the row and replaces the conditions. `derived_facts.attribute_rule_id` names the rule; `derived_at` and `invalidated_at` are the conclusion's record time [0002]. `materialize` keeps a still-standing conclusion's row and rewrites its premise links when the reason changed [0030]. `proof()` walks the premises; the export mints `…:rule:{id}` and, since 0020's revision, says the rule's family.

## Decisions

**1. A definition is append-only.** Every edit that changes what a rule says opens a version: a full snapshot of subject class, conclusion (kind and target), join predicate and conditions, with a record time; the previous version is closed with `superseded_at`, never rewritten. Name, description and the enabled switch are a label and a switch, not the definition, and do not open a version. Whether a definition changed is decided by comparing the snapshot JSON, produced by one SQL expression that the migration's backfill and the store share, so a no-op save opens nothing.

**2. A derivation names the version it was drawn under.** `derived_facts.attribute_rule_version_id`, written at materialization from the version the run read. A conclusion that still stands after an edit keeps its row [0030] and moves to the new version, counted as `redefined` in the report: the row's identity is the conclusion, and what it now rests on is the current definition. The rows an edit invalidates keep pointing at the version they were drawn under, which is the sentence the record axis was missing.

**3. The version is read wherever the rule is explained.** The proof carries the version number and its definition; the rules panel shows the version next to the name and opens the history: each version with its record interval, how many conclusions stand on it now, and its criterion and conclusion rendered the way the current one is, with the labels the ids resolve to today. `GET /kbs/{id}/rules/{rule_id}/versions` is the same reading for an integration.

**4. Existing rules start at version 1.** The migration snapshots every rule as it stands, dated by its last edit, and points every existing derivation at that version. Nothing older is reconstructible and nothing pretends to be.

## What this does not decide

- **The export of a version's body.** This record makes it honest to export a business rule's conditions and expressions per version, which 0020's revision deferred; the vocabulary for operands, range bounds and expressions is still #902's second cut and is not chosen here.
- **Restoring an old version.** Editing back to an earlier definition opens a new version with the same content. A "revert" button would be sugar over that and can wait for someone to want it.
- **Versions of axiom declarations.** An axiom is a flag on a predicate, and a derivation already names the declaring predicate and kind; whether declarations need a history is a different question.
