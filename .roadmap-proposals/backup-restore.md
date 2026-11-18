# Backup and restore commands

Roadmap item (README §Roadmap): *"Enterprise: OIDC SSO, backup and
restore commands, benchmarks at 100k documents."*

This proposal adds a `utopia` operator CLI with two subcommands,
`backup` and `restore`, that shell out to `pg_dump` / `pg_restore` and
`tar` to produce and replay a self-describing archive of the database
plus the on-disk `data/` directory.

## Why a separate crate

The server binary at `crates/utopia-server/src/main.rs` is the
production runtime. It already loads `AppConfig`, opens the database
pool, runs migrations, and starts the HTTP server. Adding
`utopia backup` to it is *possible* — a subcommand dispatch in `main`
would do — but two reasons argue for a separate `utopia-cli` crate:

1. **The CLI doesn't need the HTTP server, the extractor, the chat
   loop, the mapping engine, or any of the 50+ modules pulled in by
   `utopia-server`.** Today `utopia-server` is the largest crate in
   the workspace; making the operator compile it just to dump the
   database is a real cost (CI time, image size, attack surface).

2. **The CLI is its own surface.** It runs in containers, in
   Kubernetes Jobs, in cron, on operators' laptops. Keeping it
   separate makes the boundary explicit and lets us ship a thinner
   image.

The CLI reuses `utopia-core::config::AppConfig::load()` for connection
info (so it picks up the same `.env`, the same `UTOPIA_DATABASE_URL`,
the same `UTOPIA_DATA_DIR` that the server uses), but nothing else.

## `utopia backup`

```text
$ utopia backup [flags]

  --output <PATH>            # archive path; default utopia-<timestamp>.tar.gz
  --include-data-dir         # also tar the data/ directory (without secret.key)
  --include-secret-key       # …and the sealing key as well, deliberately
  --dry-run                  # print the plan, don't write anything
  --pg-dump <PATH>           # path to pg_dump binary
  --tar <PATH>               # path to tar binary
  --migration-url <URL>      # override connection string for pg_dump

# defaults to .env / UTOPIA_DATABASE_URL for the connection
```

What it does:

1. Resolves the connection string and `data_dir` from `AppConfig`.
2. Runs `pg_dump -Fc` to a temporary file under the current directory.
   `-Fc` (custom format) is the only format `pg_restore` consumes;
   plain SQL would lose the ability to do parallel restore and would
   fail on large objects.
3. Optionally tars `data/` into a second temporary file.
4. Builds a `manifest.json` with:
   - `schema_version`: pulled from the running migrations
   - `utopia_version`: from `Cargo.toml`
   - `created_at`: UTC ISO 8601
   - `components.pg_dump.{path, format, bytes}` and `data_dir.{path, present}`
   - `checksums`: `sha256:<hex>` for each component
5. Writes a single `*.tar.gz` containing `manifest.json` + `pg_dump.custom`
   + (optional) `data/`. Refuses to overwrite an existing archive.

Why a tarball wrapper instead of just shipping the `pg_dump` output?

- One file to copy off-host.
- `manifest.json` records what was backed up, when, and from which
  schema version — without it, six months later, you can't tell
  whether a `.dump` file came from a v0.1 or v0.2 server.
- Checksums in the manifest let `restore` fail loudly on a corrupted
  archive before touching the live database.

## `utopia restore`

```text
$ utopia restore --from <PATH> [flags]

  --from <PATH>              # archive to restore from (required)
  --target-data-dir <PATH>   # override the data_dir path
  --pg-restore <PATH>        # path to pg_restore binary
  --dry-run                  # validate and print the plan; don't change targets
  --force                    # allow overwriting a non-empty database or data dir
  --yes                      # skip the "are you sure?" prompt
```

Before changing either target, restore checks required archive components,
checksums, version, target path types and overwrite permission. Declared data
must exist in the archive. `--force` is required for a database containing user
objects (even empty tables, schemas, functions or types), and for a non-empty
data directory when the archive includes data. `--yes` only skips confirmation.
`--dry-run` uses the same preflight, including a read-only database check; it
extracts temporary staging files in the current working directory but never
restores the database or writes the target data directory. After confirmation
and data preparation, restore repeats component, target-path and overwrite
checks immediately before running `pg_restore`.

Database-only archives leave the data directory untouched. Otherwise, data is
prepared separately and replaces the old directory after `pg_restore` succeeds;
files and keys are never merged with the old directory. A database restore
failure preserves the old files. With `--force`, `pg_restore --clean --if-exists`
replaces objects in the archive without removing unrelated database objects.
Every restore uses `--single-transaction --exit-on-error`: a SQL execution error
rolls back the database restore instead of leaving partial SQL changes.

Stop all writers and retain a backup of both targets before restoring. Preflight
does not prevent concurrent changes or later permission and disk failures.
Database and filesystem restoration are **not atomic**: the database may be
committed before a later directory installation fails. Failed installation
attempts to put the old directory back; a failed rollback reports its retained
location. After an interruption, also check the target's parent for
`.utopia-restore-data-*/previous`. Keep the service stopped, resolve the error,
and retry the archive with `--force`, or restore both targets from the previous
backup. Moving old files back alone does not undo database changes.

