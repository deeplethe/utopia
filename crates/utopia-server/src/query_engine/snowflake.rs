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

#[derive(Deserialize, Clone)]
struct Meta {
    #[serde(rename = "rowType")]
    row_type: Vec<RowType>,
    #[serde(rename = "numRows")]
    num_rows: Option<u64>,
    /// 多分块时给的 chunk URL 列表；每条带 rowCount 和 url。按 docs 的语义 GET
    /// 每个 url 拼回完整结果集。schema 读到这里的多分块表示 information_schema.columns
    /// 一次返回装不下，需要跟 #703 Databricks 的解法同样的「跟 link 拉齐」路径
    #[serde(rename = "partitionInfo")]
    partition_info: Option<Vec<PartitionInfo>>,
}

#[derive(Deserialize, Clone)]
struct RowType {
    name: String,
    #[serde(rename = "type")]
    ty: String,
}

/// Snowflake 的 partition 信息：每个 chunk 一个 URL，按 url GET 拿剩下的 data。
/// `row_count` 是这个 chunk 自己的行数，`url` 是相对路径
#[derive(Deserialize, Clone)]
struct PartitionInfo {
    #[serde(rename = "rowCount")]
    row_count: u64,
    url: Option<String>,
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
        // 把 meta 拷出一份再消费；下面既要从 partition_info 拉分块，
        // 又要从 row_type 还原类型，两个都要用
        let meta = resp.meta.clone();
        let types: Vec<RowType> = meta
            .as_ref()
            .map(|m| m.row_type.clone())
            .unwrap_or_default();
        let partitions: Vec<PartitionInfo> = meta
            .as_ref()
            .and_then(|m| m.partition_info.clone())
            .unwrap_or_default();
        let expected_total_rows: Option<u64> = meta.as_ref().and_then(|m| m.num_rows);
        let mut rows: Vec<Vec<serde_json::Value>> = resp
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
        // Snowflake 的 schema 读需要拉齐所有 partition：一次返回装不下时按 partitionInfo
        // 里每条 URL GET 拿剩下的 data。partitionInfo 的第一条对应的就是这次响应本身
        // （按 docs「first partition is returned inline」），所以从第二条开始拉
        // （#703：之前 partitionInfo 直接被丢，宽 catalog 上 schema 文档被截断）
        let mut seen_partition_rows: u64 = rows.len() as u64;
        if partitions.len() > 1 {
            for chunk in partitions.iter().skip(1) {
                let Some(url) = chunk.url.as_deref() else {
                    // docs 说每个 partition 必须有 url；没有就当服务端偷偷少给了
                    anyhow::bail!("Snowflake partition without a url; rows may be incomplete");
                };
                let chunk_resp: StatementResponse = self
                    .request(client.get(url))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                let chunk_rows = chunk_resp.data.unwrap_or_default();
                seen_partition_rows = seen_partition_rows.saturating_add(chunk_rows.len() as u64);
                rows.extend(chunk_rows.into_iter().map(|row| {
                    row.iter()
                        .enumerate()
                        .map(|(i, v)| coerce(types.get(i).map(|t| t.ty.as_str()).unwrap_or(""), v))
                        .collect()
                }));
            }
        }
        // numRows 是整个结果集的总数（不是当前 chunk 的）；分区 rowCount 之和也应该是它
        // ——任一对不上都说明服务端偷偷改 protocol 或中途丢包，残缺数据不要灌进结构文档
        if let Some(expected) = expected_total_rows {
            if seen_partition_rows != expected {
                anyhow::bail!(
                    "Snowflake schema read returned {seen_partition_rows} rows across partitions but numRows reports {expected}"
                );
            }
        }
        let partition_total: u64 = partitions.iter().map(|p| p.row_count).sum();
        if !partitions.is_empty() && partition_total != seen_partition_rows {
            anyhow::bail!(
                "Snowflake partition row counts sum to {partition_total} but received {seen_partition_rows} rows"
            );
        }
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

    /// schema 读按 partitionInfo 走多分块：第一条 partition 对应这次响应本身，
    /// 从第二条开始按 url GET 拿剩下的 data。docs 说 num_rows 是整个结果集的总数，
    /// 不是当前 chunk 的——可以拿来对账（这里不强校验，但证明分块走通）
    /// （#703：之前 partitionInfo 直接被丢，宽 catalog 上 schema 文档被截断）
    #[tokio::test]
    async fn a_schema_read_follows_partitions_until_exhausted() {
        let server = MockServer::start().await;
        let chunk2_url = format!("{}/api/v2/statements/h3/chunk/2", server.uri());
        let chunk3_url = format!("{}/api/v2/statements/h3/chunk/3", server.uri());
        Mock::given(method("POST"))
            .and(path("/api/v2/statements"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "resultSetMetaData": {
                    "numRows": 3,
                    "rowType": [
                        { "name": "TABLE_SCHEMA", "type": "text" },
                        { "name": "TABLE_NAME", "type": "text" },
                        { "name": "COLUMN_NAME", "type": "text" },
                        { "name": "DATA_TYPE", "type": "text" },
                        { "name": "COMMENT", "type": "text" }
                    ],
                    "partitionInfo": [
                        { "rowCount": 1, "url": format!("{}/api/v2/statements/h3/chunk/1", server.uri()) },
                        { "rowCount": 1, "url": chunk2_url },
                        { "rowCount": 1, "url": chunk3_url }
                    ]
                },
                "data": [
                    ["default", "wide_orders", "id", "BIGINT", "主键"]
                ],
                "code": "090001",
                "statementHandle": "h3",
                "message": "Statement executed successfully."
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/statements/h3/chunk/2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [ ["default", "wide_orders", "region", "STRING", null] ],
                "code": "090001"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v2/statements/h3/chunk/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [ ["default", "wide_orders", "amount", "DECIMAL(12,2)", null] ],
                "code": "090001"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cols = SnowflakeEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap();
        assert_eq!(
            cols.len(),
            3,
            "want 3 columns across partitions, got {cols:?}"
        );
        assert_eq!(cols[0].column, "id");
        assert_eq!(cols[1].column, "region");
        assert_eq!(cols[2].column, "amount");
    }
}
