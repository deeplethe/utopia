# 0018 · The lakehouse is one protocol away

- **Status**: implemented · `trino` / `databricks` / `snowflake` engines in `query_engine/` (migration `0021` widens the `engine` CHECK) · Trino has run against a real cluster and keeps a gated live test (#327); Databricks and Snowflake are covered by protocol replays only (wiremock) and **still want one** (#241, #242) · MaxCompute is not done, see the last section · the MySQL wire family this record skipped landed later in #303 (migration `0025`), so the order below is now "Postgres, MySQL, HTTP" as the `query_engine` header always had it
- **Written**: 2026-09-03 (conventions in the [README](README.md))
- **Related**: [0011](0011-a-mapping-is-not-a-fact.md) placed data sources at the deployment level and mounts at the base level; this record leaves that layer alone. [0016](0016-close-the-open-seams-before-cutting-new-ones.md) D4 put the MySQL wire protocol ahead of the lakehouse; the first section explains why the order flipped

> The roadmap line reads "Iceberg / Delta Lake, Databricks, Snowflake and MaxCompute": four names, three kinds of thing. The first two are table formats, the next two are services, the last is a service on another cloud. Treating them as four engines would be writing code per product name. This record pins down what an engine is first, then decides which ones to build.

## Engines follow protocols

The header of `query_engine` (written for 0011) already gave the direction: the Postgres wire protocol, then the MySQL wire family, then the HTTP family. This step lands the HTTP family and skips MySQL. 0016 D4's "TiDB / OceanBase / Doris / StarRocks for free" is a fine list, but the lakehouse is what is wanted now, and on the protocol axis it is closer than it looks:

| Wanted | What it is | Which protocol that is for us |
|---|---|---|
| Iceberg, Delta Lake, Hive, Hudi | table formats plus a catalog, with no query endpoint of their own | one **Trino** catalog each; `POST /v1/statement` with `nextUri` paging |
| Databricks | a Delta lakehouse behind a SQL warehouse | its own **SQL Statement Execution API** (`/api/2.0/sql/statements`) |
| Snowflake | a cloud warehouse that also reads Iceberg | its own **SQL API v2** (`/api/v2/statements`) |
| MaxCompute | Alibaba Cloud's warehouse | signed REST, asynchronous instances, results through Tunnel |

The first three rows are all "JSON in, JSON out, Bearer or Basic auth". `reqwest` is already a dependency; each engine is about two hundred lines. The binary still carries no native database driver, which is the promise in the README's first sentence. It is also why the answer to Iceberg is Trino rather than an Iceberg reader: reading Iceberg directly pulls in Arrow, Parquet, object-storage SDKs and a query planner, and that is a different product.

## The connection string is the only input

The data-source page has a name and a connection string. Three new engines add no dropdown: **the scheme picks the engine** (`engine_from_conn`), and each engine parses the rest (`conn.rs`). The shape follows `postgres://user:pass@host/db`: credentials in the userinfo, the path is "catalog / database / schema", engine-specific switches go in the query string:

```
trino://alice[:password]@host[:8080]/catalog[/schema][?ssl=true]
databricks://:TOKEN@workspace-host/sql/1.0/warehouses/ID[?catalog=main&schema=default]
snowflake://:TOKEN@account.snowflakecomputing.com/DB[/SCHEMA][?warehouse=WH&role=R&token_type=pat|oauth]
```

The Databricks path is the httpPath shown in the console, so it can be pasted as is. All three tokens sit in the password position; `TOKEN@` with the colon missing is the most common slip, so the username position is accepted too. The shape is validated at registration, and the error carries the expected form. `ssl=false` exists for local proxies and stand-ins; the three services themselves only speak https.

Left out on purpose: Snowflake key-pair JWT (local RSA signing, a dependency for a second login method, wait for someone to need it) and Trino Kerberos / OAuth2 (same reasoning). Passwords and tokens are the whole surface.

## The HTTP family has three of the four gates

0011 set up defense in depth: parse and admit only SELECT, wrap a LIMIT, a read-only session with a timeout, JSON Lines out. The HTTP family **has no session**, so the third gate is a timeout alone (Trino's `query_max_execution_time` session property, Databricks' `wait_timeout`, Snowflake's `timeout`), and read-only rests entirely on the first gate. The first gate therefore parses with each engine's dialect: `DatabricksDialect`, `SnowflakeDialect`, and `GenericDialect` for Trino (sqlparser has no Trino dialect; Generic is a superset). One test runs the same three checks under all four dialects: SELECT passes, DELETE fails, two statements fail.

The fourth gate is assembled here for the HTTP family. Databricks' `JSON_ARRAY` and Snowflake's `data` return every value as a string; `coerce` restores numbers and booleans from the manifest / rowType column types, otherwise a model handed `"42"` stops doing arithmetic. Key order is assembled by hand instead of through `serde_json::Map`, which sorts keys unless `preserve_order` is on, and column order is the order the query wrote.

## Loopback goes direct

reqwest's system-proxy detection on Windows reads the registry and did not honor a `127.*` bypass, so a stand-in on the loopback address went through the proxy and came back as 502. The engine client now carries an explicit policy: loopback and `NO_PROXY` hosts go direct, everything else follows `HTTPS_PROXY` / `HTTP_PROXY` / `ALL_PROXY`. A server process reads its environment; that matches how docker-compose configures it. The other connectors keep reqwest's default.

## Replays are the only tests so far

No stand-in for any of the three runs on this machine: Docker Hub cannot be reached here (see memory), and Databricks and Snowflake are cloud-only anyway. The tests replay each vendor's documented protocol with wiremock: Trino's two `nextUri` pages, Databricks' PENDING → SUCCEEDED polling, Snowflake's 202 → 200, and each error body. **They prove our reading of the protocol, and only that.** Until one real cluster has answered, the README keeps these three marked as awaiting a real run, handled the way #214 / #215 handle GCS and Notion: an issue per engine labeled help wanted.

## MaxCompute waits

It is the one name of the four that is not "JSON in, JSON out": requests are signed with an AccessKey (HMAC-SHA1 over canonicalized headers), SQL runs as an asynchronous instance, and results come either through Tunnel (another protocol) or `GetInstanceResult` as CSV capped at ten thousand rows. Together that is a connector's worth of work, and this machine has no account that could sign a request, so the result could only be "probably like this". It stays on the roadmap until someone with an account arrives, or until its MySQL-compatible entry (MCQA) can ride the MySQL wire protocol of 0016 D4.

## Open questions

- **How much schema to fetch.** All three expose `information_schema.columns`, and a lakehouse catalog can hold thousands of tables; `sync_schema_doc` caps at 200. With a schema in the connection string only that schema is read, otherwise the whole catalog. Whether that is enough waits for a real cluster.
- **The type-restoration table** in `coerce` is hand-written from the three vendors' docs. Snowflake's `fixed` with a scale returns `"42.10"`, which becomes 42.1 and loses the trailing zero; harmless for a model, possibly not for an "exact definition". Revisit when the semantic layer keeps evidence (0016 D1) and decide whether to keep the raw string alongside.
- **Trino's `ssl` inference**: a password, `ssl=true`, or port 443 / 8443 means https, anything else is plaintext. That is trino-python's rule, and someone who gets it wrong sees a TLS error instead of a hint.
