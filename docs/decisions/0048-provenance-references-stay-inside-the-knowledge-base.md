# 0048 · Provenance references stay inside the knowledge base

- **Status**: Proposed 2026-09-20 · implemented in PR #832 (migration 0070), pending review
- **Written**: 2026-09-20 (conventions in the [README](README.md))
- **Related**: [0009](0009-no-type-is-a-type.md)'s "NULL means undecided" is why several edges below are nullable and therefore cannot lean on `MATCH FULL`; [0002](0002-reasoning-engine.md) owns the derivation model whose premise edges are covered here. Design question opened as issue #842.

> The exporter writes `urn:utopia:kb:A:fact:{id}` and asserts the id belongs to knowledge base A. Nothing in the schema made that true — a foreign key proves the row exists, not which base it lives in. A cross-KB `supersedes` would mint a local IRI naming a foreign fact; a cross-KB `type_id` would resolve to nothing and the statement would silently lose its class. This record decides where the same-KB invariant is enforced, and with what.

## The invariant

Every reference an export can resolve must join rows that live in the same knowledge base. One invariant, stated once; the question is only which mechanism proves it per edge, and at which transaction boundary.

## Why the existing foreign keys are insufficient

`FOREIGN KEY (ref) REFERENCES t (id)` checks one column against one key. `kb_id` is not part of the check, so `(kb A).fact → (kb B).entity` is a perfectly satisfied foreign key. The failure is silent in both directions it matters: the exporter mints an IRI that names a row in another base (a complete-looking file pointing at nothing), or a vocabulary reference resolves to a foreign row the vocabulary lookup cannot see (a slice of semantics dropped without an error).

## The mechanism, per edge

Thirty-nine reference edges are protected. The split is decided by a single question: **does the referencing row carry the kb authority itself?**

| Source | Edge → target | kb authority | Mechanism |
|---|---|---|---|
| `chunks` | `document_id` → `documents` | own `kb_id` | composite FK |
| `facts` | `subject_id`, `object_id` → `entities`; `predicate_id` → `relation_types` | own `kb_id` | composite FK |
| `facts` | `supersedes`, `from_statement_id` → `facts` | own `kb_id` | composite FK, `DEFERRABLE INITIALLY DEFERRED` |
| `derived_facts` | `subject_id`, `object_id` → `entities`; `predicate_id` → `relation_types`; `rule_id` → `rules`; `attribute_rule_id` → `attribute_rules` | own `kb_id` | composite FK |
| `entities` | `type_id` → `entity_types` | own `kb_id` | composite FK |
| `entity_type_disjoint` | `a_id`, `b_id` → `entity_types` | own `kb_id` | composite FK ×2 |
| `relation_types` | `inverse_of`, `sub_property_of` → `relation_types` | own `kb_id` | composite FK, deferred, `ON DELETE SET NULL (col)` |
| `rules` | `predicate_id` → `relation_types` | own `kb_id` | composite FK |
| `attribute_rules` | `subject_type_id`, `conclude_type_id` → `entity_types`; `conclude_predicate_id` → `relation_types` | own `kb_id` | composite FK |
| `time_mentions` | `fact_id` → `facts`; `chunk_id` → `chunks` | own `kb_id` | composite FK |
| `type_bindings` | `type_id` → `entity_types` | own `kb_id` | composite FK |
| `phrase_bindings` | `subject_type_id`, `object_type_id` → `entity_types`; `relation_type_id` → `relation_types` | own `kb_id` | composite FK |
| `fact_evidence` | `chunk_id` → `chunks`; `document_id` → `documents` | owning fact's kb | trigger |
| `fact_qualifiers` | `qualifier_type_id` → `relation_types`; `entity_id` → `entities` | owning fact's kb | trigger |
| `fact_derivations` | `premise_fact_id` → `facts`; `premise_derived_id` → `derived_facts` | derived fact's kb | trigger |
| `entity_type_parents` | `parent_id` → `entity_types` | child's kb | trigger |
| `relation_type_domains`, `relation_type_ranges` | `entity_type_id` → `entity_types` | relation's kb | trigger (shared function) |
| `relation_type_qualifiers` | `qualifier_type_id` → `relation_types` | relation's kb | trigger |
| `attribute_rule_conditions` | `predicate_id` → `relation_types` | owning rule's kb | trigger |
| `typed_fact_sources` | `statement_id` → `facts` | owning fact's kb | trigger |
| `statement_qualifiers` | `entity_id` → `entities` | owning fact's kb | trigger |

**26 edges** are declared as `FOREIGN KEY (kb_id, ref) REFERENCES t (kb_id, id)`, replacing the single-column FK that was already there — one kernel-level lookup now proves existence *and* same-base, where before it proved existence and a second check would have had to prove the rest. Eight `UNIQUE (kb_id, id)` constraints on the referenced tables back those keys (`documents`, `chunks`, `entities`, `entity_types`, `relation_types`, `facts`, `rules`, `attribute_rules`).

**13 edges** cannot be expressed that way: their kb authority is a *parent row's* `kb_id`, and the link row has no `kb_id` column at all. A composite key cannot name `fact.kb_id` from a `fact_evidence` row. These keep row-level `BEFORE` triggers — nine functions, ten triggers — that read the parent's base and compare.

**`kb_id` is immutable** on every owned table (one shared function, twelve `BEFORE UPDATE OF kb_id` triggers). Composite keys only guard the rows being pointed *at*; the trigger-covered edges derive their authority from a parent row, and a parent moving base would silently un-anchor every child pointing at it. Making kb reassignment impossible is what lets a point-in-time check stay correct.

