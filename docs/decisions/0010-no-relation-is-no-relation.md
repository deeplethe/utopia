# 0010 · An unnamed relation stays empty

- **Status**: Implemented · `facts.predicate_id` is nullable, `related_to` is gone, display
  falls back to the source's wording through `fact_surface_predicate(uuid)`, guarded by
  `no_predicate_still_shows.rs` · both `mapped_to` follow-ups done with
  [0011](0011-a-mapping-is-not-a-fact.md) (#126); the seed table and
  `ensure_default_ontology` left with #128 · the two pieces of dead code noted 2026-09-02 are
  cleared
- **Written**: 2026-08-30 (undated in the original; the day its migration ran) · condensed
  into English 2026-09-03
- **Related**: [0009](0009-no-type-is-a-type.md) the twin on the type side;
  [0001](0001-ontology-import-and-governance.md) holds the sentence this record overturns;
  [0011](0011-a-mapping-is-not-a-fact.md) takes `mapped_to` the rest of the way

## The problem

`related_to` encoded "the extractor found an edge but the ontology has no matching relation"
— a program state. Yet it sat in `relation_types` as a `builtin` relation, listed on the
ontology page beside `acquired` and `works_at`, as though someone had decided the relation
between the two things is called "related". Nobody had.

The cost was measured before deleting it: in one 348-chunk KB, 533 facts hung on it and
every one displayed as 有关联, while every one also carried the source's wording in
`fact_evidence.proposed_predicate`. The meaning was in the database all along, covered by a
fake vocabulary word.

## Decisions

1. **`facts.predicate_id` becomes nullable and `related_to` is deleted.** The reader sees
   more: on a real corpus (14,706 facts, 5,934 on `related_to`), 93 graph edges that all
   read "related" now read `placed_pressure_on` / `agreed_to_resign` / `countersued` /
   `revived_legal_action` / `license_from`.
2. **Display falls back to the source's wording through one SQL function,
   `fact_surface_predicate(uuid)`.** The most frequent wording wins, ties by lexical order —
   the rule `predicate_match::merge_key` uses, and deterministic, so one edge has one name
   on the graph, in the panel and in the history. Only 3.0% of facts (16 of 533) had more
   than one wording. One function rather than a subquery in each of the eight read paths:
   eight copies diverge, and divergence here means different names on different pages.
3. **The reader must see where a word comes from**, or the lie is retold another way. Rows
   carry `inferred`: edges outside the ontology are drawn in a lighter gray; the panel and
   document output say on hover "not in the ontology; this is the source's wording"; facts
   with neither show an italic "unnamed relation" — never a made-up word.
4. **Data changed in place, no migration path.** The database held test data before release.
   Once users have run for half a year, 0001's "only new extractions; rebuild the backlog"
   applies again.
5. **`mapped_to` follows.** Only `related_to` was excluded from the prompt, so the model
   used `mapped_to` (entity → data-source schema) on entity↔entity pairs 41 times. It leaves
   the prompt, then `relation_types` altogether ([0011](0011-a-mapping-is-not-a-fact.md)):
   the same error, control flow written as vocabulary. The `FALLBACK_RELATION_*` constant
   and branches, the `extraction_drops` reason "no fallback relation", and the "fallback
   predicate" wording went with it.

## Dead ends

- **0001's sentence "`related_to` is honest vagueness; a wrong guess is confident error."**
  The second half stands, the first is wrong. Staying at "I don't know" is more honest than
  guessing — but implementing that honesty as a word in the ontology turned it into an
  **assertion**: a relation displayed as "related" claims such a relation exists. The honest
  expression is empty, next to "the source said `acquired`". 0001's aside "(kept in the
  ontology as the code-level fallback)" is void: the fallback now happens at read time.
- **Twenty inner joins the compiler cannot see.** Once the column is nullable, every `JOIN
  relation_types` on a read path is a silent filter; `cargo check` and clippy say nothing —
  the same three-valued trap as 0009's `NULL <> uuid`. Eleven became `LEFT JOIN` plus
  fallback; six were already right (four filter by `r.key`, two read `functional` /
  `temporal`). Three misses were caught by the database and none by the compiler (an
  `r.label` without COALESCE, an `f.id` referenced outside its CTE, a stale `rt.id`), plus
  two mis-numbered placeholders — the eighth time "SQL is invisible to the compiler" in this
  repository, so DB tests are not optional when SQL strings change. The most expensive miss:
  `fact_snapshot` through an inner join returned `None`, so rejecting a predicate-less fact
  would have left no ledger entry — and the ledger exists so the record survives the fact.
- **420 legacy exceptions.** Of 5,934 fallback facts, 420 have no source wording, all from
  the two oldest KBs (`Industry Corpus` 275, `General` 145), before `add_evidence` recorded
  `proposed_predicate` unconditionally. They now display empty; they used to display
  "related" — zero information either way.
- **End-to-end on the full clone**: 5,934 facts go NULL and 5,514 recover their wording;
  graph 457 edges — 364 ontology, 93 wording, 0 unnamed; rejecting a predicate-less fact
  leaves a self-contained snapshot in the decision ledger. `no_predicate_still_shows.rs`
  guards the line: a fact with no predicate must be visible on every read path, and turning
  any `LEFT JOIN relation_types` back into `JOIN` makes it red (both controls verified).

## Revisions

- 2026-08-30: we deleted the data, not the code, and it grew back in seven minutes.
  Migration 0052 deleted the `relation_types` **row**; the seed was code
  (`DEFAULT_RELATION_TYPES`), and `ensure_default_ontology` ran at KB creation, on the
  ontology page and at every extraction — 0052 applied at 17:54, `bench demo-autoextend`
  re-created a `builtin=true` row at 18:01. Worse, 0052 also dropped the prompt's exclusion
  filter on the premise "no row now, so the escape hatch and the reminder disappear
  together"; the premise was false, so `related_to` was listed in the prompt for the first
  time — and 0001 had measured that of 359 uses, 321 were the model picking from the list.
  The two must change together; changing one is worse than neither. (#128 removed the
  function and the seed table; regrowth is now impossible.)
- 2026-08-30: the guard assertion was empty. `SELECT count(*) FROM relation_types WHERE key
  = 'related_to' AND builtin` passed because the fixture built the KB with raw SQL and never
  called `ensure_default_ontology` — asserting on bare ground. The test now seeds first, and
  the control was verified (adding `related_to` back to the seed table turns it red).
- 2026-09-02: two pieces of dead code. `graph.rs` `confirmed_mappings()` had zero callers
  (chat reads `mappings::confirmed`), and the `r.key = 'mapped_to'` join in `confirm_fact`
  matched zero rows. Both removed since (0016 A3).
