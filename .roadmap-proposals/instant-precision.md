# Time to the moment: an `instant` precision beside year / month / day

Roadmap item (README §Roadmap): *"Time to the moment: an `instant` precision
beside year / month / day, for sources that carry a real timestamp. Today a
connector rounds it to a UTC day, which can shift an event across midnight by
one day."*

This proposal adds a `SourcePrecision` knob to a source's config and routes it
through every ingestion connector that currently prints `created_at` /
`updated_at` as a UTC date. The downstream storage already accepts the
sub-day precisions (`hour` / `minute` / `second`, see `migrations/0003_graph.sql`
and `migrations/0033_the_world_axis_reaches_the_second.sql`); what is missing
is the wire-side plumbing that turns "the API gave us 23:59:59" into "the fact
record carries 23:59:59" instead of "the fact record carries 00:00:00 of the
next day". This proposal names that gap, fixes it for the demonstrative
connector (`github_issues`), and stubs the remaining ones with TODO markers so
each can be picked up as a one-connector PR.

## What is currently rounded and where

A connector is "rounding" when it takes a `DateTime<Utc>` from the upstream
API and prints it as `%Y-%m-%d` into the document text. The extractor then
parses that text back to a date, and the model emits `valid_from` at day
precision even though the source had seconds. The downstream DB then stores
the row at day precision and the moment is lost forever.

Every site that does this today is in `crates/utopia-server/src/`, not under
`crates/utopia-server/src/query_engine/`. The `query_engine` modules
(`postgres.rs`, `mysql.rs`, `trino.rs`, `databricks.rs`, `snowflake.rs`) read
typed `DateTime<Utc>` from `sqlx` and JSON-encode it as RFC 3339
(`mysql.rs:154-156` is the only date-specific cell branch and it
`.to_rfc3339()`s the instant whole). The round-to-day behaviour is in the
ingestion connectors that build the *document text* the extractor reads:

- `crates/utopia-server/src/github_issues.rs`
  - line 118 — `Opened by X on {}.` (issue creation)
  - line 123 — `Closed on {}.` (issue resolution)
  - line 158 — each event in `## History` (`e.created_at`)
  - line 177 — each comment header (`c.created_at`)
- `crates/utopia-server/src/jira_issues.rs`
  - line 177 — `Reported by X on {}.` (issue creation)
  - line 196 — `Resolved on {}.` (issue resolution)
  - line 228 — each field change in `## History`
  - line 257 — each comment header
  - line 272 — the JQL `updated >=` argument (this one is a request
    parameter, not a render site, and a sub-day window there changes the
    incremental cursor shape — see §Migration)
- `crates/utopia-server/src/extraction.rs:1055` — `doc_time` (the document's
  own date) is also `format!("%Y-%m-%d")` into the prompt. This is a
  different knob from the source-precision proposal (it is the *document*
  axis, not the *fact* axis), but the same fix shape applies — a config
  field on `Source` named `precision` can extend to it.

Notion (`crates/utopia-server/src/notion.rs:34,65,209`), WebDAV
(`crates/utopia-server/src/webdav.rs:15,37,108`), and the RSS / object-storage
sync paths already carry full `DateTime<Utc>` through. They are not in scope
for this cut, because they do not have the bug.

The closed issue #351 fixed exactly the same shape of bug for one specific
site — the `change_line` formatter at `crates/utopia-server/src/api/tools.rs`
that printed `recorded_at` as a date and made the model invent "the day
before" when it wanted the moment. The fix there was to print RFC 3339. This
proposal generalises that fix to the ingestion side and makes it a configured
choice rather than a hard-coded change.

## What is already on the receiving end

The DB layer does not need to change. `facts.valid_from_precision` already
accepts `hour` / `minute` / `second`:

- `migrations/0003_graph.sql:62` — the column is `TEXT`
- `migrations/0033_the_world_axis_reaches_the_second.sql:31,52,99` —
  index predicates and the world-axis interval code already include those
  values
- `crates/utopia-server/src/time_text.rs:27-30` — the prompt side
  (`world(at, Some("second"))`) already writes a string that
  `utopia_extract::parse_time` reads back as `(t, "second")`
  (`crates/utopia-extract/src/lib.rs:1047-1082`)

The "instant" precision that the roadmap item names is **already in the
schema**; what is missing is the ingestion path that puts it there.

## The proposed design

### 1. A `SourcePrecision` enum on the source config

Add to `crates/utopia-core/src/models.rs` (next to `Source`):

