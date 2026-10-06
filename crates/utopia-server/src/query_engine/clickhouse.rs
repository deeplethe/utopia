//! ClickHouse over HTTP: `POST /` with the statement in the body. With no session, read-only
//! and the timeout are URL settings on each request, and **the account decides which it takes**:
//!
//! - **Drop a refused setting and retry.** Ask for `readonly=1` and `max_execution_time`; drop
//!   the one the profile refuses (four measured account types: [`ClickHouseEngine::negotiated`]).
//! - **If the server cannot hold the time, the engine kills the query.** Each request has a
//!   `query_id`; at the deadline, or when the caller drops the future (Stop in chat), we send
//!   `KILL QUERY`. This needs `SELECT ON system.processes`; without it a query is refused, unless
//!   readonly=1 holds and the account's own `max_execution_time` is 10 s or less. Behind a load
//!   balancer with several replicas a KILL can reach another replica and match nothing; give such
//!   accounts a server-side `max_execution_time`.
//! - **Values come back by column type** from `JSONCompactStrings`; [`cell`] says why.

use super::conn::ClickHouseConn;
use super::{
    coerce, rows_to_json_lines, truncate_rows, wrap_limit, QueryEngine, QueryResult, SchemaColumn,
    STATEMENT_TIMEOUT_SECS,
};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

/// How `JSONCompactStrings` writes NULL. When the server's `output_format_pretty_grid_charset`
/// is ASCII, it writes `NULL`
const NULL_TEXT: &str = "ᴺᵁᴸᴸ";
const NULL_TEXT_ASCII: &str = "NULL";

pub struct ClickHouseEngine {
    conn: Arc<ClickHouseConn>,
    /// Kill the query if no reply comes by then. Two seconds more than the server's
    /// `max_execution_time`: when both apply, the server's error is clearer than ours
    deadline: Duration,
}

#[derive(Deserialize)]
struct Reply {
    meta: Vec<Meta>,
    data: Vec<Vec<serde_json::Value>>,
    /// An error raised after the result started. Servers 23.9 to 25.10 put it here by default
    /// (`http_write_exception_in_output_format`), with status 200 and half the data. From 25.11
    /// the error text follows the output, and the body does not parse
    exception: Option<String>,
}

#[derive(Deserialize)]
struct Meta {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
}

/// An error from the server. `code` comes from the `X-ClickHouse-Exception-Code` header
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct ServerError {
    code: Option<u32>,
    message: String,
}

/// The two guards a request carries on its URL
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Guards {
    readonly: bool,
    timeout: bool,
}

impl Guards {
    const ALL: Self = Self {
        readonly: true,
        timeout: true,
    };
    const NONE: Self = Self {
        readonly: false,
        timeout: false,
    };
}

#[derive(Debug, PartialEq, Eq)]
enum Refused {
    Timeout,
    ReadOnly,
}

/// A ClickHouse string literal. ClickHouse also reads a backslash as an escape, so the shared
/// `sql_literal`, which only doubles quotes, would misread a name like `sales\north`
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The last `n` characters: a long reply keeps its error at the end
fn tail(text: &str, n: usize) -> &str {
    text.char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map_or(text, |(i, _)| &text[i..])
}

/// The server's message from an error reply. An error after part of the result comes after
/// that data. reqwest asks for gzip, and when the server compresses (`enable_http_compression`,
/// on by default in 26.x) it holds back the headers up to a size: below it the reply is a 500
/// with all rows before `Code: N.` (26.9.12.8: 15 MB); above it see [`trailer_message`]. The
/// message can quote the same `Code: N.` in a literal, so a reply that starts with it is the
/// plain error and stays whole
fn server_message(code: Option<u32>, text: &str) -> String {
    let text = text.trim();
    if let Some(message) = trailer_message(text) {
        return message.to_string();
    }
    let Some(c) = code else {
        return tail(text, 1000).to_string();
    };
    if text.starts_with(&format!("Code: {c}.")) {
        return text.to_string();
    }
    match text.rfind(&format!("Code: {c}. DB::")) {
        Some(i) => text[i..].to_string(),
        None => tail(text, 1000).to_string(),
    }
}

/// When the headers have already gone out (no compression, or a result too large to hold
/// back), 25.11+ sends status 200: the rows, then
/// `__exception__ <tag> <message> <length> <tag> __exception__`, one per line. The block must
/// end the body and open and close with the same tag
fn trailer_message(text: &str) -> Option<&str> {
    let tag = trailer_tag(text)?;
    let body = text
        .strip_suffix("__exception__")?
        .trim_end()
        .rsplit_once('\n')?
        .0;
    let block = &body[body.rfind("__exception__")? + "__exception__".len()..];
    Some(block.trim_start().strip_prefix(tag)?.trim())
}

/// The tag on the last line of an `__exception__` block
fn trailer_tag(text: &str) -> Option<&str> {
    let rest = text.strip_suffix("__exception__")?.trim_end();
    rest.rsplit_once('\n')?.1.split_whitespace().nth(1)
}

fn query_id() -> String {
    format!("utopia-{}", uuid::Uuid::now_v7().simple())
}

fn request(
    client: &Client,
    conn: &ClickHouseConn,
    body: &str,
    guards: Guards,
    query_id: Option<&str>,
) -> anyhow::Result<reqwest::RequestBuilder> {
    let mut url = reqwest::Url::parse(&conn.base)?;
    {
        let mut q = url.query_pairs_mut();
        if let Some(id) = query_id {
            q.append_pair("query_id", id);
        }
        if let Some(db) = &conn.database {
            q.append_pair("database", db);
        }
        if guards.readonly {
            q.append_pair("readonly", "1");
        }
        if guards.timeout {
            q.append_pair("max_execution_time", &STATEMENT_TIMEOUT_SECS.to_string());
        }
    }
    let mut req = client.post(url).body(body.to_string());
    if let Some(user) = &conn.user {
        req = req.basic_auth(user, conn.password.as_deref());
    }
    Ok(req)
}

