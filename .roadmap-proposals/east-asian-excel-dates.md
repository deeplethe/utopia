# East Asian Excel dates

Roadmap item (issue): *"East Asian Excel dates: which dependency fix should Utopia use?"*

This proposal investigates the bug surface and lays out three concrete
fix paths so the maintainer can pick.

## What's actually broken

`crates/utopia-ingest/src/parsers.rs::spreadsheet` previously rendered
each cell with `other.to_string()`. For `Data::DateTime(e)` that hits
calamine's `Display for ExcelDateTime`, which writes the raw f64 serial:

```
Data::DateTime(ExcelDateTime::new(45292.0, ..., false)).to_string()
// → "45292"
```

So a Japanese 顧客マスタ with a `登録日` column came out as one number
per row, indistinguishable from `Data::Float(45292.0)` and from
`45,292` revenue totals in adjacent columns. The model saw
`39217\tclient_a\t45000\t2024-01-15\t..." three rows downstream and
couldn't tell which column was a date at all.

This isn't East-Asia-specific — it's an `utoia` bug on every locale
that's been quietly tolerable for English (where the cell content is
often re-stamped with an Excel date format on display) and broken for
every locale whose spreadsheet is read programmatically (the whole
point of Utopia).

## What we control vs. what calamine controls

Excel date serial arithmetic — including 1900 vs. 1904 base date
systems — is **already correct** in calamine 0.36:

- `ExcelDateTime::new(value, ty, is_1904: bool)` keeps the 1904 flag
- `ExcelDateTime::as_datetime()` returns a `chrono::NaiveDateTime`
  with the right offset applied (`+ EXCEL_1900_1904_DIFF = 1462.`)
- 0.36 is the current `max_stable_version` on crates.io — there is no
  newer upstream release to chase

So the "which dependency fix" framing from the issue is the wrong
question. Calamine isn't the bug; the cell rendering in Utopia is.
We don't change libraries; we fix the rendering.

## Fix paths (any one ships the behavior)

### Path A — render through `Data::as_datetime()` (recommended, ~30 lines)

- `Cargo.toml`: enable `calamine` `chrono` feature
  (`calamine = { version = "0.36", features = ["chrono"] }`)
- `Cargo.toml`: `utopia-ingest` workspace dep on `chrono`
- `crates/utopia-ingest/src/parsers.rs`: extract a `cell_to_string`
  helper. Map `Data::DateTime` / `DateTimeIso` / `DurationIso` to
  ISO 8601 (`%Y-%m-%d %H:%M:%S`); everything else stays on
  `Data::to_string()`
- Two unit tests on `cell_to_string` cover: a serial
  (`44562.0` → `2022-01-01 00:00:00`), an ISO string
  (`"2024-03-15T08:30:00"` → `2024-03-15 00:00:00`), and a no-touch
  round-trip on `Int` / `Float` / `String` / `Bool` / `Empty`.

### Path B — format-aware layer (path A +)

After path A lands, a `format_selector` step wraps `cell_to_string`:
read the XLSX `styles.xml`, find `cellXfs/numFmtId`, hand off to a
small mapping keyed on the built-in Excel number-format IDs. That
gives `"yyyy\"年\"m\"月\"d\"日\""` (Japanese date format) and
`"yyyy/m/d"` (Taiwan date format) without Utopia itself owning the
locale table.

Skipped unless the maintainer asks for it — most users will be happy
with ISO 8601, and a future "render dates per-locale" PR is a
straightforward extension on top of path A.

### Path C — replace calamine (not recommended)

There are alternatives (`excel-struct-rows`, `umya-spreadsheet`,
`simple_excel`) but each has its own date story and pulls a heavier
dependency tree. Path A reuses calamine's own
`as_datetime()` so the contract stays single-source. Not worth
chasing.

## What this proposal landed

Path A. Diff in `Cargo.toml`, `crates/utopia-ingest/Cargo.toml`,
`crates/utopia-ingest/src/parsers.rs`. Two unit tests.

## Open questions for the maintainer

- Should `DateTime` with only a date (no clock-time) render as
  `2024-01-15` or `2024-01-15 00:00:00`? Current proposal uses the
  latter; the former reads cleaner in chat answers. Easy flip if
  needed.
- Path B is real work that will touch `xl/styles.xml` parsing —
  `quick-xml` is already a workspace dep, so it's not a new
  dependency, just extra code. Flag if you want a follow-up.