Both halves are implemented. `backup` landed first, and `restore`
followed in the same crate, for two reasons worth keeping on the record:

1. The backup side is independently useful — operators want to *take*
   backups even before they have a working restore, because the
   alternative (no backups at all) is worse.
2. The restore side has design questions that benefit from running
   backup in production for a while first (see Open question #2).

## Manifest version policy

`schema_version` is a `u32`. On read, `restore` will refuse a manifest
whose `schema_version` is greater than the running server's
`schema_version` — forward-incompatible backups are useless. Older
manifests are accepted with a warning, since older schema versions
might still restore cleanly into a newer server.

## Docker compatibility

The server ships as `ghcr.io/deeplethe/utopia`. Today the runtime
image does *not* include `postgresql-client` (pg_dump lives in a
separate `postgres:16` image). Two ways to close that gap:

- **A: Bundle `pg_dump` in the server image.** Pro: one image, `utopia
  backup` works in any container that the server runs in. Con:
  ~30 MB extra for the postgres client.
- **B: Ship a separate `utopia-cli` image with `pg_dump` and `pg_restore`.**
  Pro: server image stays small. Con: two artifacts to publish.

Recommendation: **B**. The cli image is also where future ops tools
will live (`utopia migrate`, `utopia reindex`); it's the right
home for `postgresql-client`.

This proposal does **not** ship the docker image change — it lands the
binary first, the image second.

## Open questions for the maintainer

1. **Where does the CLI live?** `crates/utopia-cli/` (separate crate,
   separate `utopia` binary — this proposal) vs. a `bin/utopia.rs`
   inside `utopia-server` (one less artifact, slower compile for
   everyone, one less Cargo.toml to maintain). My read: separate
   crate, separate binary.
2. **Restore strategy.** Drop-and-recreate the database (requires
   `CREATEDB` privilege on the operator's role) vs. in-place
   `pg_restore --clean` (no extra privilege, but slower and brittle
   on partial restores). My read: in-place `--clean`, since the
   operator role that can dump is usually the same one that owns the
   schema.
3. **Should the app Docker image install `postgresql-client`?** My
   read: no — keep the runtime image minimal; ship a separate
   `utopia-cli` image with `pg_dump` and `pg_restore`.
4. **Manifest version policy.** Refuse forward-incompatible
   manifests on read? Accept older ones with a warning? My read:
   refuse forward, warn on older.
5. **Should `UTOPIA_BACKUP_DIR` become a new config knob** for the
   default output location (today's default is the current working
   directory)? My read: yes, but trivially — it's the kind of thing
   operators will set once and forget.

## What this cut does NOT do

- No docker image change (see Open question #3).
- No `UTOPIA_BACKUP_DIR` config (see Open question #5).
- No automatic migration of older manifests — older versions are accepted
  with a warning; newer versions are refused.
- **The sealing key is left out of the archive by default.** The dump carries
  credentials sealed with `data/secret.key`, and an archive is the artifact that
  gets copied between hosts and handed to whoever runs the restore, so shipping
  both halves in one file is not an unencrypted archive — it is no sealing at
  all. `--include-secret-key` opts in to copying an existing key file. The
  manifest records that flag even when an environment-key deployment has no
  key file; restore remains compatible with those archives. When replacing
  data, an absent key is never taken from the old directory: restore warns to
  set `UTOPIA_SECRET_KEY` to the original key, or re-enter sealed credentials
  after starting the server.
- No encryption-at-rest. Out of scope; the operator's filesystem
  encryption (LUKS, EBS encryption, etc.) is the right layer.
- No streaming upload to S3. Out of scope; the operator can pipe
  `utopia backup --output -` to `aws s3 cp - s3://…` today.

## Test plan

Unit tests cover parsing, manifests, versions, checksums and key-copy policy.
The CLI integration suite runs in `cargo test --workspace`: a recording
`pg_restore` double checks preflight ordering, post-confirmation validation,
overwrite guards, replacement, rollback, dry runs and database-only archives.
Real `pg_dump`/`pg_restore` tests exercise database restoration and SQL rollback
using disposable test databases.
CI provides PostgreSQL 16 clients; the double also needs `rustc`, and
archive operations need `tar`.

Tests use `utopia_store::test_db::url()` and create uniquely named databases on
that test cluster; they never modify the guard database's contents. Use only a
dedicated test cluster with `CREATEDB` and catalog-fixture privileges, never a
business database. Without a URL, tests skip unless `UTOPIA_TEST_REQUIRE_DB=1`
requires them to run, as in CI.

```bash
UTOPIA_DATABASE_URL=postgres://test_role:test_password@localhost:1543/test_admin \
UTOPIA_TEST_REQUIRE_DB=1 cargo test -p utopia-cli
```

## Migration concern: existing data is already there

Backups are forward-looking. Anything written before this lands has
no `manifest.json` and no `checksums`. The operator's first backup
after upgrade will be the first one with a manifest; older `.dump`
files (if any exist) are still valid `pg_dump -Fc` files but cannot
be verified or version-checked by `utopia restore`.
