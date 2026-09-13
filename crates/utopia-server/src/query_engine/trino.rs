//! Trino（旧名 Presto）：REST 协议 `POST /v1/statement`，然后沿 `nextUri` 一页页取。
//! 一个引擎顶起整个湖仓——Iceberg / Delta / Hive / Hudi 都是它的 catalog，
//! 换格式不换协议。Starburst 同协议。
//!
//! 没有会话可设只读：超时靠 `X-Trino-Session: query_max_execution_time`，
//! 只读靠 `guard_sql_for`。

use super::conn::TrinoConn;
use super::{
    rows_to_json_lines, sql_literal, truncate_rows, wrap_limit, QueryEngine, QueryResult,
    SchemaColumn, HTTP_POLL_BUDGET, STATEMENT_TIMEOUT_SECS,
};
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::Deserialize;
use std::time::Instant;

pub struct TrinoEngine {
    conn: TrinoConn,
}

#[derive(Deserialize)]
struct Column {
    name: String,
}

#[derive(Deserialize)]
struct Page {
    #[serde(rename = "nextUri")]
    next_uri: Option<String>,
    columns: Option<Vec<Column>>,
    data: Option<Vec<Vec<serde_json::Value>>>,
    error: Option<TrinoError>,
}

#[derive(Deserialize)]
struct TrinoError {
    message: String,
    #[serde(rename = "errorName")]
    error_name: Option<String>,
}

impl TrinoEngine {
    pub fn new(conn: TrinoConn) -> Self {
        Self { conn }
    }

    fn headers(&self) -> anyhow::Result<HeaderMap> {
        let mut h = HeaderMap::new();
        h.insert("X-Trino-User", HeaderValue::from_str(&self.conn.user)?);
        h.insert("X-Trino-Source", HeaderValue::from_static("utopia"));
        h.insert(
            "X-Trino-Session",
            HeaderValue::from_str(&format!(
                "query_max_execution_time={STATEMENT_TIMEOUT_SECS}s"
            ))?,
        );
        if let Some(c) = &self.conn.catalog {
            h.insert("X-Trino-Catalog", HeaderValue::from_str(c)?);
        }
        if let Some(s) = &self.conn.schema {
            h.insert("X-Trino-Schema", HeaderValue::from_str(s)?);
        }
        if let Some(p) = &self.conn.password {
            let raw = format!("{}:{p}", self.conn.user);
            let token = base64::engine::general_purpose::STANDARD.encode(raw);
            h.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Basic {token}"))?,
            );
        }
        Ok(h)
    }

    /// 提交并沿 nextUri 收完：列在第一个带 columns 的页上，数据分页累积
    async fn run(&self, sql: &str) -> anyhow::Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        let client = super::http()?;
        let headers = self.headers()?;
        let mut page: Page = client
            .post(format!("{}/v1/statement", self.conn.base))
            .headers(headers.clone())
            .body(sql.to_string())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let started = Instant::now();
        let mut columns: Option<Vec<String>> = None;
        let mut rows = Vec::new();
        loop {
            if let Some(e) = page.error {
                let name = e.error_name.map(|n| format!("{n}: ")).unwrap_or_default();
                anyhow::bail!("{name}{}", e.message);
            }
            if columns.is_none() {
                columns = page
                    .columns
                    .take()
                    .map(|cs| cs.into_iter().map(|c| c.name).collect());
            }
            if let Some(d) = page.data.take() {
                rows.extend(d);
            }
            let Some(next) = page.next_uri.take() else {
                break;
            };
            if started.elapsed() > HTTP_POLL_BUDGET {
                anyhow::bail!(
                    "Trino query did not finish within {}s",
                    HTTP_POLL_BUDGET.as_secs()
                );
            }
            page = client
                .get(&next)
                .headers(headers.clone())
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
        }
        Ok((columns.unwrap_or_default(), rows))
    }
}

