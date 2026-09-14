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

    /// information_schema.columns 的候选写法 `(说明, SQL)`，说明只用来报错。
    /// 没有 catalog 时只剩一条：会话默认那份。
    fn schema_queries(&self) -> Vec<(&'static str, String)> {
        let schema_filter = self
            .conn
            .schema
            .as_deref()
            .map(|s| format!(" AND table_schema = {}", sql_literal(s)))
            .unwrap_or_default();
        let select = "SELECT table_schema, table_name, column_name, data_type, comment";
        let tail = format!(
            "table_schema <> 'information_schema'{schema_filter} \
             ORDER BY table_schema, table_name, ordinal_position"
        );
        match self.conn.catalog.as_deref() {
            Some(catalog) => vec![
                (
                    "catalog information_schema",
                    format!(
                        "{select} FROM `{}`.information_schema.columns WHERE {tail}",
                        catalog.replace('`', "``")
                    ),
                ),
                (
                    "system information_schema",
                    format!(
                        "{select} FROM system.information_schema.columns \
                         WHERE table_catalog = {} AND {tail}",
                        sql_literal(catalog)
                    ),
                ),
            ],
            None => vec![(
                "session information_schema",
                format!("{select} FROM information_schema.columns WHERE {tail}"),
            )],
        }
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
        // 两条候选，先准后全；哪条存在由集群决定。真实集群上
        // `main`.information_schema.columns 回过 TABLE_OR_VIEW_NOT_FOUND（#241），
        // 所以第一条不通就退到 system 那份。两条都失败才失败，错误带上试过的写法。
        let queries = self.schema_queries();
        let mut last: Option<anyhow::Error> = None;
        for (_, sql) in &queries {
            match self.run(sql).await {
                Ok((_, rows)) => {
                    return Ok(rows.into_iter().map(super::trino::schema_row).collect())
                }
                Err(e) => last = Some(e),
            }
        }
        let tried = queries
            .iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "no readable information_schema (tried: {tried}): {}",
            last.map(|e| e.to_string()).unwrap_or_default()
        )
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
    use wiremock::matchers::{body_string_contains, header, method, path};
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

    /// catalog 级那份读不到（真实集群回过 TABLE_OR_VIEW_NOT_FOUND）时退到 system 那份。
    #[tokio::test]
    async fn the_schema_read_falls_back_to_the_system_information_schema() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("`main`.information_schema.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s3",
                "status": { "state": "FAILED", "error": {
                    "error_code": "BAD_REQUEST",
                    "message": "[TABLE_OR_VIEW_NOT_FOUND] The table or view `main`.`information_schema`.`columns` cannot be found."
                } }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("system.information_schema.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s4",
                "status": { "state": "SUCCEEDED" },
                "manifest": { "schema": { "columns": [
                    { "name": "table_schema", "type_text": "STRING", "position": 0 },
                    { "name": "table_name", "type_text": "STRING", "position": 1 },
                    { "name": "column_name", "type_text": "STRING", "position": 2 },
                    { "name": "data_type", "type_text": "STRING", "position": 3 },
                    { "name": "comment", "type_text": "STRING", "position": 4 }
                ] } },
                "result": { "data_array": [
                    ["default", "orders", "amount", "DECIMAL(12,2)", "订单金额"]
                ] }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cols = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap();
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].schema, "default");
        assert_eq!(cols[0].table, "orders");
        assert_eq!(cols[0].column, "amount");
        assert_eq!(cols[0].data_type, "DECIMAL(12,2)");
        assert_eq!(cols[0].comment.as_deref(), Some("订单金额"));
    }

    /// 两条都不通时，错误要带上试过的写法。
    #[tokio::test]
    async fn two_dead_ends_say_which_ones_were_tried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": { "state": "FAILED", "error": {
                    "error_code": "BAD_REQUEST",
                    "message": "[TABLE_OR_VIEW_NOT_FOUND] nope"
                } }
            })))
            .mount(&server)
            .await;

        let err = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("catalog information_schema"), "{err}");
        assert!(err.contains("system information_schema"), "{err}");
        assert!(err.contains("TABLE_OR_VIEW_NOT_FOUND"), "{err}");
    }
}
