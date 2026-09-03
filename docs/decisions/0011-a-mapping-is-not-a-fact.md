# 0011 · A mapping is configuration

- **Status**: Implemented · `concept_mappings` table and wiring (#126, the same commit as
  this record), a standalone Data Mappings page (#140), moved out of the Review queue (#148)
  · of the three things to rebuild, the Review flow and history are done, the evidence chain
  is not · one of two open questions answered (2026-09-02 check)
- **Written**: 2026-08-31 · condensed into English 2026-09-03
- **Related**: [0009](0009-no-type-is-a-type.md) removes the builtin entity classes,
  [0010](0010-no-relation-is-no-relation.md) the fallback relation, #125 the other eight
  seed relations — this record is the last step on that line: **only vocabulary remains in
  the ontology**

## The problem

The semantic layer stored "business concept → data asset definition" as a fact: subject a
concept entity (Metric / Dimension), predicate `mapped_to` — a row in `relation_types`
beside `works_at` — object an `object_value` JSON `{ source, table?, expr?, sql?, unit?,
summary? }`, confidence 0.6 proposed / 1.0 confirmed. `review_routes` picked it up as a
low-confidence fact (< 0.75), Confirm set 1.0, and `chat.rs` read `confirmed_mappings` (≥
0.75) into the query prompt.

The reuse was deliberate and bought three real things: a Review flow with zero new UI,
bitemporal history for free, and an evidence chain like any other fact's. Splitting means
rebuilding all three.

## Dead ends

- **Keep the mapping as a fact.** Rejected on four grounds:
  1. It is not an assertion about the world. The ontology answers "what exists and how
     things relate"; `mapped_to` answers "how this number is computed in our database". The
     third application of one sentence: 0009 (`concept` is control flow), 0010 (`related_to`
     is control flow), here `mapped_to` is configuration.
  2. It appeared where it should not: the ontology page listed it, the extraction prompt
     offered it, ontology size counted it, and after #125 it was the **only** seed relation
     — a new KB's ontology held exactly one thing, unrelated to the graph the user wants.
  3. "Confirm" already broke the ledger's foundation: `UPDATE facts SET confidence = 1.0` in
     an append-only table where a correction inserts a new row with `supersedes`, because
     the change of belief is itself information (0001 P0). Confirming changes not our belief
     but whether a configuration is in effect; it only worked because that state happened to
     fit in one float.
  4. The shape was wrong. The real fields `source / table / expr / sql / unit / summary` sat
     inside JSONB: unqueryable ("which concepts map to `orders`"), unconstrainable
     (uniqueness per (concept, source) hidden in JSON, closed by process rather than by the
     database), unexplainable (`mapped_to` has no range — the object is neither an entity
     nor a datatype).
- **Bitemporal history for the new table.** A definition has one axis, "when it took
  effect"; forcing the ledger's two axes would move complexity rather than solve it.

## Decisions

1. **Its own table, `concept_mappings`**: concept entity id → definition, fields as columns,
   `UNIQUE (kb_id, concept_id, source)` (a constraint; `id` is the primary key), plus a
   `derived` column for derived metrics. `mapped_to` leaves `relation_types`,
   `DEFAULT_RELATION_TYPES` empties, and a new KB seeds no relations at all.
2. **Review without borrowing the low-confidence tier.** A proposal is configuration, not a
   knowledge assertion. Built first as its own Review group, then (#140) as a standalone
   Data Mappings page, with Review keeping only a count and a link (#148: a mapping is
   neither a queue nor a history). The approval endpoints stay in `review_routes`, where the
   `mapping.decided` audit stream lives.
3. **History as revisions.** `created_at / confirmed_at / confirmed_by` on the row, full
   snapshots in `concept_mapping_revisions` written by `revise()`, and a History drawer on
   the page.
4. **Evidence recorded separately** — a mapping's evidence is "which table's schema the LLM
   read", not "which sentence". Not built: `concept_mappings` has no such column, and
   exploration proposals leave no evidence.
5. **No data migration.** Mock data only; the old `mapped_to` facts stay in the ledger
   unread (as with #125). After release this convenience is gone.
6. **Two things that landed unplanned.** `propose()` uses `ON CONFLICT … DO UPDATE … WHERE
   status = 'proposed'`, so rerunning exploration does not erase a human rejection (when the
   `WHERE` fails, `RETURNING` yields zero rows). Visibility is tiered: a Viewer can read the
   list, `revise` needs Editor — seeing the answer without the definition means trusting an
   algorithm you are not shown.

## Revisions

- 2026-09-02: the status line said "planned" while the record arrived in the same commit as
  its implementation. The status line is the implementing PR's responsibility; now a README
  rule.
- 2026-09-02: we assumed a Review group would be the mapping's home; in use it became its
  own page (#140, #148).

## Open questions

- **A confirmed definition changes.** Answered by `revise` + `concept_mapping_revisions` +
  the History drawer: "last quarter's definition" can be read back. Whether querying uses it
  to replay historical reports is not built.
- **One concept, several sources.** `(kb_id, concept_id, source)` allows a different
  definition per source, on purpose. Which one does querying use? Today all go into the
  prompt (cap 30) and the model picks; rules are easier to add now that it is a table.
