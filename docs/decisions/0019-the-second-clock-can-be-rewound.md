# 0019 · The second clock can be rewound

- **Status**: in progress · the read paths and the API rewind the recording axis (#317); the graph control stays a separate cut (#307)
- **Written**: 2026-09-04 (conventions in the [README](README.md))
- **Related**: [0003](0003-ontology-growth-loop.md) put adoption's rewrites on the same append path as human correction, so the prior state is still on disk; [0002](0002-reasoning-engine.md) built the proof chain on the same rows. #268 (deleting a document) is what made the gap urgent, and is deliberately a separate change

> The write side has kept two clocks since the graph migration: `valid_from` / `valid_to` for when a fact held in the world, `recorded_at` / `invalidated_at` for when we held it. The read side rewinds only the first. "What did we believe in March?" is answerable from rows we already store and unanswerable through any query we ship.

## What is missing

`edges_among(at)` keeps a fact whose validity window contains `at`, treating a NULL bound as open — and carries an unconditional `invalidated_at IS NULL` on top. That hard-coded filter appears 26 times in `graph.rs` and 54 times across the store.

The recording axis has two readers, and neither answers this question. `graph_changes(since, until)` covers the whole graph but returns a list of events; its comment draws the line this record acts on — one asks "what did the world look like at that moment", the other "what did we change our minds about". `entity_history` does not filter `invalidated_at` at all, but answers one entity at a time. So a fact corrected in March is invisible at every slider position, and the state before that correction can be reached only by opening the entity that carries it.

Deleting a document makes it urgent (#268): a fact whose last source is gone should be invalidated on the recording axis, and after that it is reachable only if the axis can be rewound. The gap predates deletion, though — every retracted or corrected fact is equally unreachable today, which is half of what the README means by "record the whole course of changing understanding".

## One predicate

`held_at(T)` — `recorded_at <= T AND (invalidated_at IS NULL OR invalidated_at > T)`, where `None` means now. Every **read** that hard-codes `invalidated_at IS NULL` takes it instead. `derived_facts` carries `derived_at` and `invalidated_at`, so derivations follow the same rule and a rewound graph keeps the edges the engine had drawn by then.

Write paths keep the hard-coded filter: `confirm_fact`, `reject_fact`, the adoption undo and the dedup lookups are guards on the current row, and a correction is never made as of March.

## Two parameters, never one control

`at` (world) and `as_of` (record) stay separate all the way out to the API. Folded into one slider they would answer "the world in March, as we understand it now" with "the world in March as we understood it then", or the reverse, and both look plausible on screen. `graph_changes` already warns that mixing the two sets of columns gives a quiet wrong answer; mixing the two controls is the same mistake one layer up.

## The risk is a forgotten WHERE

This defence lives in SQL, which is where 0009 was bitten: `NULL <> uuid` raises nothing and selects nothing, so a guard that reads correctly in Rust ran silently empty. `crates/utopia-store/tests/human_type_decisions.rs` exists because of it, and its header states the shape of the danger — a defence spread across read sites fails the moment one is missed, and `cargo check` never says a word.

So the predicate goes in one place per table rather than at every read site, and a database-backed test asserts both directions: a retracted fact is absent at `as_of = now`, present at an `as_of` before its `invalidated_at`, and a fact recorded after T is absent at T.

Read-only throughout. No new column, no migration.

## Open questions

- **Entities have no clock.** `merged_into` records that a merge happened, not when; the time is in `entity_merges.created_at` / `reverted_at`. Two entities merged in March should be two nodes at an `as_of` in February, which means unwinding merges through that table rather than reading the entity row. Probably a second cut.
- **Retrieval has the same two clocks.** `chunks` carries `created_at` / `superseded_at`, so the predicate above applies unchanged. What is missing is content: `replace_chunks` clears `embedding` on the superseded version to save storage, and the full-text index keeps no history. Deleted documents keep everything (#268), so as-of retrieval over them costs nothing; over earlier *versions* it costs keeping their vectors, which is a storage decision, and full text stays "now" either way.
- **What the control is.** A second slider doubles the surface for a question most people ask rarely; a mode switch on the existing slider is cheaper but risks reading as the same axis, which is the confusion the section above is written to prevent.