#[async_trait::async_trait]
impl QueryEngine for TrinoEngine {
    async fn test(&self) -> anyhow::Result<()> {
        self.run("SELECT 1").await.map(|_| ())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        let catalog = self.conn.catalog.as_deref().ok_or_else(|| {
            anyhow::anyhow!("trino://: put the catalog in the connection string (trino://user@host/CATALOG) so the schema can be read")
        })?;
        let schema_filter = self
            .conn
            .schema
            .as_deref()
            .map(|s| format!(" AND table_schema = {}", sql_literal(s)))
            .unwrap_or_default();
        let sql = format!(
            "SELECT table_schema, table_name, column_name, data_type, comment \
             FROM \"{}\".information_schema.columns \
             WHERE table_schema <> 'information_schema'{schema_filter} \
             ORDER BY table_schema, table_name, ordinal_position",
            catalog.replace('"', "\"\"")
        );
        let (_, rows) = self.run(&sql).await?;
        // 键是锦上添花：读不出来就照从前那样只给列，不让整次取 schema 失败（#502）。
        // Trino 引擎本身不强制 PK / FK 约束，`information_schema.table_constraints` 在
        // 大多数 catalog（Hive / Iceberg / Delta / TPC-H）里**也是空的**——读不到
        // 是预期，不是 bug。tracing::warn + 空 Keys，不让整次失败
        let keys = match keys(&self.conn, &self.headers()?, catalog).await {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(error = %e, "读不出 Trino 主键/外键，schema 不带键标记");
                Keys::default()
            }
        };
        Ok(rows.into_iter().map(|r| schema_row(r, &keys)).collect())
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult> {
        let (columns, rows) = self.run(&wrap_limit(sql)).await?;
        let (rows, truncated) = truncate_rows(rows);
        Ok(QueryResult {
            rows: rows_to_json_lines(&columns, &rows),
            truncated,
        })
    }
}

type ColumnKey = (String, String, String);

/// 一个 catalog 里的单列主键与单列外键，按 (schema, table, column) 查
///
/// Trino 与 PG 的差别：PG 的约束名在一张表内唯一（同一 schema 内可以有重名），
/// 而 Trino 的 `information_schema.table_constraints.constraint_name` 是 catalog
/// 全局唯一的。所以这里可以放心按 `(constraint_schema, constraint_name)` 连接，
/// 不会有 PG 那种「两张表都叫 `fk_ref`」的串表风险（trino-python-client #205）。
/// Trino 引擎本身不强制 PK / FK——这条 helper 在大多数 catalog 里会返回空集合，
/// 不是 bug
#[derive(Default)]
pub(crate) struct Keys {
    primary: std::collections::HashSet<ColumnKey>,
    /// 外键列 → 它指向的 `schema.table`
    foreign: std::collections::HashMap<ColumnKey, String>,
}

