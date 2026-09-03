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
        Ok(rows.into_iter().map(schema_row).collect())
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

/// information_schema 的一行 → SchemaColumn（值可能是 null，comment 常是）
pub(crate) fn schema_row(row: Vec<serde_json::Value>) -> SchemaColumn {
    let text = |i: usize| -> String {
        row.get(i)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    SchemaColumn {
        schema: text(0),
        table: text(1),
        column: text(2),
        data_type: text(3),
        comment: row
            .get(4)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use super::super::conn::TrinoConn;
    use super::super::QueryEngine;
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
}
