# ClickHouse driver

Roadmap item (README §Roadmap): *"More sources: a ClickHouse driver."*

This proposal lands a ClickHouse engine alongside the existing five
postgres / mysql / trino / databricks / snowflake. Same shape: one
connection string, four layers of safety, results as JSON Lines. Three
cuts land it without breaking the public interface mid-stream.

## Why HTTP, not the wire protocol

ClickHouse ships a binary TCP protocol (`native`, `tcp`), but the
HTTP interface is the de facto one for tools, proxies and our own
mounting shape: one SQL → one request → one NDJSON body. The wire
protocol and the HTTP interface expose the same SQL surface, and our
four safety layers (sqlparser → LIMIT cap → session-readonly →
statement timeout) only need:

- `guard_sql_for` against `PostgreSqlDialect` (ClickHouse SQL is a
  superset of the common cases; sqlparser has no ClickHouse dialect).
- An outer `LIMIT cap+1` and a `truncate_rows` step.
- A per-statement timeout. ClickHouse has no session, so "readonly"
  lives only in `guard_sql_for`. Same trade-off Trino is already on.
- The per-query `?max_execution_time=10` so an accidental `SELECT *
  FROM huge_table` doesn't keep a worker busy forever.

Going HTTP keeps the driver under 200 lines and matches the Trino /
Snowflake shape — `reqwest` + URL scheme dispatch in `mod.rs` + a
`Conn` struct in `conn.rs`.

## What's already on disk (cut 1)

- `migrations/0056_clickhouse_engine.sql` registers `clickhouse` in
  the `data_sources.engine` CHECK constraint.
- `crates/utopia-server/src/query_engine/clickhouse.rs` was first
  cut as a placeholder shell that compiles and refuses with
  `"not yet implemented (cut 2)"`. The shell confirmed the scheme
  dispatch landed in the factory; the body lived behind `bail!`.
- `query_engine::engine_from_conn("clickhouse://...")` and
  `engine_for("clickhouse", ...)` were stubbed so URLs of the form
  `clickhouse://user@host:8123/db` parsed without panic.

## Cut 2 (this PR): real HTTP driver

- Connection string parser: `ClickhouseConn::parse` in `conn.rs`
  mirrors Trino's rules (basic-auth upgrade, port 8443 → https,
  `?ssl=false` to keep tests on plain http). Library is the path's
  first segment — ClickHouse's "database" is the analogue of PG's
  schema, no second namespace.
- `test()`: `SELECT 1` over `POST /`, parse the one-row
  `{"1":1}\n` body.
- `fetch_schema()`: `system.columns`, scoped to the bound database.
  `is_in_primary_key` is read here (MergeTree-only, but populated
  for the cases the system cares about; for non-MergeTree the column
  is 0, same as "we don't know" for the other engines).
- `execute()`: `FORMAT JSONEachRow`, single round-trip, no
  pagination. `wrap_limit_for_ch` appends `LIMIT cap+1` directly
  rather than wrapping in `(SELECT ...) AS _q` — ClickHouse parses
  that subquery into a derived table, and the inner SELECT wouldn't
  push the LIMIT down the same way. Direct suffix is what every
  client we've read does.
- Column-order preservation: `serde_json::Value::Object` is now a
  linked-hash map (`preserve_order` feature in the workspace).
  Without it, `JSONEachRow`'s "keys in SELECT order" guarantee would
  get alphabetised at parse time and the column order the model
  reads would silently change.
- Errors: `4xx/5xx` with body text — ClickHouse writes
  `Code: 60. DB::Exception: ...` to the body on every query error,
  no separate JSON. We surface the whole body, no parsing.
- Wiremock-based tests cover Basic auth, multi-row truncation at
  201, `system.columns` mapping, and error surface. The `factory_builds_a_clickhouse_engine`
  test asserts the cut-1 `bail!` is gone.

## Cut 3 (this PR / next): is_primary_key 跟随表引擎家族

`#502` 列了各家引擎的 PK/FK 覆盖。cut 2 已经从 `system.columns.is_in_primary_key` 读出来 ——
但该字段对非 MergeTree 系（`Log` / `Kafka` / `Memory` / `Distributed` / `Materialized*` …）
恒为 0。0 在 schema 层既可以读成「该列不是 PK」，也可以读成「不适用」，
而后者才是 CH 的真实语义。

cut 3 在 `fetch_schema` 里把 `system.tables.engine_full` 通过 JOIN 一起读进来，
`schema_row` 用引擎家族判断「PK 是否适用」：

- 子串匹配 `MergeTree`（包含 `ReplacingMergeTree` / `SummingMergeTree`
  / `Replicated*` 各家），仅当命中才把 `is_in_primary_key` 拨给 `system.columns` 的值。
- 不命中且原 `is_in_primary_key` 是 0 时，把 comment 加上「not a MergeTree-family engine (Kafka)」
  之类字符串，给探索提示词一个明确语义。

外键：`references_table` 仍是 `None`，但这是因为 ClickHouse 没有 first-class FK —— 不是 bug。
schema 层不需要任何代码改动，注释文档写在 `schema_row` 上一并保留。

`ON CLUSTER` 是 DDL，`guard_sql_for` 已经把它挡在只读外，
cut 3 不需要触碰。

## Open questions for the maintainer

- Should `test()` insist on a `SELECT 1` that returns a non-empty body, or also accept `OkHttpResponse` with empty body (some hosted versions reply that way)? Current cut bails on empty; happy to flip.
- Real-cluster live test (`UTOPIA_TEST_CLICKHOUSE_URL`) — tracked in `trino.rs::live_tests` but not present here yet. Adding it alongside the wiremock tests would close the same gap `trino.rs` opens with; cut 3 PR is the natural place.

## Open questions for the maintainer

- Should `test()` insist on a `SELECT 1` that returns a non-empty body, or also accept `OkHttpResponse` with empty body (some hosted versions reply that way)? Current cut bails on empty; happy to flip.
- Real-cluster live test (`UTOPIA_TEST_CLICKHOUSE_URL`) — tracked in `trino.rs::live_tests` but not present here yet. Adding it alongside the wiremock tests would close the same gap `trino.rs` opens with; cut 3 PR is the natural place.