```rust
/// How a connector should print timestamps from the upstream API into the
/// document text the extractor reads. The default stays `Day` so existing
/// sources do not silently change shape; new and explicit `Instant` sources
/// keep the second.
///
/// `Year` / `Month` are accepted for symmetry with the world-axis precisions
/// but no current connector needs them — a `created_at` is always at least
/// to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePrecision {
    Year,
    Month,
    Day,
    Instant,
}

impl SourcePrecision {
    pub fn from_config(config: &serde_json::Value) -> Self {
        match config.get("precision").and_then(Value::as_str) {
            Some("year") => Self::Year,
            Some("month") => Self::Month,
            Some("instant") => Self::Instant,
            // `day` and anything else (including absent) keeps the current
            // behaviour. The same rule that applies to `extract` applies
            // here: a typo must not silently change what a source emits
            _ => Self::Day,
        }
    }
}
```

### 2. A shared formatter that takes a `DateTime<Utc>` and a `SourcePrecision`

Add to `crates/utopia-server/src/time_text.rs`:

```rust
/// Render a source-side timestamp at the configured precision. Connectors
/// call this instead of `.format("%Y-%m-%d")` so the choice is uniform and
/// one place owns the formatting rules.
///
/// `Instant` is RFC 3339 with the `Z` — the same form `change_line` adopted
/// after #351, so a tool that already reads an RFC 3339 instant can read
/// the source text too.
pub fn source(t: DateTime<Utc>, precision: SourcePrecision) -> String {
    match precision {
        SourcePrecision::Year => t.format("%Y").to_string(),
        SourcePrecision::Month => t.format("%Y-%m").to_string(),
        SourcePrecision::Day => t.format("%Y-%m-%d").to_string(),
        SourcePrecision::Instant => {
            t.to_rfc3339_opts(SecondsFormat::AutoSi, true)
        }
    }
}
```

### 3. Plumb it through one connector end-to-end

The demonstrative connector is `github_issues.rs`. Its `render` function
already takes the data and is pure — it is the cleanest place to thread the
precision through. Concretely:

- `pub fn render(issue, comments, events)` → `pub fn render_with(issue,
  comments, events, precision: SourcePrecision) -> String`
- `render` becomes `render_with(..., SourcePrecision::Day)` for callers that
  do not care (the existing tests continue to use it).
- The four `format!("…{}.format("%Y-%m-%d")…")` sites at lines 118, 123,
  158, 177 all switch to `source(t, precision)`.

The unit test pattern follows the existing `Fx` schema-key tests in
`crates/utopia-server/src/query_engine/postgres.rs:175-227` — per-test
fixture, `Uuid::now_v7()` suffix. Here the connector is pure (no DB), so
the fixture is a single timestamp: `2026-09-05T23:59:59Z`.

### 4. Stubs on the other connectors with TODO markers

For every other connector that has the rounding bug, leave a `TODO(#item)`
comment at the round site pointing at the same `source` helper, with one-line
context about how the precision arrives for that connector:

- `jira_issues.rs` lines 177, 196, 228, 257, 272 (the JQL line gets a
  comment that the cursor shape changes when `Instant` is selected — see
  §Migration)
- `extraction.rs:1055` (`doc_time` — note that this is the document axis,
  not the fact axis, and the same proposal extends there with a separate
  config key or a `Source::doc_time_precision` field; that is a second
  decision, see §Open questions)

### 5. No DB migration

The schema already accepts `hour` / `minute` / `second`. No migration is
needed for this cut.

## Migration concern: existing data is already rounded

This proposal is **forward-looking only**. Anything that has already been
ingested carries a `valid_from_precision = "day"` row and the second is gone.
Three options for the maintainer to pick from (also §Open questions):

1. **Do nothing for old data, new precision only for new writes.** The
   default of `SourcePrecision::Day` keeps the behaviour identical for any
   source that has not opted in. Existing `valid_from` rows are untouched.
2. **A `since`-flag re-extraction path.** When a source's config changes
   from `Day` to `Instant`, the next sync re-queues every document with
   `supersedes` chains the way #311 made wrong dates correctable. The
   document text is re-rendered at the new precision, the extractor
   re-reads, the new facts chain to the old ones (per 0037, the qualifier
   facts carry over; the `valid_from` itself changes from day to second).
3. **A bulk backfill job that re-reads every issue / PR / ticket from the
   upstream API and writes a `supersedes` chain.** This is the most
   invasive option and only worth it if the maintainer wants the seconds
   retroactively.