**Four same-table self-references** (`facts.supersedes`, `facts.from_statement_id`, `relation_types.inverse_of`, `relation_types.sub_property_of`) are `DEFERRABLE INITIALLY DEFERRED`. A row-level check — immediate FK or `BEFORE` trigger alike — cannot see a target row inserted later in the same transaction, and forward references are a normal restore shape (a multi-row `INSERT` or `COPY` batch lists the referring row before its target). Deferred evaluation at commit sees the whole batch; a sequential write whose target never arrives is rejected at `COMMIT`. One boundary is honest: "the edge is valid when the transaction ends", which is exactly what a restore needs and no more than what an in-order write already had to satisfy.

## COPY, restore, and `search_path`

`pg_restore` loads data with `COPY` and creates constraints after, so any shape that survives the migration survives a restore — and the deferred self-references are what let a same-transaction or intra-statement forward chain restore without ordering games. Two sharper edges were designed for explicitly:

- `pg_restore --disable-triggers` and any `session_replication_role = replica` load suppress *user* triggers wholesale. Declarative foreign keys are internal constraint triggers and are **not** suppressed — so the 26 declarative edges hold even in a replica-mode load, which is a second reason to prefer them wherever the schema can say them. The 13 trigger-covered edges admit the gap and are backstopped by the export-side `provenance_integrity` check and the §0 audit query, which doubles as a post-load audit.
- The migration pins `search_path = pg_catalog` in every function body and qualifies every identifier `public.*`, because restore empties the session `search_path` and a hostile first schema must not redirect name resolution. `migration_0070_runs_under_any_search_path` installs the whole migration under a normal, an empty, and a decoy-first `search_path`.
- The coverage itself is guarded by a catalog-derived regression in the same test file: it enumerates every column-level reference inside the ledger surface from `pg_catalog` and fails when an edge resolves to no declared composite-FK, owner-derived-trigger, or explicit exclusion — so a reference column added later cannot silently slip past the invariant.

## The precondition scan

§0 counts existing cross-KB rows on all 39 edges and aborts the whole migration with the offending edge names and row counts if any exist. Installing an invariant over a ledger that already violates it would be signing off on bad data; whoever will not repair the ledger must not get a green migration. **This is also the operational requirement**: the scan (or its query, which is safe to run read-only) should be executed against real deployments *before* rollout, so an upgrade does not stop halfway on a base nobody had checked. The scan is the audit; the migration failing closed is the enforcement.

## Measured cost

Same host (macOS, Darwin 25.4.0), same Postgres 16 (`pgvector/pgvector:pg16`), same toolchain (rustc 1.98.1), two fresh databases migrated by the respective build, alternating runs.

`bench_100k` populate phase (`UTOPIA_BENCH_DOCS=1000`, `UTOPIA_BENCH_HUB_FACTS=1000`, ~1000 documents / ~3000 chunks / ~3000 facts written through the real store functions), three runs each, alternating patched/base:

| run | base | patched |
|---|---|---|
| 1 | 9.18s | 9.35s |
| 2 | 10.29s | 8.71s |
| 3 | 9.77s | 11.11s |
| **median** | **9.77s** | **9.35s** |

The medians differ by −4% with fully overlapping ranges — **no measurable write-path overhead** on the populate path, which exercises the composite-FK edges (chunks→documents, facts→entities). A focused measurement covers the trigger-covered edges the populate phase never touches: 4000-row bulk `INSERT`s on a scratch fixture, warm, median of three — `fact_evidence` 37.1ms → 63.7ms, `typed_fact_sources` 34.3ms → 58.1ms, i.e. roughly **+6–7µs per row per triggered edge** (the two `SELECT` lookups the trigger performs). On a path that writes thousands of provenance rows a minute this is noise; it is recorded because a permanent write tax should carry a number, not an adjective.

## Alternatives considered

- **Uniform triggers everywhere** (the first cut of the migration). One mechanism for all 39 edges is simpler to describe and needs no new unique indexes, but it pays PL/pgSQL calls where a kernel-level RI check does the same work, and — the deciding point — user triggers go silent under `session_replication_role = replica` while declarative constraints do not. The replica-mode hole is real enough that uniformity was not worth it.
- **Fully declarative: add `kb_id` to the link tables.** `fact_evidence`, `fact_qualifiers`, `fact_derivations`, `entity_type_parents`, `relation_type_*`, `attribute_rule_conditions`, `typed_fact_sources`, `statement_qualifiers` could grow a `kb_id` column and take composite keys too. But that column is a denormalization of the parent's `kb_id`, and keeping it equal is a *second* invariant — which would itself need a trigger to enforce. Trading a check for a column plus the same check is the worst of both.
- **Application-layer checks.** The exporter already filters cross-KB references defensively; that is a backstop for readers, not an invariant for writers. The schema is the only layer every write path — present and future — passes through.
- **`MATCH FULL` composite keys** were considered for nullable edges and rejected: `MATCH SIMPLE` (skip the check when the reference is NULL) preserves the existing nullable-edge semantics exactly; nothing here makes a NULL reference meaningful.

## Open questions

- Whether maintainers prefer the hybrid split recorded here or uniform triggers (issue #842). The trigger machinery is additive — moving an edge from §1b to §1c is a one-line change in either direction, so the decision is cheap to revisit.
- Whether the eight supporting `UNIQUE (kb_id, id)` indexes should be partial indexes over live rows instead; measured cost does not currently justify the extra subtlety.
