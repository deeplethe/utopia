//! Snowflake SQL API v2（`/api/v2/statements`）。同步提交（`async=false`）拿不完的
//! 语句回 202，沿 statementHandle 轮询。值全是字符串，按 rowType 还原数与布尔。
//!
//! 只收令牌，不收密码：programmatic access token 或 OAuth。密钥对 JWT 要本地签名，
//! 这一版不做——见 `conn.rs`。

use super::conn::SnowflakeConn;
use super::{
    coerce, rows_to_json_lines, sql_literal, truncate_rows, wrap_limit, QueryEngine, QueryResult,
    SchemaColumn, HTTP_POLL_BUDGET, STATEMENT_TIMEOUT_SECS,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};

pub struct SnowflakeEngine {
    conn: SnowflakeConn,
}

#[derive(Deserialize)]
struct StatementResponse {
    #[serde(rename = "resultSetMetaData")]
    meta: Option<Meta>,
    data: Option<Vec<Vec<serde_json::Value>>>,
    message: Option<String>,
    code: Option<String>,
    #[serde(rename = "statementHandle")]
    handle: Option<String>,
}

#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "rowType")]
    row_type: Vec<RowType>,
}

#[derive(Deserialize)]
struct RowType {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

impl SnowflakeEngine {
    pub fn new(conn: SnowflakeConn) -> Self {
        Self { conn }
    }

    fn request(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        r.bearer_auth(&self.conn.token)
            .header("X-Snowflake-Authorization-Token-Type", self.conn.token_type)
            .header("Accept", "application/json")
    }

    async fn run(&self, sql: &str) -> anyhow::Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        let client = super::http()?;
        let mut body = json!({
            "statement": sql,
            "timeout": STATEMENT_TIMEOUT_SECS,
            "parameters": { "MULTI_STATEMENT_COUNT": "1" },
        });
        for (key, value) in [
            ("database", &self.conn.database),
            ("schema", &self.conn.schema),
            ("warehouse", &self.conn.warehouse),
            ("role", &self.conn.role),
        ] {
            if let Some(v) = value {
                body[key] = json!(v);
            }
        }
        let mut http = self
            .request(client.post(format!("{}/api/v2/statements?async=false", self.conn.base)))
            .json(&body)
            .send()
            .await?;
        let started = Instant::now();
        // 202 = 还在跑；其余非 2xx 的 body 里带 message
        while http.status() == StatusCode::ACCEPTED {
            let partial: StatementResponse = http.json().await?;
            let handle = partial.handle.ok_or_else(|| {
                anyhow::anyhow!("Snowflake returned 202 without a statementHandle")
            })?;
            if started.elapsed() > HTTP_POLL_BUDGET {
                anyhow::bail!(
                    "Snowflake statement did not finish within {}s",
                    HTTP_POLL_BUDGET.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            http = self
                .request(client.get(format!("{}/api/v2/statements/{handle}", self.conn.base)))
                .send()
                .await?;
        }
        if !http.status().is_success() {
            let status = http.status();
            let text = http.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<StatementResponse>(&text)
                .ok()
                .and_then(|r| r.message)
                .unwrap_or(text);
            anyhow::bail!("Snowflake {status}: {msg}");
        }
        let resp: StatementResponse = http.json().await?;
        if let (Some(code), Some(message)) = (&resp.code, &resp.message) {
            // 2xx 里也可能带业务错误码；090001 是 "statement executed successfully"
            if code != "090001" && resp.meta.is_none() {
                anyhow::bail!("Snowflake {code}: {message}");
            }
        }
        let types: Vec<RowType> = resp.meta.map(|m| m.row_type).unwrap_or_default();
        let rows = resp
            .data
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, v)| coerce(types.get(i).map(|t| t.ty.as_str()).unwrap_or(""), v))
                    .collect()
            })
            .collect();
        Ok((types.into_iter().map(|t| t.name).collect(), rows))
    }
}

#[async_trait::async_trait]
impl QueryEngine for SnowflakeEngine {
    async fn test(&self) -> anyhow::Result<()> {
        self.run("SELECT 1").await.map(|_| ())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        let database = self.conn.database.as_deref().ok_or_else(|| {
            anyhow::anyhow!("snowflake://: put the database in the connection string (snowflake://:TOKEN@account/DATABASE) so the schema can be read")
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
             WHERE table_schema <> 'INFORMATION_SCHEMA'{schema_filter} \
             ORDER BY table_schema, table_name, ordinal_position",
            database.replace('"', "\"\"")
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
    use super::super::conn::SnowflakeConn;
    use super::super::QueryEngine;
    use super::SnowflakeEngine;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn conn(server: &MockServer) -> SnowflakeConn {
        SnowflakeConn::parse(&format!(
            "snowflake://:pat-test@{}/ANALYTICS/PUBLIC?warehouse=WH&ssl=false",
            server.uri().trim_start_matches("http://")
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn a_synchronous_answer_is_typed_by_row_type() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/statements"))
            .and(header("authorization", "Bearer pat-test"))
            .and(header(
                "X-Snowflake-Authorization-Token-Type",
                "PROGRAMMATIC_ACCESS_TOKEN",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resultSetMetaData": { "numRows": 1, "rowType": [
                    { "name": "REGION", "type": "text" },
                    { "name": "TOTAL", "type": "fixed", "scale": 2 }
                ] },
                "data": [ ["east", "42.10"] ],
                "code": "090001",
                "statementHandle": "h1",
                "message": "Statement executed successfully."
            })))
            .expect(1)
            .mount(&server)
            .await;
        let out = SnowflakeEngine::new(conn(&server))
            .execute("SELECT region, total FROM orders")
            .await
            .unwrap();
        assert_eq!(out.rows, vec![r#"{"REGION":"east","TOTAL":42.1}"#]);
    }

    #[tokio::test]
    async fn a_202_is_polled_until_the_answer_arrives() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/statements"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({
                "code": "333334", "statementHandle": "h2", "message": "Asynchronous execution in progress."
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/statements/h2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resultSetMetaData": { "rowType": [ { "name": "N", "type": "fixed" } ] },
                "data": [ ["1"] ], "code": "090001", "statementHandle": "h2"
            })))
            .expect(1)
            .mount(&server)
            .await;
        let out = SnowflakeEngine::new(conn(&server))
            .execute("SELECT 1 AS n")
            .await
            .unwrap();
        assert_eq!(out.rows, vec![r#"{"N":1}"#]);
    }

    #[tokio::test]
    async fn an_error_body_is_surfaced() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v2/statements"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "code": "002003", "message": "SQL compilation error: Object 'NOPE' does not exist"
            })))
            .mount(&server)
            .await;
        let err = SnowflakeEngine::new(conn(&server))
            .execute("SELECT * FROM nope")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist"), "{err}");
    }
}