The same applies to the `JQL` cursor: a source that previously synced at
day precision and is now `Instant` should keep the day-precision cursor in
the JQL request — moving the cursor forward in time is a separate decision
and risks losing the `updated_at >= 2026-08-30` window when the day and the
instant disagree across a midnight boundary.

## Open questions for the maintainer

1. **Should `Instant` be the new default?** The current `Day` default is the
   safe one — it does not silently change what existing sources emit — but
   the roadmap item reads as "the day rounding is the bug". A new default
   would force every existing source to opt back into day precision and
   would silently change a lot of data. Recommendation: keep `Day` as the
   default for now; flip it after a release cycle.
2. **What about sources that already lost the time?** Option 1 (no
   backfill) is the cheapest. Option 3 (bulk re-read from the upstream)
   is the most invasive and only realistic for sources whose API is
   idempotent (`github_issues` is, `jira_issues` is). Option 2
   (`since`-flag re-extraction) sits in between and is the natural pair
   with the precision change — but it needs a separate decision about
   whether the document text is rewritten in place or only on re-extract.
3. **Should `Source::extracts`-style default behaviour apply to a missing
   key, or to an unparseable one?** The pattern in `Source::extracts`
   (`models.rs:156-161`) is "anything not a bool is treated as missing".
   For precision, the same pattern would mean `"precisions": "instant"`
   (typo) keeps day precision. That is the proposal above. Worth confirming.
4. **Does the precision knob extend to `doc_time` in the upload path
   (`extraction.rs:1055`)?** The roadmap item is about connector sources,
   but the same day-rounding bug exists at the upload prompt. A second
   config key on `Source` (`doc_time_precision`) would solve it; making it
   the same key is a single-knob UX but loses information about which axis
   is being controlled. Recommendation: separate key, separate PR.
5. **The JQL `updated >=` argument at `jira_issues.rs:272`.** If the
   precision is `Instant`, should the JQL send the second too? Doing so
   would change the cursor shape — the window is now bounded by an
   instant rather than a day — and could re-fetch the same row if the
   upstream `updated_at` is rounded to a day on its side. Recommendation:
   keep the JQL argument at day precision regardless; the document text
   inside is what carries the second. Worth a one-line note in the config
   docs.

## Test plan

One unit test per connector, all following the existing pure-function
pattern (no live DB, no `wiremock`):

1. **`github_issues.rs`** — `a_late_night_event_keeps_its_time_at_instant_precision`:
   - Build an `Issue` with `created_at = 2026-09-05T23:59:59Z`, one `Event`
     at the same instant, one `Comment` at the same instant.
   - Call `render_with(..., SourcePrecision::Instant)`.
   - Assert the rendered text contains `2026-09-05T23:59:59Z` and does
     **not** contain `2026-09-06`.
   - Call `render_with(..., SourcePrecision::Day)`.
   - Assert the rendered text contains `2026-09-05` and does **not**
     contain the `T23:59:59` substring.
   - The "no silent midnight shift" check is the assertion the bug would
     have failed: `2026-09-05T23:59:59Z` UTC is `2026-09-06T07:59:59` in
     `Asia/Shanghai` and the day rounding that previously existed would
     have shifted the event into the wrong day for any reader east of UTC.

2. **`jira_issues.rs`** — same shape, with a `JiraTime` fixture and the
   `render(issue)` entry point, after the precision argument is added
   in a follow-up PR.

3. **`time_text.rs`** — `source_formats_by_precision`: a four-case unit
   test on the new helper itself, with one canonical instant per
   precision. This is the place to land if the helper is moved into
   `time_text.rs` before the per-connector cuts.

The fixture pattern follows `crates/utopia-server/src/query_engine/postgres.rs:175-227`
(`Fx::new` with `Uuid::now_v7()` suffix for parallel-safe schemas).
Connectors are pure functions, so the fixture is just an in-memory value;
the suffix pattern is not needed at the helper level but will be needed
for any test that exercises the full sync path with a real source row.

## Non-goals

- No change to the world-axis code in `crates/utopia-store/src/world_axis.rs`
  — it already handles `hour` / `minute` / `second`.
- No change to the prompt side (`time_text.rs::world`) — it already
  serialises every precision correctly.
- No change to the upload path (`extraction.rs:1055`) in this cut; a
  follow-up PR with `doc_time_precision` is the right shape (see
  §Open questions 4).
- No backfill or supersedes-chain logic for already-rounded data
  (see §Migration concern).