/// Kill our own query. Gives back whether the KILL matched it: the reply has one TSV row per
/// matched query (status, query_id, user, query), and none when no query matched (for example,
/// the KILL reached another replica). The error holds the server's reason (usually no
/// `system.processes` grant)
async fn kill_query(
    client: &Client,
    conn: &ClickHouseConn,
    query_id: &str,
) -> anyhow::Result<bool> {
    let sql = format!(
        "KILL QUERY WHERE query_id = {} ASYNC FORMAT TabSeparated",
        literal(query_id)
    );
    let resp = request(client, conn, &sql, Guards::NONE, None)?
        .timeout(Duration::from_secs(5))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    anyhow::ensure!(status.is_success(), "{}", text.trim());
    Ok(text
        .lines()
        .any(|row| row.split('\t').nth(1) == Some(query_id)))
}

/// Kills the query if the future is dropped (Stop in chat, MCP disconnect) or at the deadline
/// ([`KillOnDrop::fire`]). The KILL runs in its own task, so a later drop cannot stop it
struct KillOnDrop {
    target: Option<(Client, Arc<ClickHouseConn>, String)>,
}

impl KillOnDrop {
    fn disarm(&mut self) {
        self.target = None;
    }

    fn fire(mut self) -> Option<JoinHandle<anyhow::Result<bool>>> {
        self.target.take().and_then(spawn_kill)
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(target) = self.target.take() {
            spawn_kill(target);
        }
    }
}

fn spawn_kill(
    (client, conn, query_id): (Client, Arc<ClickHouseConn>, String),
) -> Option<JoinHandle<anyhow::Result<bool>>> {
    let rt = tokio::runtime::Handle::try_current().ok()?;
    Some(rt.spawn(async move {
        let killed = kill_query(&client, &conn, &query_id).await;
        match &killed {
            Err(e) => tracing::warn!(%query_id, error = %e, "could not kill the ClickHouse query"),
            Ok(false) => tracing::warn!(%query_id, "KILL QUERY matched no ClickHouse query"),
            Ok(true) => {}
        }
        killed
    }))
}

impl ClickHouseEngine {
    pub fn new(conn: ClickHouseConn) -> Self {
        Self {
            conn: Arc::new(conn),
            deadline: Duration::from_secs(u64::from(STATEMENT_TIMEOUT_SECS) + 2),
        }
    }

    async fn post(&self, client: &Client, body: &str, guards: Guards) -> anyhow::Result<String> {
        let id = query_id();
        let req = request(client, &self.conn, body, guards, Some(&id))?;
        let mut guard = KillOnDrop {
            target: Some((client.clone(), self.conn.clone(), id)),
        };
        let sent = tokio::time::timeout(self.deadline, async {
            let mut resp = req.send().await?;
            let status = resp.status();
            let header = |name| {
                resp.headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.trim().to_string())
            };
            let code = header("X-ClickHouse-Exception-Code").and_then(|v| v.parse::<u32>().ok());
            let tag = header("X-ClickHouse-Exception-Tag");
            let mut body = Vec::new();
            loop {
                match resp.chunk().await {
                    Ok(Some(chunk)) => body.extend_from_slice(&chunk),
                    Ok(None) => break,
                    // After an error block 25.11+ cuts the reply on purpose, so that it cannot pass
                    // for a whole result. A block with the header's tag is the server's answer;
                    // any other cut is a transport error
                    Err(e) => {
                        let text = String::from_utf8_lossy(&body);
                        match (&tag, trailer_tag(text.trim())) {
                            (Some(want), Some(got)) if want == got => break,
                            _ => return Err(e),
                        }
                    }
                }
            }
            let text = String::from_utf8(body)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
            Ok::<_, reqwest::Error>((status, code, text))
        })
        .await;
        let Ok(sent) = sent else {
            let secs = self.deadline.as_secs();
            let killed = match guard.fire() {
                Some(task) => task.await.unwrap_or_else(|e| Err(e.into())),
                None => Err(anyhow::anyhow!("no runtime to send KILL QUERY from")),
            };
            return Err(match killed {
                Ok(true) => anyhow::anyhow!(
                    "ClickHouse query did not finish within {secs}s and was cancelled"
                ),
                Ok(false) => anyhow::anyhow!(
                    "ClickHouse query did not finish within {secs}s and could not be cancelled \
                     (KILL QUERY matched no query; behind a load balancer it may have reached \
                     another replica); it may still be running"
                ),
                Err(e) => anyhow::anyhow!(
                    "ClickHouse query did not finish within {secs}s and could not be cancelled \
                     ({e}); it may still be running"
                ),
            });
        };
        let (status, code, text) = match sent {
            // A success, or an error with ClickHouse's exception header, means ClickHouse answered
            // and the statement has ended. A reply without that header (a gateway's 502 or 504)
            // proves nothing, so the guard stays armed
            Ok(reply) => {
                if reply.0.is_success() || reply.1.is_some() {
                    guard.disarm();
                }
                reply
            }
            // Only a connect error proves the statement never reached the server. After any other
            // error (say, the connection drops while we read the reply) it may still run
            Err(e) => {
                if e.is_connect() {
                    guard.disarm();
                }
                return Err(e.into());
            }
        };
        if !status.is_success() {
            return Err(ServerError {
                code,
                message: server_message(code, &text),
            }
            .into());
        }
        Ok(text)
    }

    /// Sends with read-only and the timeout, drops a refused setting and sends again, and returns
    /// the guards it finally sent. What each account type accepts (measured on 26.9.12.8):
    ///
    /// - normal account: both, and the server enforces both. Use `readonly=1`, not 2: with
    ///   `readonly=2`, `SELECT … SETTINGS max_execution_time = 0` turns the timeout off;
    /// - profile with `readonly=1`: `max_execution_time` gets READONLY (164); `readonly=1` is
    ///   the current value, so the server accepts it;
    /// - profile with `readonly=2`: the reverse (`readonly=1` refused); statements can change it;
    /// - `max_execution_time` locked by a constraint (`CONST`): SETTING_CONSTRAINT_VIOLATION (452).
    ///
    /// Drop `readonly` only if the account itself is read-only: a statement with `SETTINGS
    /// readonly = 0` gets the same "Cannot modify 'readonly'", and dropping it would open the gate
    async fn negotiated(&self, client: &Client, body: &str) -> anyhow::Result<(String, Guards)> {
        let mut guards = Guards::ALL;
        loop {
            if guards != Guards::ALL {
                self.can_stop_in_time(client, guards).await?;
            }
            let err = match self.post(client, body, guards).await {
                Ok(text) => return Ok((text, guards)),
                Err(e) => e,
            };
            let refused = err
                .downcast_ref::<ServerError>()
                .and_then(|e| refused_setting(e.code, &e.message));
            match refused {
                Some(Refused::Timeout) if guards.timeout => guards.timeout = false,
                Some(Refused::ReadOnly) if guards.readonly => {
                    // If the probe fails or the account itself is 0, return the original refusal
                    let own = self.account_setting(client, "readonly").await;
                    if !matches!(own.as_deref(), Ok("1" | "2")) {
                        return Err(err);
                    }
                    guards.readonly = false;
                }
                _ => return Err(err),
            }
        }
    }

    /// One setting of the account itself (from its profile), asked without settings. FORMAT is
    /// fixed: a profile can set `default_format` to JSONEachRow, which returns a JSON object
    async fn account_setting(&self, client: &Client, name: &str) -> anyhow::Result<String> {
        let sql = format!("SELECT getSetting({}) FORMAT TabSeparated", literal(name));
        Ok(self
            .post(client, &sql, Guards::NONE)
            .await?
            .trim()
            .to_string())
    }

    /// Runs before a statement goes out without one of the guards. The server holds the time
    /// only with both guards, so otherwise only KILL is left: try it now on an unused id, before
    /// the statement runs. If KILL fails, an account running under readonly=1 (a profile or our
    /// own setting) may rely on its own max_execution_time, including a CONST one, but
    /// only if it is not longer than ours: a 3600 s limit does not hold our 10 s
    async fn can_stop_in_time(&self, client: &Client, guards: Guards) -> anyhow::Result<()> {
        // A fresh id never matches, so only the permission counts here
        let Err(reason) = kill_query(client, &self.conn, &query_id()).await else {
            return Ok(());
        };
        let limit = if guards.readonly {
            let own = self.account_setting(client, "max_execution_time").await;
            own.ok().and_then(|t| t.parse::<f64>().ok())
        } else {
            None
        };
        if limit.is_some_and(|t| t > 0.0 && t <= f64::from(STATEMENT_TIMEOUT_SECS)) {
            return Ok(());
        }
        anyhow::bail!(
            "This ClickHouse account cannot cancel its own queries ({reason}), so nothing would \
             stop a query at the {STATEMENT_TIMEOUT_SECS}s timeout. Grant it SELECT ON \
             system.processes, or set max_execution_time to at most {STATEMENT_TIMEOUT_SECS} in \
             its read-only settings profile."
        )
    }

    async fn run(&self, client: &Client, sql: &str) -> anyhow::Result<(Reply, Guards)> {
        let (text, guards) = self
            .negotiated(client, &format!("{sql}\nFORMAT JSONCompactStrings"))
            .await?;
        let reply: Reply =
            serde_json::from_str(&text).map_err(|_| match trailer_message(text.trim()) {
                Some(message) => anyhow::anyhow!("{message}"),
                None => anyhow::anyhow!(
                    "ClickHouse returned an unreadable result: {}",
                    tail(&text, 300).trim()
                ),
            })?;
        if let Some(e) = &reply.exception {
            anyhow::bail!("{}", e.trim());
        }
        Ok((reply, guards))
    }
}

