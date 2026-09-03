//! Databricks SQL Statement Execution API（`/api/2.0/sql/statements`）。
//! 一个 SQL warehouse 后面是 Unity Catalog 的整个湖仓（Delta 为主），
//! 令牌是 personal access token。结果要 INLINE + JSON_ARRAY：值全是字符串，
//! 按 manifest 里的列类型还原成数与布尔。

use super::conn::DatabricksConn;
use super::{
    coerce, rows_to_json_lines, sql_literal, truncate_rows, wrap_limit, QueryEngine, QueryResult,
    SchemaColumn, HTTP_POLL_BUDGET, ROW_CAP,
};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

pub struct DatabricksEngine {
    conn: DatabricksConn,
}

#[derive(Deserialize)]
struct StatementResponse {
    statement_id: Option<String>,
    status: Status,
    manifest: Option<Manifest>,
    result: Option<ResultData>,
}

#[derive(Deserialize)]
struct Status {
    state: String,
    error: Option<StatusError>,
}

#[derive(Deserialize)]
struct StatusError {
    message: Option<String>,
    error_code: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    schema: Option<Schema>,
}

#[derive(Deserialize)]
struct Schema {
    columns: Vec<ColumnInfo>,
}

#[derive(Deserialize)]
struct ColumnInfo {
    name: String,
    type_text: Option<String>,
}

#[derive(Deserialize)]
struct ResultData {
    data_array: Option<Vec<Vec<serde_json::Value>>>,
}

impl DatabricksEngine {
    pub fn new(conn: DatabricksConn) -> Self {
        Self { conn }
    }

    async fn run(&self, sql: &str) -> anyhow::Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        let client = super::http()?;
        let mut body = json!({
            "warehouse_id": self.conn.warehouse_id,
            "statement": sql,
            "wait_timeout": "30s",
            "on_wait_timeout": "CONTINUE",
            "disposition": "INLINE",
            "format": "JSON_ARRAY",
            "row_limit": ROW_CAP + 1,
        });
        if let Some(c) = &self.conn.catalog {
            body["catalog"] = json!(c);
        }
        if let Some(s) = &self.conn.schema {
            body["schema"] = json!(s);
        }
        let mut resp: StatementResponse = client
            .post(format!("{}/api/2.0/sql/statements", self.conn.base))
            .bearer_auth(&self.conn.token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let started = Instant::now();
        loop {
            match resp.status.state.as_str() {
                "SUCCEEDED" => break,
                "PENDING" | "RUNNING" => {
                    let id = resp
                        .statement_id
                        .clone()
                        .ok_or_else(|| anyhow::anyhow!("Databricks returned no statement_id"))?;
                    if started.elapsed() > HTTP_POLL_BUDGET {
                        anyhow::bail!(
                            "Databricks statement did not finish within {}s",
                            HTTP_POLL_BUDGET.as_secs()
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    resp = client
                        .get(format!("{}/api/2.0/sql/statements/{id}", self.conn.base))
                        .bearer_auth(&self.conn.token)
                        .send()
                        .await?
                        .error_for_status()?
                        .json()
                        .await?;
                }
                other => {
                    let e = resp.status.error.as_ref();
                    let code = e
                        .and_then(|e| e.error_code.clone())
                        .map(|c| format!("{c}: "))
                        .unwrap_or_default();
                    let msg = e
                        .and_then(|e| e.message.clone())
                        .unwrap_or_else(|| format!("statement ended in state {other}"));
                    anyhow::bail!("{code}{msg}");
                }
            }
        }
        let columns: Vec<ColumnInfo> = resp
            .manifest
            .and_then(|m| m.schema)
            .map(|s| s.columns)
            .unwrap_or_default();
        let raw_rows = resp.result.and_then(|r| r.data_array).unwrap_or_default();
        let rows = raw_rows
            .into_iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let ty = columns
                            .get(i)
                            .and_then(|c| c.type_text.as_deref())
                            .unwrap_or("");
                        coerce(ty, v)
                    })
                    .collect()
            })
            .collect();
        Ok((columns.into_iter().map(|c| c.name).collect(), rows))
    }
}

#[async_trait::async_trait]
impl QueryEngine for DatabricksEngine {
    async fn test(&self) -> anyhow::Result<()> {
        self.run("SELECT 1").await.map(|_| ())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        // 带 catalog 就查那个 catalog 的 information_schema；不带就是会话默认的
        let prefix = self
            .conn
            .catalog
            .as_deref()
            .map(|c| format!("`{}`.", c.replace('`', "``")))
            .unwrap_or_default();
        let schema_filter = self
            .conn
            .schema
            .as_deref()
            .map(|s| format!(" AND table_schema = {}", sql_literal(s)))
            .unwrap_or_default();
        let sql = format!(
            "SELECT table_schema, table_name, column_name, data_type, comment \
             FROM {prefix}information_schema.columns \
             WHERE table_schema <> 'information_schema'{schema_filter} \
             ORDER BY table_schema, table_name, ordinal_position"
        );
        let (_, rows) = self.run(&sql).await?;
        Ok(rows.into_iter().map(super::trino::schema_row).collect())
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

#[cfg(test)]
mod tests {
    use super::super::conn::DatabricksConn;
    use super::super::QueryEngine;
    use super::DatabricksEngine;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn conn(server: &MockServer) -> DatabricksConn {
        DatabricksConn::parse(&format!(
            "databricks://:dapi-test@{}/sql/1.0/warehouses/wh1?catalog=main&ssl=false",
            server.uri().trim_start_matches("http://")
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn polls_until_succeeded_and_restores_types() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(header("authorization", "Bearer dapi-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s1",
                "status": { "state": "PENDING" }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/2.0/sql/statements/s1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s1",
                "status": { "state": "SUCCEEDED" },
                "manifest": { "schema": { "columns": [
                    { "name": "region", "type_text": "STRING", "position": 0 },
                    { "name": "total", "type_text": "DECIMAL(12,2)", "position": 1 },
                    { "name": "active", "type_text": "BOOLEAN", "position": 2 }
                ] } },
                "result": { "data_array": [ ["east", "12.50", "true"], ["west", null, "false"] ] }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let out = DatabricksEngine::new(conn(&server))
            .execute("SELECT region, total, active FROM orders")
            .await
            .unwrap();
        assert_eq!(
            out.rows,
            vec![
                r#"{"region":"east","total":12.5,"active":true}"#,
                r#"{"region":"west","total":null,"active":false}"#
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_statement_reports_the_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s2",
                "status": { "state": "FAILED", "error": { "error_code": "BAD_REQUEST", "message": "TABLE_OR_VIEW_NOT_FOUND: nope" } }
            })))
            .mount(&server)
            .await;
        let err = DatabricksEngine::new(conn(&server))
            .execute("SELECT * FROM nope")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("TABLE_OR_VIEW_NOT_FOUND"), "{err}");
    }
}