/// information_schema 的一行 → SchemaColumn（值可能是 null，comment 常是）
pub(crate) fn schema_row(row: Vec<serde_json::Value>, keys: &Keys) -> SchemaColumn {
    let text = |i: usize| -> String {
        row.get(i)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let schema = text(0);
    let table = text(1);
    let column = text(2);
    let key = (schema.clone(), table.clone(), column.clone());
    SchemaColumn {
        schema,
        table,
        column,
        data_type: text(3),
        comment: row
            .get(4)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        is_primary_key: keys.primary.contains(&key),
        references_table: keys.foreign.get(&key).cloned(),
    }
}

/// 读单列主键与单列外键。两段 SQL（PK / FK），各自拼出 `(schema, table, column)`。
///
/// - **PK** 走 `information_schema.table_constraints WHERE constraint_type='PRIMARY KEY'`
///   与 `key_column_usage` JOIN，对单列 PK 加 guard（约束下列数 = 1 + ordinal_position = 1）
/// - **FK** 走 `referential_constraints` 把外键约束名与对应唯一约束名配对，再
///   `key_column_usage` 各取一次——第一次的列名作为外键列，第二次的列名作为目标
///
/// 列名在 wire 上是字符串，sqlx 的 `query_as` 不参与：HTTP 引擎走的是 JSON
/// 解析路径（`run` 里把 data 拉成 `Vec<serde_json::Value>`），所以没有 PG / MySQL
/// 那种 `VARBINARY` 陷阱
async fn keys(conn: &TrinoConn, headers: &HeaderMap, catalog: &str) -> anyhow::Result<Keys> {
    let client = super::http()?;
    let pk_sql = format!(
        "SELECT k.table_schema, k.table_name, k.column_name \
         FROM \"{cat}\".information_schema.key_column_usage k \
         JOIN \"{cat}\".information_schema.table_constraints c \
           ON k.constraint_schema = c.constraint_schema \
          AND k.constraint_name   = c.constraint_name \
         WHERE c.constraint_type = 'PRIMARY KEY' \
           AND k.ordinal_position = 1 \
           AND (SELECT count(*) FROM \"{cat}\".information_schema.key_column_usage k2 \
                WHERE k2.constraint_schema = c.constraint_schema \
                  AND k2.constraint_name   = c.constraint_name) = 1",
        cat = catalog.replace('"', "\"\"")
    );
    let fk_sql = format!(
        "SELECT k.table_schema, k.table_name, k.column_name, \
                c.ref_table_schema, c.ref_table_name \
         FROM \"{cat}\".information_schema.key_column_usage k \
         JOIN \"{cat}\".information_schema.table_constraints tc \
           ON k.constraint_schema = tc.constraint_schema \
          AND k.constraint_name   = tc.constraint_name \
         JOIN \"{cat}\".information_schema.referential_constraints rc \
           ON k.constraint_schema = rc.constraint_schema \
          AND k.constraint_name   = rc.constraint_name \
         JOIN \"{cat}\".information_schema.table_constraints c \
           ON rc.unique_constraint_schema = c.constraint_schema \
          AND rc.unique_constraint_name   = c.constraint_name \
         WHERE tc.constraint_type = 'FOREIGN KEY' \
           AND k.ordinal_position = 1 \
           AND (SELECT count(*) FROM \"{cat}\".information_schema.key_column_usage k2 \
                WHERE k2.constraint_schema = tc.constraint_schema \
                  AND k2.constraint_name   = tc.constraint_name) = 1",
        cat = catalog.replace('"', "\"\"")
    );
    // 走两次 POST /v1/statement + 沿 nextUri 收完，路径上 `Page` 是同一个类型，
    // 故把循环抽进一个内嵌闭包——`headers` 在闭包里先 clone 再 move 进 async move，
    // 避免后续 `headers.clone()` 借已 move 的值
    async fn post_paged(
        client: &reqwest::Client,
        base: &str,
        headers: &HeaderMap,
        sql: &str,
    ) -> anyhow::Result<Vec<Vec<serde_json::Value>>> {
        let mut page: Page = client
            .post(format!("{base}/v1/statement"))
            .headers(headers.clone())
            .body(sql.to_string())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
        loop {
            if let Some(e) = page.error {
                let name = e.error_name.map(|n| format!("{n}: ")).unwrap_or_default();
                anyhow::bail!("{name}{}", e.message);
            }
            if let Some(d) = page.data.take() {
                rows.extend(d);
            }
            let Some(next) = page.next_uri.take() else {
                break;
            };
            page = client
                .get(&next)
                .headers(headers.clone())
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
        }
        Ok(rows)
    }
    let pk_rows = post_paged(&client, &conn.base, headers, &pk_sql).await?;
    let fk_rows = post_paged(&client, &conn.base, headers, &fk_sql).await?;
    let mut keys = Keys::default();
    for row in pk_rows {
        if let (Some(schema), Some(table), Some(column)) = (
            row.first().and_then(|v| v.as_str()),
            row.get(1).and_then(|v| v.as_str()),
            row.get(2).and_then(|v| v.as_str()),
        ) {
            keys.primary
                .insert((schema.to_string(), table.to_string(), column.to_string()));
        }
    }
    for row in fk_rows {
        if let (Some(schema), Some(table), Some(column), ref_schema, ref_table) = (
            row.first().and_then(|v| v.as_str()),
            row.get(1).and_then(|v| v.as_str()),
            row.get(2).and_then(|v| v.as_str()),
            row.get(3).and_then(|v| v.as_str()),
            row.get(4).and_then(|v| v.as_str()),
        ) {
            // 与 PG/MySQL 同样的处理：可能 ref_table_name 缺失（罕见），那就是「是
            // 外键但读不出目标表」——这一列不标 references_table，但只走这一次的话
            // 没法在 struct 里体现这个差别（references_table 是 Option<String>），
            // 这里选择不写入——保持与 PG/MySQL 一致
            if let (Some(rs), Some(rt)) = (ref_schema, ref_table) {
                keys.foreign
                    .entry((schema.to_string(), table.to_string(), column.to_string()))
                    .or_insert(format!("{rs}.{rt}"));
            }
        }
    }
    Ok(keys)
}

#[cfg(test)]
mod live_tests {
    use super::super::conn::TrinoConn;
    use super::super::QueryEngine;
    use super::TrinoEngine;

    /// 对着真 Trino 跑的那一档。没有 `UTOPIA_TEST_TRINO_URL` 就跳过——
    /// wiremock 回放证得了协议分页与列序，证不了「真集群的 information_schema
    /// 长这样、REST 一页页取回来的值解得对」。这两样只有真服务器有答案。
    ///
    /// 起一个来跑（内置 tpch 目录，数据现成、schema 固定）：
    /// `docker run -d -p 8080:8080 trinodb/trino`
    /// 然后 `UTOPIA_TEST_TRINO_URL=trino://probe@127.0.0.1:8080/tpch/tiny`。
    fn live_url() -> Option<String> {
        std::env::var("UTOPIA_TEST_TRINO_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
    }

    #[tokio::test]
    async fn a_live_cluster_reads_schema_and_answers() {
        let Some(url) = live_url() else {
            return;
        };
        let engine = TrinoEngine::new(TrinoConn::parse(&url).expect("parse url"));
        engine.test().await.expect("SELECT 1");

        // fetch_schema 打的是 "<catalog>".information_schema.columns——列名与写法
        // 只有真集群能证。schema 段限定了范围（tpch 有许多 sfN，只取 tiny）
        let schema = engine.fetch_schema().await.expect("schema");
        let name = schema
            .iter()
            .find(|c| c.table == "region" && c.column == "name")
            .expect("tpch.tiny.region.name in information_schema");
        assert_eq!(name.schema, "tiny", "schema 段应被限定");
        assert!(
            name.data_type.starts_with("varchar"),
            "data_type 要给出真形态，拿到的是 {}",
            name.data_type
        );

        // 取值往返：QUEUED → nextUri 一页页取，直到拿到 data。tpch.tiny.region
        // 是 TPC-H 标准表，五行、内容固定，正好当判据
        let r = engine
            .execute("SELECT regionkey, name FROM tpch.tiny.region ORDER BY regionkey")
            .await
            .expect("execute");
        assert_eq!(r.rows.len(), 5, "region 有五行");
        let first: serde_json::Value = serde_json::from_str(&r.rows[0]).unwrap();
        assert_eq!(first["regionkey"], serde_json::json!(0));
        assert_eq!(first["name"], serde_json::json!("AFRICA"));
        let last: serde_json::Value = serde_json::from_str(&r.rows[4]).unwrap();
        assert_eq!(last["name"], serde_json::json!("MIDDLE EAST"));

        // 写路径仍被闸挡住（第 1 层），与 mysql 那档对称
        assert!(super::super::guard_sql_for("trino", "DROP TABLE tpch.tiny.region").is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::super::conn::TrinoConn;
    use super::super::QueryEngine;
    use super::super::SchemaColumn;
    use super::TrinoEngine;
    use serde_json::json;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn follows_next_uri_and_keeps_column_order() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/statement"))
            .and(header("X-Trino-User", "alice"))
            .and(header("X-Trino-Catalog", "hive"))
            .and(body_string_contains("LIMIT 201"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "q1",
                "nextUri": format!("{}/v1/statement/q1/1", server.uri()),
                "stats": { "state": "QUEUED" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/statement/q1/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "q1",
                "columns": [ { "name": "region", "type": "varchar" }, { "name": "total", "type": "double" } ],
                "data": [ ["east", 12.5], ["west", 3] ],
                "stats": { "state": "FINISHED" }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let uri = server.uri();
        let conn = TrinoConn::parse(&format!(
            "trino://alice@{}/hive/default?ssl=false",
            uri.trim_start_matches("http://")
        ))
        .unwrap();
        let out = TrinoEngine::new(conn)
            .execute("SELECT region, total FROM orders")
            .await
            .unwrap();
        assert_eq!(
            out.rows,
            vec![
                r#"{"region":"east","total":12.5}"#,
                r#"{"region":"west","total":3}"#
            ]
        );
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn a_trino_error_page_becomes_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/statement"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "q2",
                "error": { "message": "line 1:8: Table 'hive.default.nope' does not exist", "errorName": "TABLE_NOT_FOUND" },
                "stats": { "state": "FAILED" }
            })))
            .mount(&server)
            .await;
        let conn = TrinoConn::parse(&format!(
            "trino://alice@{}/hive?ssl=false",
            server.uri().trim_start_matches("http://")
        ))
        .unwrap();
        let err = TrinoEngine::new(conn)
            .execute("SELECT * FROM nope")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("TABLE_NOT_FOUND"), "{err}");
    }

    /// 把 `fetch_schema` 的两次 SQL（列 + 键）按顺序回应。Mock 的 body match
    /// 用 `body_string_contains`：键查询 SQL 里有 `information_schema.table_constraints`，
    /// 列查询里有 `information_schema.columns`，单凭哪个含子串就能分清
    async fn mock_columns(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/v1/statement"))
            .and(body_string_contains("information_schema.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "qcols",
                "columns": [
                    { "name": "table_schema", "type": "varchar" },
                    { "name": "table_name",   "type": "varchar" },
                    { "name": "column_name", "type": "varchar" },
                    { "name": "data_type",   "type": "varchar" },
                    { "name": "comment",     "type": "varchar" }
                ],
                "data": [
                    ["tiny", "p", "id",   "bigint",   "PRIMARY KEY"],
                    ["tiny", "p", "fk",   "bigint",   null],
                    ["tiny", "p", "note", "varchar",  null],
                    ["tiny", "line", "oid",   "bigint", null],
                    ["tiny", "line", "note", "bigint", null]
                ],
                "stats": { "state": "FINISHED" }
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    /// 同列：PK 查询只关心 key_column_usage + table_constraints
    async fn mock_keys(
        server: &MockServer,
        pk_data: Vec<Vec<serde_json::Value>>,
        fk_data: Vec<Vec<serde_json::Value>>,
    ) {
        Mock::given(method("POST"))
            .and(path("/v1/statement"))
            .and(body_string_contains("table_constraints c"))
            .and(body_string_contains("'PRIMARY KEY'"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "qpk",
                "columns": [
                    { "name": "table_schema", "type": "varchar" },
                    { "name": "table_name",   "type": "varchar" },
                    { "name": "column_name", "type": "varchar" }
                ],
                "data": pk_data,
                "stats": { "state": "FINISHED" }
            })))
            .expect(1)
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/statement"))
            .and(body_string_contains("table_constraints tc"))
            .and(body_string_contains("'FOREIGN KEY'"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "qfk",
                "columns": [
                    { "name": "table_schema", "type": "varchar" },
                    { "name": "table_name",   "type": "varchar" },
                    { "name": "column_name", "type": "varchar" },
                    { "name": "ref_table_schema", "type": "varchar" },
                    { "name": "ref_table_name",   "type": "varchar" }
                ],
                "data": fk_data,
                "stats": { "state": "FINISHED" }
            })))
            .expect(1)
            .mount(server)
            .await;
    }

    fn col<'a>(
        cols: &'a [SchemaColumn],
        schema: &str,
        table: &str,
        column: &str,
    ) -> &'a SchemaColumn {
        cols.iter()
            .find(|c| c.schema == schema && c.table == table && c.column == column)
            .unwrap_or_else(|| panic!("{schema}.{table}.{column}"))
    }

    /// catalog 报 PK / FK 时——`tiny.p.id` 标 PK，`tiny.p.fk` 标 FK 指向 `tiny.p`。
    /// wiremock 把 PK 与 FK 两条 SQL 区分开（靠 body_string_contains 判查询类型）
    #[tokio::test]
    async fn keys_are_marked_when_the_catalog_reports_them_trino() {
        let server = MockServer::start().await;
        mock_columns(&server).await;
        mock_keys(
            &server,
            vec![vec![
                serde_json::Value::from("tiny"),
                serde_json::Value::from("p"),
                serde_json::Value::from("id"),
            ]],
            vec![vec![
                serde_json::Value::from("tiny"),
                serde_json::Value::from("p"),
                serde_json::Value::from("fk"),
                serde_json::Value::from("tiny"),
                serde_json::Value::from("p"),
            ]],
        )
        .await;
        let uri = server.uri();
        let conn = TrinoConn::parse(&format!(
            "trino://alice@{}/hive/tiny?ssl=false",
            uri.trim_start_matches("http://")
        ))
        .unwrap();
        let cols = TrinoEngine::new(conn).fetch_schema().await.expect("schema");
        let id = col(&cols, "tiny", "p", "id");
        assert!(id.is_primary_key);
        assert!(id.references_table.is_none());
        let fk = col(&cols, "tiny", "p", "fk");
        assert!(!fk.is_primary_key);
        assert_eq!(fk.references_table.as_deref(), Some("tiny.p"));
        let note = col(&cols, "tiny", "p", "note");
        assert!(!note.is_primary_key);
        assert!(note.references_table.is_none());
        let line_oid = col(&cols, "tiny", "line", "oid");
        assert!(!line_oid.is_primary_key, "非键列不标");
        assert!(line_oid.references_table.is_none());
    }

    /// 组合主键的成员不标。wiremock 端：「同约束下列数 = 1」的 guard 在 SQL 里，
    /// 不是 mock 的责任——所以 mock 直接给「不是单列 PK」的空集合，断言「整张
    /// 表上没有任何列被标 PK」。在真集群上 guard SQL 过滤后才回空集合，效果一致
    #[tokio::test]
    async fn a_composite_primary_key_stays_unmarked_trino() {
        let server = MockServer::start().await;
        mock_columns(&server).await;
        mock_keys(
            &server,
            // 空集合：单列 PK guard 在真集群上过滤掉了组合 PK
            vec![],
            vec![],
        )
        .await;
        let uri = server.uri();
        let conn = TrinoConn::parse(&format!(
            "trino://alice@{}/hive/tiny?ssl=false",
            uri.trim_start_matches("http://")
        ))
        .unwrap();
        let cols = TrinoEngine::new(conn).fetch_schema().await.expect("schema");
        // columns mock 里所有的列都不是 PK / FK——与「guard 过滤后留空集合」一致
        for c in &cols {
            assert!(
                !c.is_primary_key,
                "{}.{}.{} 不该被标成单列 PK",
                c.schema, c.table, c.column
            );
            assert!(
                c.references_table.is_none(),
                "{}.{}.{} 不该有 references_table",
                c.schema,
                c.table,
                c.column
            );
        }
    }

    /// keys() 失败不能 kill 整次 fetch_schema。Trino 上 PK / FK 大概率是空的
    /// （trino-python-client #205），但「真集群上 information_schema 视图被
    /// 安全策略禁了」也是合法场景。这一档测试 graceful degradation
    #[tokio::test]
    async fn a_keys_query_failure_does_not_fail_fetch_schema_trino() {
        let server = MockServer::start().await;
        mock_columns(&server).await;
        // PK 查询直接 500——模拟信息视图被禁
        Mock::given(method("POST"))
            .and(path("/v1/statement"))
            .and(body_string_contains("'PRIMARY KEY'"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let uri = server.uri();
        let conn = TrinoConn::parse(&format!(
            "trino://alice@{}/hive/tiny?ssl=false",
            uri.trim_start_matches("http://")
        ))
        .unwrap();
        // 仍应返回列——所有列 is_primary_key=false、references_table=None
        let cols = TrinoEngine::new(conn).fetch_schema().await.expect("schema");
        assert!(!cols.is_empty(), "keys() 失败时 fetch_schema 仍应返回列");
        for c in &cols {
            assert!(!c.is_primary_key, "键查询失败时不标 PK");
            assert!(c.references_table.is_none(), "键查询失败时不标 FK");
        }
    }
}