/// Which of our settings the error refuses: `Cannot modify 'X' setting in readonly mode` (164)
/// for a read-only account, `Setting X should not be changed` (452) for a constraint. A write
/// that read-only blocks is also 164, but its message names no setting
fn refused_setting(code: Option<u32>, message: &str) -> Option<Refused> {
    if !matches!(code, Some(164) | Some(452)) {
        return None;
    }
    let names = |name: &str| {
        message.contains(&format!("'{name}' setting"))
            || message.contains(&format!("Setting {name} "))
    };
    if names("max_execution_time") {
        Some(Refused::Timeout)
    } else if names("readonly") {
        Some(Refused::ReadOnly)
    } else {
        None
    }
}

/// Strips `LowCardinality(...)` / `Nullable(...)` (they can nest); the bool means "can be NULL"
fn unwrap_type(type_name: &str) -> (&str, bool) {
    let mut ty = type_name.trim();
    let mut nullable = false;
    loop {
        if let Some(inner) = ty
            .strip_prefix("Nullable(")
            .and_then(|t| t.strip_suffix(')'))
        {
            ty = inner;
            nullable = true;
        } else if let Some(inner) = ty
            .strip_prefix("LowCardinality(")
            .and_then(|t| t.strip_suffix(')'))
        {
            ty = inner;
        } else {
            return (ty, nullable);
        }
    }
}

/// One cell: a `JSONCompactStrings` string becomes a JSON value.
///
/// Not `JSONCompact`: quotes on 64-bit and wider integers depend on the server's
/// `output_format_json_quote_64bit_integers` (no quotes by default since 25.8, quotes before),
/// and a read-only account cannot change it. serde_json reads an unquoted 128 / 256-bit integer
/// as f64 and rounds it. Not `coerce` either: its number list has Snowflake's `FIXED`, so the
/// zip code `'02134'` in a `FixedString(5)` becomes 2134. Only these types convert:
///
/// - integers: a number if it fits i64 / u64, else (128 / 256 bits) an exact string. An exact
///   string is better for the model than a number with wrong last digits;
/// - `Decimal(P, S)`: a number if P ≤ 15 (f64 holds it exactly), else a string, same reason;
/// - floats become numbers (`nan` / `inf` stay strings: not JSON numbers), `Bool` a bool;
/// - NULL only in `Nullable(...)` columns, and the ASCII `NULL` only in non-string columns. A
///   `Nullable(String)` cell that holds that text is ambiguous: the cost of exact integers
fn cell(type_name: &str, raw: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let (base, nullable) = unwrap_type(type_name);
    let Value::String(s) = raw else {
        return raw.clone();
    };
    let text_type = base == "String"
        || ["FixedString(", "Enum8(", "Enum16("]
            .iter()
            .any(|t| base.starts_with(t));
    if nullable && (s == NULL_TEXT || (s == NULL_TEXT_ASCII && !text_type)) {
        return Value::Null;
    }
    let integer = base
        .strip_prefix("UInt")
        .or_else(|| base.strip_prefix("Int"))
        .is_some_and(|bits| !bits.is_empty() && bits.chars().all(|c| c.is_ascii_digit()));
    if integer {
        if let Ok(n) = s.parse::<i64>() {
            return n.into();
        }
        if let Ok(n) = s.parse::<u64>() {
            return n.into();
        }
        return raw.clone();
    }
    let short_decimal = base
        .strip_prefix("Decimal(")
        .and_then(|rest| rest.split(',').next())
        .and_then(|p| p.trim().parse::<u32>().ok())
        .is_some_and(|p| p <= 15);
    if short_decimal || matches!(base, "Float32" | "Float64" | "BFloat16") {
        return coerce("DOUBLE", raw);
    }
    if base == "Bool" {
        return coerce("BOOL", raw);
    }
    raw.clone()
}

