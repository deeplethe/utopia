# Contributing to Utopia

[中文](CONTRIBUTING.zh-CN.md)

Welcome. This file covers what is specific to this repository; general open-source etiquette is assumed.

## Branches and how changes land

| Branch | What it is |
|---|---|
| `main` | Stable, matches the released version. Only maintainers merge into it, from `dev` |
| `dev` | Integration branch. Every contribution lands here first |

Your path as a contributor:

```bash
git switch dev && git pull
git switch -c fix/some-thing        # branch off dev, not main
# make changes, commit with -s (see DCO below)
git push -u origin fix/some-thing
```

Then open a PR with **base `dev`, not `main`**. A maintainer merges once CI and review pass.

Merging `dev → main` is done by maintainers on their own schedule; contributors don't need to think about it. Two rules keep the two branches from drifting apart, and both are for maintainers:

- **Back-merge `main` into `dev` right after a release.** The `dev → main` merge commit lives only on `main`, so without this `main` reads as ahead even though the trees are identical, and the gap grows by one every release.
- **Urgent fixes go through `dev` too.** A PR opened straight against `main` is the one thing that makes the two branches genuinely diverge, and then someone has to reconcile them by hand.

Both branches are protected: pull request required, CI (`backend` and `web`) must pass, no force pushes, no deletions, and admins are held to the same rules.

## Issue first, or straight to a PR

| Change | What to do |
|---|---|
| Bug fixes, docs, i18n strings, tests | Open a PR directly |
| New features, dependency changes | Open an [issue](https://github.com/deeplethe/utopia/issues) first and describe the use case |
| Data model, ontology contract, public API | Discuss in an issue, then land an [ADR](docs/decisions/) before writing code |

`docs/decisions/` is where this project's reasoning lives. An ADR records **why this and not that**, including the approaches that were tried and failed. For a change of any size, that document outlives the code.

## Local setup

Requires Docker, Rust 1.85+, Node 20+, pnpm.

```bash
docker compose up -d db                 # Postgres with pgvector
cargo run -p utopia-server              # runs migrations, :1516
cd web && pnpm install && pnpm dev      # :5173, proxies /api to the backend
```

## Before you push

CI runs exactly this. Green locally means green in CI:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd web && pnpm install --frozen-lockfile && pnpm build   # build type-checks
```

### Database-backed tests **skip** without the env var

This is the easiest thing to get wrong here. A green `cargo test --workspace` does not mean everything ran. A number of tests begin like this:

```rust
let Some(url) = utopia_store::test_db::url() else {
    return Ok(());
};
```

`test_db::url()` reads `UTOPIA_DATABASE_URL`. With `UTOPIA_TEST_REQUIRE_DB=1` also set, a missing database is a failure rather than a skip — that is how the `migrations` job in CI runs the whole `utopia-store` suite, so a green run there means the SQL was exercised. Use the same guard in new tests; do not read the env var directly.

They guard what the compiler cannot see: table aliases inside SQL strings, how `NULL` behaves in a comparison, rows an `INNER JOIN` silently drops, whether a recursive CTE expands the same ancestor twice under diamond inheritance. `cargo check` and clippy say nothing about any of it.

If you touched SQL under `crates/utopia-store/`, set it and run again:

```bash
export UTOPIA_DATABASE_URL=postgres://utopia:utopia@localhost:5432/utopia
cargo test --workspace
```

## Things review will send back

**Don't collide migration numbers.** `migrations/` rolls forward by number. Check the latest number on `main` before opening a PR — two branches each writing an `0011_` has happened, and after the merge neither one runs.

**UI strings go in i18n.** Add to both `web/src/i18n/en.ts` and `zh.ts`; no hard-coded strings in components.

**Comments explain why.** This repository comments densely and deliberately records the traps it fell into ("the first version used OR, and the Elon Musk article then produced a snapshot every 6KB"). Follow that. A comment restating what the code does will be asked to go.

**Commit messages: one English sentence, stating the motivation.** No long body. Skim `git log` for the register.

**Every workflow declares its own `permissions:`.** The repository default is now read-and-write, because one workflow commits a generated chart. A workflow without a `permissions:` block inherits that default, so leaving it out silently hands write access to something that only needed to read. Declare the minimum the job actually needs — `contents: read` for anything that just builds or tests.

## DCO: sign off every commit

We use the [DCO](https://developercertificate.org/), not a CLA. You keep the copyright on your code; you are certifying that you have the right to submit it under Apache-2.0.

Commit with `-s` and git adds the line for you:

```bash
git commit -s -m "Fix the thing"
```

which appends:

```
Signed-off-by: Your Name <your@email>
```

Forgot? `git commit --amend -s` for the last commit, or `git rebase --signoff HEAD~3` for several (adjust the count), then `git push -f`.

Use a real name and a reachable email address.

## License

By contributing you agree that your work is released under [Apache-2.0](LICENSE).
