<!--
Base branch: dev, not main.
Title: one English sentence stating what changes; it becomes the commit on dev.
CONTRIBUTING.md has the details behind every line below.
-->

## Why

## What changes

## How it was checked

<!-- Tests added or run; for behaviour a test cannot show, what you ran and what you saw. -->

## Before review

<!-- Delete the lines that do not apply. -->

- [ ] Every commit is signed off (`git commit -s`)
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` pass
- [ ] For changes under `web/`: `pnpm build` and `pnpm test` pass
- [ ] SQL under `crates/utopia-store/` was tested with `UTOPIA_DATABASE_URL` set (those tests skip without it)
- [ ] A new migration takes the next free number on `dev`, and `CURRENT_SCHEMA_VERSION` in `crates/utopia-cli/src/main.rs` equals the number of files in `migrations/`
- [ ] UI strings are in both `web/src/i18n/en.ts` and `zh.ts`
- [ ] A change to the data model, the ontology contract or a public API has its ADR in `docs/decisions/`