#[async_trait::async_trait]
impl QueryEngine for ClickHouseEngine {
    async fn test(&self) -> anyhow::Result<()> {
        self.run(&super::http()?, "SELECT 1").await.map(|_| ())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        // With a database in the URL, read only that one: the default database is often empty, and
        // without a scope the whole server goes into the schema document. Keep `type` as is, with
        // `Nullable(...)` / `LowCardinality(...)`: the first says NULLs occur, the second usually
        // marks a dimension column. Both help, and the SQL is the same as for the inner type
        let scope = match &self.conn.database {
            Some(db) => format!("database = {}", literal(db)),
            None => "database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema')".into(),
        };
        let client = super::http()?;
        let (reply, _) = self
            .run(
                &client,
                &format!(
                    "SELECT database, table, name, type, comment FROM system.columns \
                     WHERE {scope} ORDER BY database, table, position"
                ),
            )
            .await?;
        Ok(reply
            .data
            .into_iter()
            .map(super::trino::schema_row)
            .collect())
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult> {
        let client = super::http()?;
        let (reply, _) = self.run(&client, &wrap_limit(sql)).await?;
        let columns: Vec<String> = reply.meta.iter().map(|m| m.name.clone()).collect();
        let rows: Vec<Vec<serde_json::Value>> = reply
            .data
            .iter()
            .map(|row| {
                row.iter()
                    .zip(&reply.meta)
                    .map(|(v, m)| cell(&m.type_name, v))
                    .collect()
            })
            .collect();
        let (rows, truncated) = truncate_rows(rows);
        Ok(QueryResult {
            rows: rows_to_json_lines(&columns, &rows),
            truncated,
        })
    }
}

#[cfg(test)]
mod live_tests {
    use super::super::conn::ClickHouseConn;
    use super::super::QueryEngine;
    use super::ClickHouseEngine;
    use std::time::{Duration, Instant};

    /// The real-server test; it skips without `UTOPIA_TEST_CLICKHOUSE_URL` (measured on 26.9.12.8):
    /// `docker run -d -p 18124:8123 -e CLICKHOUSE_PASSWORD=pw -e
    /// CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT=1 clickhouse/clickhouse-server:26.9.12.8`, run the SQL
    /// below as default, then once per account, e.g. `clickhouse://ro:pw@127.0.0.1:18124/default`:
    /// ```sql
    /// CREATE TABLE orders (id UInt64, region LowCardinality(String), note Nullable(String),
    ///   amount Decimal(12, 2) COMMENT 'CNY', big UInt64) ENGINE = MergeTree ORDER BY id;
    /// INSERT INTO orders VALUES (1, 'east', NULL, 1234.56, 18446744073709551615);
    /// CREATE USER ro IDENTIFIED BY 'pw' SETTINGS readonly = 1;
    /// CREATE USER ro2 IDENTIFIED BY 'pw' SETTINGS readonly = 2;
    /// CREATE USER pinned IDENTIFIED BY 'pw' SETTINGS max_execution_time = 30 CONST;
    /// CREATE USER nokill IDENTIFIED BY 'pw' SETTINGS readonly = 1;
    /// GRANT SELECT, INSERT, CREATE TABLE ON default.* TO ro, ro2, pinned, nokill;
    /// GRANT SELECT ON system.processes TO ro, ro2, pinned; -- the engine needs it; nokill has none
    /// ```
    fn live_url(var: &str) -> Option<String> {
        std::env::var(var).ok().filter(|u| !u.trim().is_empty())
    }

    #[tokio::test]
    async fn a_live_server_answers_read_only_and_within_the_timeout() {
        let Some(url) = live_url("UTOPIA_TEST_CLICKHOUSE_URL") else {
            return;
        };
        let engine = ClickHouseEngine::new(ClickHouseConn::parse(&url).expect("parse url"));
        engine.test().await.expect("test()");

        let schema = engine.fetch_schema().await.expect("schema");
        let col = |name: &str| {
            schema
                .iter()
                .find(|c| c.table == "orders" && c.column == name)
                .unwrap_or_else(|| panic!("orders.{name}"))
        };
        assert_eq!(col("region").data_type, "LowCardinality(String)");
        assert_eq!(col("note").data_type, "Nullable(String)");
        assert_eq!(col("amount").comment.as_deref(), Some("CNY"));
        assert_eq!(
            col("note").comment,
            None,
            "an empty comment is filtered out"
        );
        assert!(!col("id").is_primary_key, "a sort key is not a unique key");

        // The UInt64 maximum would become 18446744073709552000 through f64
        let r = engine
            .execute("SELECT id, region, note, amount, big FROM orders ORDER BY id")
            .await
            .expect("execute");
        assert_eq!(
            r.rows,
            vec![
                r#"{"id":1,"region":"east","note":null,"amount":1234.56,"big":18446744073709551615}"#
            ]
        );

        // The server refuses the write: the normal account through our readonly=1, the others
        // through their own settings. This bypasses the gate to prove layer 3
        let client = super::super::http().unwrap();
        let err = engine
            .negotiated(
                &client,
                "CREATE TABLE orders_probe (x UInt8) ENGINE = Memory",
            )
            .await
            .expect_err("a write must be refused")
            .to_string();
        assert!(err.contains("readonly mode"), "{err}");

        // Timeout: the server stops the query for the normal and readonly=2 accounts, our KILL
        // for the rest. Both must return near the deadline and leave no query on the server
        let started = Instant::now();
        let err = engine
            .execute("SELECT sum(sipHash64(number)) FROM numbers(1000000000000)")
            .await
            .expect_err("the slow query must be stopped")
            .to_string();
        let took = started.elapsed();
        assert!(
            err.contains("TIMEOUT_EXCEEDED") || err.contains("did not finish"),
            "{err}"
        );
        assert!(
            took >= Duration::from_secs(9) && took < engine.deadline + Duration::from_secs(3),
            "took {took:?}: {err}"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (left, _) = engine
                    .run(
                        &client,
                        "SELECT count() FROM system.processes WHERE \
                         query LIKE '%numbers(1000000000000)%' AND query NOT LIKE '%system.processes%'",
                    )
                    .await
                    .expect("processes");
                if left.data[0][0] == serde_json::json!("0") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the slow query is still on the server: {err}"));
    }

    /// An account that cannot kill its queries and has no short profile limit is refused, on Test
    /// connection and on every query. Point `UTOPIA_TEST_CLICKHOUSE_NO_KILL_URL` at `nokill` above
    #[tokio::test]
    async fn an_account_that_cannot_kill_its_queries_fails_the_test() {
        let Some(url) = live_url("UTOPIA_TEST_CLICKHOUSE_NO_KILL_URL") else {
            return;
        };
        let engine = ClickHouseEngine::new(ClickHouseConn::parse(&url).expect("parse url"));
        let err = engine
            .test()
            .await
            .expect_err("test() must refuse")
            .to_string();
        assert!(err.contains("SELECT ON system.processes"), "{err}");
        let err = engine
            .execute("SELECT 1")
            .await
            .expect_err("a query must be refused without test() too")
            .to_string();
        assert!(err.contains("SELECT ON system.processes"), "{err}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::conn::ClickHouseConn;
    use super::super::{QueryEngine, QueryResult};
    use super::{cell, literal, refused_setting, unwrap_type, ClickHouseEngine, Refused};
    use serde_json::json;
    use std::time::Duration;
    use wiremock::matchers::{
        body_string_contains, method, query_param, query_param_contains, query_param_is_missing,
    };
    use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

    const REFUSED_TIMEOUT: &str =
        "Cannot modify 'max_execution_time' setting in readonly mode. (READONLY)";
    const REFUSED_READONLY: &str = "Cannot modify 'readonly' setting in readonly mode. (READONLY)";
    const SLOW: &str = "SELECT sum(number) AS s FROM numbers(1000000000000)";

    fn engine(server: &MockServer) -> ClickHouseEngine {
        let host = server.uri().trim_start_matches("http://").to_string();
        ClickHouseEngine::new(
            ClickHouseConn::parse(&format!("clickhouse://u:p@{host}/sales?ssl=false")).unwrap(),
        )
    }

    fn mock_post() -> MockBuilder {
        Mock::given(method("POST"))
    }

    /// Each mock must get exactly one request
    async fn once(server: &MockServer, mock: MockBuilder, response: ResponseTemplate) {
        mock.respond_with(response).expect(1).mount(server).await;
    }

    fn reply(meta: &[(&str, &str)], data: serde_json::Value) -> ResponseTemplate {
        let meta: Vec<_> = meta
            .iter()
            .map(|(n, t)| json!({ "name": n, "type": t }))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({ "meta": meta, "data": data, "rows": 1 }))
    }

    fn refusal(code: u32, message: &str) -> ResponseTemplate {
        ResponseTemplate::new(500)
            .insert_header("X-ClickHouse-Exception-Code", code.to_string().as_str())
            .set_body_string(format!("Code: {code}. DB::Exception: {message}"))
    }

    fn text(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_string(format!("{body}\n"))
    }

    #[test]
    fn values_come_back_by_column_type() {
        let i128_min = "-170141183460469231731687303715884105728";
        let decimal38 = "12345678901234567890.0123456789";
        for (ty, raw, want) in [
            (
                "UInt64",
                "18446744073709551615",
                json!(18446744073709551615u64),
            ),
            ("Int64", "-5", json!(-5)),
            // Wider than 64 bits stays an exact string, not an approximation
            ("Int128", i128_min, json!(i128_min)),
            ("Nullable(Int32)", "ᴺᵁᴸᴸ", json!(null)),
            ("Nullable(Int32)", "NULL", json!(null)),
            ("LowCardinality(Nullable(String))", "ᴺᵁᴸᴸ", json!(null)),
            // Text NULL in a string column is ambiguous, so it stays; non-Nullable means no NULL
            ("Nullable(String)", "NULL", json!("NULL")),
            ("String", "ᴺᵁᴸᴸ", json!("ᴺᵁᴸᴸ")),
            // An Enum label is text too: a label `NULL` stays, the real marker is null
            (
                "Nullable(Enum8('NULL' = 1, 'z' = 2))",
                "NULL",
                json!("NULL"),
            ),
            ("Nullable(Enum8('NULL' = 1, 'z' = 2))", "ᴺᵁᴸᴸ", json!(null)),
            // Leading zeros are part of the value
            ("FixedString(5)", "02134", json!("02134")),
            ("Decimal(12, 2)", "1234.56", json!(1234.56)),
            ("Decimal(38, 10)", decimal38, json!(decimal38)),
            ("Nullable(Bool)", "true", json!(true)),
            ("Float64", "1.5", json!(1.5)),
            ("Float64", "nan", json!("nan")),
            ("Date", "2023-06-01", json!("2023-06-01")),
            ("IntervalDay", "3", json!("3")),
            ("Array(UInt8)", "[1,2]", json!("[1,2]")),
        ] {
            assert_eq!(cell(ty, &json!(raw)), want, "{ty} {raw}");
        }
    }

    /// ClickHouse reads a backslash as an escape: `'sales\north'` would name `sales<newline>orth`
    #[test]
    fn a_literal_escapes_backslashes_and_quotes() {
        assert_eq!(literal(r"sales\north's"), r"'sales\\north\'s'");
    }

    #[tokio::test]
    async fn the_schema_filter_keeps_a_backslash_in_the_database_name() {
        let server = MockServer::start().await;
        let host = server.uri().trim_start_matches("http://").to_string();
        let filter = mock_post().and(body_string_contains(r"database = 'sales\\north'"));
        once(&server, filter, reply(&[("database", "String")], json!([]))).await;
        let conn = ClickHouseConn::parse(&format!("clickhouse://u@{host}/sales%5Cnorth")).unwrap();
        ClickHouseEngine::new(conn).fetch_schema().await.unwrap();
    }

    #[test]
    fn wrappers_come_off_in_any_order() {
        for (ty, want) in [
            ("LowCardinality(Nullable(String))", ("String", true)),
            ("Nullable(Decimal(12, 2))", ("Decimal(12, 2)", true)),
            ("LowCardinality(String)", ("String", false)),
            ("Array(Nullable(UInt8))", ("Array(Nullable(UInt8))", false)),
        ] {
            assert_eq!(unwrap_type(ty), want, "{ty}");
        }
    }

    #[test]
    fn only_a_refused_setting_counts_as_one() {
        let constraint =
            "Setting max_execution_time should not be changed. (SETTING_CONSTRAINT_VIOLATION)";
        for (code, message, want) in [
            (164, REFUSED_TIMEOUT, Some(Refused::Timeout)),
            (164, REFUSED_READONLY, Some(Refused::ReadOnly)),
            (452, constraint, Some(Refused::Timeout)),
            // A write that read-only blocks: also 164, but no setting was refused
            (
                164,
                "rw: Cannot execute query in readonly mode. (READONLY)",
                None,
            ),
            (62, "Syntax error: 'readonly' setting", None),
        ] {
            assert_eq!(refused_setting(Some(code), message), want, "{message}");
        }
    }

    #[tokio::test]
    async fn sends_read_only_and_a_timeout_and_keeps_column_order() {
        let server = MockServer::start().await;
        let request = mock_post()
            .and(query_param("readonly", "1"))
            .and(query_param("max_execution_time", "10"))
            .and(query_param("database", "sales"))
            .and(body_string_contains("LIMIT 201"))
            .and(body_string_contains("FORMAT JSONCompactStrings"));
        let meta = [
            ("region", "LowCardinality(String)"),
            ("total", "UInt64"),
            ("note", "Nullable(String)"),
        ];
        let data = json!([["east", "18446744073709551615", "ᴺᵁᴸᴸ"]]);
        once(&server, request, reply(&meta, data)).await;
        let out = engine(&server)
            .execute("SELECT region, total, note FROM orders")
            .await
            .unwrap();
        assert_eq!(
            out.rows,
            vec![r#"{"region":"east","total":18446744073709551615,"note":null}"#]
        );
        assert!(!out.truncated);
    }

    /// An error after the result started: valid JSON, status 200, half the data
    #[tokio::test]
    async fn an_exception_inside_the_result_is_an_error() {
        let server = MockServer::start().await;
        let half = json!({
            "meta": [{ "name": "number", "type": "UInt64" }],
            "data": [["0"]],
            "exception": "Code: 396. DB::Exception: Limit for result exceeded. (TOO_MANY_ROWS_OR_BYTES)"
        });
        once(
            &server,
            mock_post(),
            ResponseTemplate::new(200).set_body_json(half),
        )
        .await;
        let err = engine(&server).execute(SLOW).await.unwrap_err().to_string();
        assert!(err.contains("TOO_MANY_ROWS_OR_BYTES"), "{err}");
    }

    /// The same error with a gzip reply: status 500, and the rows come before the message.
    /// The message quotes `Code: 395.` once more, as ClickHouse does with a literal
    #[tokio::test]
    async fn an_error_after_rows_keeps_only_the_message() {
        let server = MockServer::start().await;
        let rows = format!(
            r#"{{"meta":[{{"name":"s","type":"String"}}],"data":[["{}"]"#,
            "x".repeat(100_000)
        );
        let message = "Code: 395. DB::Exception: boom: while executing \
                       'throwIf(..., 'Code: 395. boom'_String)'. (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO)";
        let reply = ResponseTemplate::new(500)
            .insert_header("X-ClickHouse-Exception-Code", "395")
            .set_body_string(format!("{rows}{message}\n"));
        once(&server, mock_post(), reply).await;
        let err = engine(&server).execute(SLOW).await.unwrap_err().to_string();
        assert_eq!(err, message);
    }

    /// When the headers have already gone out, the same error comes as status 200 with an
    /// `__exception__` block
    #[tokio::test]
    async fn an_error_block_after_rows_keeps_only_the_message() {
        let server = MockServer::start().await;
        let message = "Code: 395. DB::Exception: boom. (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO)";
        let body = format!(
            "{{\n\t\"meta\": [{{\"name\": \"s\", \"type\": \"String\"}}],\n\t\"data\":\n\t[\n\t\t[\"0\"]\r\n\
             __exception__\r\nabcdef\r\n{message}\n{} abcdef\r\n__exception__\r\n",
            message.len() + 1
        );
        once(
            &server,
            mock_post(),
            ResponseTemplate::new(200).set_body_string(body),
        )
        .await;
        let err = engine(&server).execute(SLOW).await.unwrap_err().to_string();
        assert_eq!(err, message);
    }

    /// A long ordinary error keeps its start, which names the cause
    #[tokio::test]
    async fn a_long_error_stays_whole() {
        let server = MockServer::start().await;
        let message = format!(
            "Code: 47. DB::Exception: Unknown expression identifier `missing_col`. In scope {}. \
             (UNKNOWN_IDENTIFIER)",
            "SELECT a AS b, ".repeat(200)
        );
        once(
            &server,
            mock_post(),
            ResponseTemplate::new(500)
                .insert_header("X-ClickHouse-Exception-Code", "47")
                .set_body_string(message.clone()),
        )
        .await;
        let err = engine(&server).execute(SLOW).await.unwrap_err().to_string();
        assert_eq!(err, message);
    }

    /// A readonly=1 profile: max_execution_time is refused and dropped, readonly=1 stays.
    /// The server no longer holds the time, so `test()` also tries a KILL
    #[tokio::test]
    async fn a_refused_timeout_is_dropped_and_read_only_stays() {
        let server = MockServer::start().await;
        let first = mock_post().and(query_param("max_execution_time", "10"));
        once(&server, first, refusal(164, REFUSED_TIMEOUT)).await;
        let retry = mock_post()
            .and(query_param_is_missing("max_execution_time"))
            .and(query_param("readonly", "1"));
        once(&server, retry, reply(&[("1", "UInt8")], json!([["1"]]))).await;
        let kill = mock_post().and(body_string_contains("KILL QUERY"));
        once(&server, kill, ResponseTemplate::new(200)).await;
        engine(&server).test().await.unwrap();
    }

    /// A readonly=1 account that cannot kill its queries, queried with `execute()` alone (no
    /// `test()`). `profile_limit` is its profile's `max_execution_time` (`None`: the probe fails).
    /// The retry without the timeout must go out `retries` times
    async fn query_without_kill(
        profile_limit: Option<&str>,
        retries: u64,
    ) -> anyhow::Result<QueryResult> {
        let server = MockServer::start().await;
        let first = mock_post().and(query_param("max_execution_time", "10"));
        once(&server, first, refusal(164, REFUSED_TIMEOUT)).await;
        let no_grant = "Not enough privileges. To execute this query, it's necessary to have \
                        the grant SELECT ON system.processes. (ACCESS_DENIED)";
        let kill = mock_post().and(body_string_contains("KILL QUERY"));
        once(&server, kill, refusal(497, no_grant)).await;
        let probe = mock_post().and(body_string_contains("getSetting('max_execution_time')"));
        let limit = profile_limit.map_or_else(|| refusal(497, "(ACCESS_DENIED)"), text);
        once(&server, probe, limit).await;
        mock_post()
            .and(query_param_is_missing("max_execution_time"))
            .and(body_string_contains("SELECT 7 AS x"))
            .respond_with(reply(&[("x", "UInt8")], json!([["7"]])))
            .expect(retries)
            .mount(&server)
            .await;
        engine(&server).execute("SELECT 7 AS x").await
    }

    /// No KILL, and the profile limit cannot hold 10 s (none, too long, unknown): the query is
    /// refused with the grant message, and the retry without the timeout never goes out
    #[tokio::test]
    async fn a_query_that_nothing_can_stop_in_time_is_refused() {
        for limit in [Some("0"), Some("3600"), None] {
            let err = query_without_kill(limit, 0).await.unwrap_err().to_string();
            assert!(
                err.contains("Grant it SELECT ON system.processes"),
                "{limit:?}: {err}"
            );
            assert!(err.contains("ACCESS_DENIED"), "{limit:?}: {err}");
        }
    }

    /// No KILL, but the profile limit is 10 s or less: the server holds it, so the query runs
    #[tokio::test]
    async fn a_short_profile_limit_stands_in_for_kill() {
        for limit in ["5", "10"] {
            let out = query_without_kill(Some(limit), 1).await.unwrap();
            assert_eq!(out.rows, vec![r#"{"x":7}"#], "{limit}");
        }
    }

    /// readonly=1 is refused (a readonly=2 profile, or a statement's own `SETTINGS readonly = 0`),
    /// and the account's own readonly is `own`. Only a read-only account drops readonly and retries
    async fn readonly_refused(own: &str) -> anyhow::Result<QueryResult> {
        let server = MockServer::start().await;
        let probe = mock_post().and(body_string_contains(
            "getSetting('readonly') FORMAT TabSeparated",
        ));
        once(&server, probe, text(own)).await;
        let first = mock_post().and(query_param("readonly", "1"));
        once(&server, first, refusal(164, REFUSED_READONLY)).await;
        // A read-only account resends the same statement without readonly, after a KILL check
        // (statements can lift its timeout); a writable one must not
        let retries = if own == "0" { 0 } else { 1 };
        mock_post()
            .and(body_string_contains("KILL QUERY"))
            .respond_with(ResponseTemplate::new(200))
            .expect(retries)
            .mount(&server)
            .await;
        mock_post()
            .and(query_param_is_missing("readonly"))
            .and(query_param("max_execution_time", "10"))
            .and(body_string_contains("SELECT 7 AS x"))
            .and(body_string_contains("LIMIT 201"))
            .respond_with(reply(&[("x", "UInt8")], json!([["7"]])))
            .expect(retries)
            .mount(&server)
            .await;
        engine(&server).execute("SELECT 7 AS x").await
    }

    #[tokio::test]
    async fn read_only_is_dropped_only_for_an_account_that_is_read_only_itself() {
        let out = readonly_refused("2")
            .await
            .expect("a readonly=2 account retries without readonly=1");
        assert_eq!(out.rows, vec![r#"{"x":7}"#]);
        let err = readonly_refused("0")
            .await
            .expect_err("a writable account keeps readonly=1 and the refusal")
            .to_string();
        assert!(err.contains("Cannot modify 'readonly'"), "{err}");
    }

    /// A slow query that answers after 5 s, plus a KILL endpoint (no timeout, exactly one hit)
    /// `matched`: the KILL reply has the row ClickHouse gives for a matched query, or is empty
    async fn slow_server(matched: bool) -> MockServer {
        let server = MockServer::start().await;
        mock_post()
            .and(body_string_contains("KILL QUERY WHERE query_id = 'utopia-"))
            .and(query_param_is_missing("max_execution_time"))
            .respond_with(move |req: &wiremock::Request| {
                let body = String::from_utf8_lossy(&req.body);
                let id = body.split('\'').nth(1).unwrap_or_default();
                let row = format!("waiting\t{id}\tu\tSELECT ...\n");
                ResponseTemplate::new(200).set_body_string(if matched {
                    row
                } else {
                    String::new()
                })
            })
            .expect(1)
            .mount(&server)
            .await;
        let slow = reply(&[("s", "UInt64")], json!([["1"]])).set_delay(Duration::from_secs(5));
        mock_post()
            .and(query_param_contains("query_id", "utopia-"))
            .and(body_string_contains("numbers"))
            .respond_with(slow)
            .mount(&server)
            .await;
        server
    }

    /// The KILL names the slow query's `query_id`. The guard sends it from another task, so wait
    async fn assert_killed_the_slow_query(server: &MockServer) {
        let requests = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let requests = server.received_requests().await.unwrap_or_default();
                if requests.len() >= 2 {
                    break requests;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("no KILL was sent");
        let body = |r: &wiremock::Request| String::from_utf8_lossy(&r.body).into_owned();
        let slow = requests
            .iter()
            .find(|r| body(r).contains("numbers"))
            .and_then(|r| r.url.query_pairs().find(|(k, _)| k == "query_id"))
            .map(|(_, v)| v.into_owned())
            .expect("the slow query carries a query_id");
        let kill = requests
            .iter()
            .map(body)
            .find(|b| b.contains("KILL QUERY"))
            .expect("a KILL was sent");
        assert!(kill.contains(&format!("'{slow}'")), "KILL names {slow}");
    }

    /// No reply by the deadline: a timeout error, and the query is killed
    #[tokio::test]
    async fn a_query_past_the_deadline_is_killed() {
        let server = slow_server(true).await;
        let mut ch = engine(&server);
        ch.deadline = Duration::from_millis(300);
        let err = ch.execute(SLOW).await.unwrap_err().to_string();
        assert!(
            err.contains("did not finish") && err.contains("was cancelled"),
            "{err}"
        );
        assert_killed_the_slow_query(&server).await;
    }

    /// The KILL matched nothing (say, it reached another replica): not cancelled
    #[tokio::test]
    async fn a_kill_that_matches_nothing_is_not_reported_as_cancelled() {
        let server = slow_server(false).await;
        let mut ch = engine(&server);
        ch.deadline = Duration::from_millis(300);
        let err = ch.execute(SLOW).await.unwrap_err().to_string();
        assert!(
            err.contains("could not be cancelled") && err.contains("may still be running"),
            "{err}"
        );
    }

    /// The caller stops waiting (Stop in chat): the future is dropped, and the guard still kills it
    #[tokio::test]
    async fn a_dropped_query_is_killed_too() {
        let server = slow_server(true).await;
        let ch = engine(&server);
        let stopped = tokio::time::timeout(Duration::from_millis(300), ch.execute(SLOW)).await;
        assert!(stopped.is_err(), "the query should still be running");
        assert_killed_the_slow_query(&server).await;
    }

    /// Reads one HTTP request (head and body) from a raw socket
    async fn read_request(sock: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
        loop {
            let n = sock.read(&mut chunk).await.unwrap_or(0);
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).into_owned();
            let body_len = text
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::to_owned)
                })
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            match text.find("\r\n\r\n") {
                Some(end) if buf.len() >= end + 4 + body_len => return text,
                _ if n == 0 => return text,
                _ => {}
            }
        }
    }

    /// The server takes the statement, then the connection drops while we read the reply. The
    /// query may still run, so the guard kills it
    #[tokio::test]
    async fn a_reply_cut_short_kills_the_query() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (seen, mut requests) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let request = read_request(&mut sock).await;
                let reply: &[u8] = if request.contains("KILL QUERY") {
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n"
                } else {
                    b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{\"meta\""
                };
                let _ = sock.write_all(reply).await;
                let _ = seen.send(request);
            }
        });
        let conn = ClickHouseConn::parse(&format!("clickhouse://u@{addr}/sales")).unwrap();
        let err = ClickHouseEngine::new(conn).execute(SLOW).await.unwrap_err();
        assert!(!err.to_string().contains("did not finish"), "{err}");
        let wait = Duration::from_secs(5);
        let statement = tokio::time::timeout(wait, requests.recv())
            .await
            .expect("the statement")
            .unwrap();
        let id = statement
            .split(['?', '&', ' '])
            .find_map(|p| p.strip_prefix("query_id="))
            .expect("the statement carries a query_id")
            .to_string();
        let kill = tokio::time::timeout(wait, requests.recv())
            .await
            .expect("a KILL was sent")
            .unwrap();
        assert!(
            kill.contains("KILL QUERY") && kill.contains(&format!("'{id}'")),
            "{kill}"
        );
    }

    /// A refused connection never reached the server, so there is nothing to kill
    #[tokio::test]
    async fn a_refused_connection_sends_no_kill() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let conn =
            ClickHouseConn::parse(&format!("clickhouse://u@127.0.0.1:{port}/sales")).unwrap();
        let err = ClickHouseEngine::new(conn).execute(SLOW).await.unwrap_err();
        assert!(
            format!("{err:?}").to_lowercase().contains("connect"),
            "{err:?}"
        );
        // This test runs on one thread, so a KILL the guard spawned has not run yet. Open the
        // port now, before the next await, and listen for it
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let knock = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await;
        assert!(knock.is_err(), "a KILL was sent");
    }

    /// A gateway in front of ClickHouse gives up and answers a full 502 without ClickHouse's
    /// header: the query may still run, so the guard kills it. A ClickHouse error reply (with
    /// the header) means the statement has ended, so nothing is killed
    #[tokio::test]
    async fn a_gateway_error_kills_the_query_and_a_clickhouse_error_does_not() {
        for (answer, kills) in [
            (ResponseTemplate::new(502).set_body_string("Bad Gateway"), 1),
            (refusal(62, "Syntax error. (SYNTAX_ERROR)"), 0),
        ] {
            let server = MockServer::start().await;
            mock_post()
                .and(body_string_contains("KILL QUERY"))
                .respond_with(ResponseTemplate::new(200))
                .expect(kills)
                .mount(&server)
                .await;
            let slow = mock_post().and(body_string_contains("numbers"));
            once(&server, slow, answer).await;
            engine(&server).execute(SLOW).await.unwrap_err();
            if kills == 1 {
                assert_killed_the_slow_query(&server).await;
            } else {
                // Give a wrongly spawned KILL time to arrive before the mock checks its count
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}
