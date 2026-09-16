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
    /// INLINE disposition with a `row_limit` set：true 表示结果按行截断。
    /// schema 读不传 row_limit，理论上不会触发；留着做显式 fail-fast 而不是静默返回
    /// 不全的数据
    truncated: Option<bool>,
    /// INLINE 第一个分块塞不下时给的下一页 GET 链接；按 docs 的语义反复 GET 直到没有
    next_chunk_internal_link: Option<String>,
    /// 整个结果集的总行数（chunked INLINE 时也有）。schema 读完后用来校验
    /// `rows.len() == total_row_count`
    total_row_count: Option<u64>,
}

#[derive(Deserialize, Clone)]
struct Schema {
    columns: Vec<ColumnInfo>,
}

#[derive(Deserialize, Clone)]
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

    /// 提交一条语句，轮询到 SUCCEEDED，把结果拼成 (列名, 行值)。所有列值都按
    /// manifest 的 `type_text` 还原成数或布尔。
    ///
    /// `row_limit`：传 `None` 表示不限行（schema 读用）。INLINE disposition 在
    /// 服务端有 25 MiB 的 body 上限，所以单分块塞不下时通过 `next_chunk_internal_link`
    /// 继续 GET，循环到 manifest 没有给出下一页为止。schema 读完后用
    /// `manifest.total_row_count` 做一次 `rows.len()` 校验——少了就当结果不全
    /// （docs 说 truncated=true 时另有标记，schema 读不该见到）
    ///
    /// 之前对所有语句都传 `row_limit: ROW_CAP + 1`（#703）：chat 查询需要这个
    /// 安全阀，schema 读不需要；information_schema.columns 每行是一列，宽 catalog
    /// 上的 schema 文档会被悄悄截到 201 行
    async fn run(
        &self,
        sql: &str,
        row_limit: Option<usize>,
    ) -> anyhow::Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        let client = super::http()?;
        let mut body = json!({
            "warehouse_id": self.conn.warehouse_id,
            "statement": sql,
            "wait_timeout": "30s",
            "on_wait_timeout": "CONTINUE",
            "disposition": "INLINE",
            "format": "JSON_ARRAY",
        });
        if let Some(n) = row_limit {
            body["row_limit"] = json!(n);
        }
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
            .as_ref()
            .and_then(|m| m.schema.as_ref())
            .map(|s| s.columns.clone())
            .unwrap_or_default();
        let mut rows: Vec<Vec<serde_json::Value>> = resp
            .result
            .as_ref()
            .and_then(|r| r.data_array.clone())
            .unwrap_or_default()
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
        // INLINE 的分块跟在第一个 chunk 之后：next_chunk_internal_link 是相对路径，
        // docs 说按 link 反复 GET，直到 manifest 不再给出下一页为止
        let manifest = resp.manifest.as_ref();
        let truncated = manifest.and_then(|m| m.truncated).unwrap_or(false);
        if truncated {
            anyhow::bail!(
                "Databricks returned a truncated result; row_limit must be raised or the query rewritten"
            );
        }
        let mut next_link = manifest.and_then(|m| m.next_chunk_internal_link.clone());
        while let Some(link) = next_link.take() {
            let chunk: StatementResponse = client
                .get(link)
                .bearer_auth(&self.conn.token)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            // 中间 chunk 的 manifest 可能再次给 truncated/link；状态字段对每块都要读
            let m = chunk.manifest.as_ref();
            if m.and_then(|m| m.truncated).unwrap_or(false) {
                anyhow::bail!(
                    "Databricks chunk was truncated by the service; rows may be incomplete"
                );
            }
            let chunk_rows = chunk.result.and_then(|r| r.data_array).unwrap_or_default();
            rows.extend(chunk_rows.into_iter().map(|row| {
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
            }));
            next_link = m.and_then(|m| m.next_chunk_internal_link.clone());
        }
        // schema 读按 docs 说不该见到 truncated=true；total_row_count 是每次都给的，
        // 校验要放在所有 chunk 都收齐之后：第一次响应给的 total 是整个结果集的总数，
        // 不是第一个 chunk 的总数
        if let (Some(manifest), None) = (manifest, row_limit) {
            let expected = manifest.total_row_count;
            if let Some(expected) = expected {
                let got: u64 = rows.len() as u64;
                if got != expected {
                    anyhow::bail!(
                        "Databricks schema read returned {got} rows but manifest reports {expected}"
                    );
                }
            }
        }
        Ok((columns.into_iter().map(|c| c.name).collect(), rows))
    }
}

/// 引擎说的是「这个 catalog / 表不存在（或看不见）」。认的是 Databricks 的错误类名，
/// 它们是协议的一部分（`[TABLE_OR_VIEW_NOT_FOUND] …`），不是措辞
fn is_not_found(e: &anyhow::Error) -> bool {
    const CLASSES: [&str; 4] = [
        "TABLE_OR_VIEW_NOT_FOUND",
        "NO_SUCH_CATALOG_EXCEPTION",
        "CATALOG_NOT_FOUND",
        "SCHEMA_NOT_FOUND",
    ];
    let text = e.to_string();
    CLASSES.iter().any(|c| text.contains(c))
}

#[async_trait::async_trait]
impl QueryEngine for DatabricksEngine {
    async fn test(&self) -> anyhow::Result<()> {
        self.run("SELECT 1", Some(ROW_CAP + 1)).await.map(|_| ())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        // 两条候选，先准后全。真实集群上 `main`.information_schema.columns 回过
        // TABLE_OR_VIEW_NOT_FOUND（#241），所以第一条**说找不到**时退到 system 那份。
        //
        // 只有「找不到」才退：令牌不对、仓库停着、超时，换一条语句也还是这样，再跑一遍
        // 只是让点按钮的人多等半分钟。退过去读到 0 行也算失败——两份 information_schema
        // 都只列令牌看得见的对象，system 那份还不含 hive_metastore，看不见的 catalog
        // 在那里是「成功地没有行」；当成功的话，结构文档会被一份空的覆盖掉。
        // 失败时每条试过的写法连同引擎的原话都带上
        //
        // row_limit 不传（#703）：information_schema.columns 每行是一列，宽 catalog
        // 上的 schema 文档会被悄悄截到 201 行；现在 `run` 自己跟 `next_chunk_internal_link`
        // 一直拉到 `manifest.total_row_count` 行，多分块也收齐
        let queries = self.schema_queries();
        let mut tried: Vec<String> = Vec::new();
        for (i, (label, sql)) in queries.iter().enumerate() {
            match self.run(sql, None).await {
                Ok((_, rows)) if rows.is_empty() && i > 0 => {
                    tried.push(format!(
                        "{label}: no columns for this catalog — the token cannot see it, \
                         or it is not managed by Unity Catalog"
                    ));
                    break;
                }
                Ok((_, rows)) => {
                    return Ok(rows.into_iter().map(super::trino::schema_row).collect())
                }
                Err(e) => {
                    let missing = is_not_found(&e);
                    tried.push(format!("{label}: {e}"));
                    if !missing {
                        break;
                    }
                }
            }
        }
        anyhow::bail!("no readable information_schema ({})", tried.join("; "))
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult> {
        let (columns, rows) = self.run(&wrap_limit(sql), Some(ROW_CAP + 1)).await?;
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

    fn failed(code: &str, message: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "status": { "state": "FAILED", "error": { "error_code": code, "message": message } }
        }))
    }

    /// 两条都不通时，错误带上每一条试过的写法和它自己的原话。
    #[tokio::test]
    async fn two_dead_ends_say_which_ones_were_tried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("`main`.information_schema.columns"))
            .respond_with(failed(
                "BAD_REQUEST",
                "[TABLE_OR_VIEW_NOT_FOUND] first words",
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("system.information_schema.columns"))
            .respond_with(failed("PERMISSION_DENIED", "second words"))
            .mount(&server)
            .await;

        let err = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("catalog information_schema"), "{err}");
        assert!(err.contains("first words"), "{err}");
        assert!(err.contains("system information_schema"), "{err}");
        assert!(err.contains("second words"), "{err}");
    }

    /// 令牌不对、仓库停着这类错误不是「找不到」：换一条语句也一样，只发一次
    #[tokio::test]
    async fn an_error_that_is_not_a_missing_table_is_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .respond_with(failed("PERMISSION_DENIED", "token lacks USE CATALOG"))
            .expect(1)
            .mount(&server)
            .await;

        let err = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("token lacks USE CATALOG"), "{err}");
        assert!(!err.contains("system information_schema"), "{err}");
    }

    /// 退到 system 那份读到 0 行：看不见这个 catalog，或它不归 Unity Catalog 管。
    /// 这是失败，不是一份空的结构——否则刷新会把原来的结构文档覆盖成空的
    #[tokio::test]
    async fn a_fallback_that_reads_no_columns_is_a_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("`main`.information_schema.columns"))
            .respond_with(failed(
                "BAD_REQUEST",
                "[TABLE_OR_VIEW_NOT_FOUND] cannot be found",
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("system.information_schema.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s5",
                "status": { "state": "SUCCEEDED" },
                "manifest": { "schema": { "columns": [
                    { "name": "table_schema", "type_text": "STRING", "position": 0 }
                ] } },
                "result": { "data_array": [] }
            })))
            .mount(&server)
            .await;

        let err = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no columns for this catalog"), "{err}");
        assert!(err.contains("cannot be found"), "{err}");
    }

    /// 宽 catalog 上的 schema 读按 docs 走 INLINE 多分块：第一个 chunk 给
    /// `next_chunk_internal_link`，按它 GET 到第二个 chunk，没有下一页就停。
    /// 之前对所有语句都传 row_limit=201（#703），宽 catalog 上 information_schema.columns
    /// 一行一列，每张表第一个 200 列之后的列就被丢掉了；现在 `fetch_schema` 不传
    /// row_limit，分块之间按 link 拉齐
    #[tokio::test]
    async fn a_schema_read_follows_chunks_until_exhausted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("`main`.information_schema.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s_chunk1",
                "status": { "state": "SUCCEEDED" },
                "manifest": {
                    "schema": { "columns": [
                        { "name": "table_schema", "type_text": "STRING", "position": 0 },
                        { "name": "table_name", "type_text": "STRING", "position": 1 },
                        { "name": "column_name", "type_text": "STRING", "position": 2 },
                        { "name": "data_type", "type_text": "STRING", "position": 3 },
                        { "name": "comment", "type_text": "STRING", "position": 4 }
                    ] },
                    "total_row_count": 3,
                    "next_chunk_internal_link": format!(
                        "{}/api/2.0/sql/statements/s_chunk1/chunk/2",
                        server.uri()
                    )
                },
                "result": { "data_array": [
                    ["default", "wide_orders", "id", "BIGINT", "主键"],
                    ["default", "wide_orders", "region", "STRING", null]
                ] }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/2.0/sql/statements/s_chunk1/chunk/2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": { "state": "SUCCEEDED" },
                "manifest": {
                    "total_row_count": 3,
                    "next_chunk_internal_link": format!(
                        "{}/api/2.0/sql/statements/s_chunk1/chunk/3",
                        server.uri()
                    )
                },
                "result": { "data_array": [
                    ["default", "wide_orders", "amount", "DECIMAL(12,2)", null]
                ] }
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/2.0/sql/statements/s_chunk1/chunk/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "status": { "state": "SUCCEEDED" },
                "manifest": { "total_row_count": 3 }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cols = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap();
        assert_eq!(cols.len(), 3, "want 3 columns across chunks, got {cols:?}");
        assert_eq!(cols[0].column, "id");
        assert_eq!(cols[1].column, "region");
        assert_eq!(cols[2].column, "amount");
    }

    /// 跑 chat 查询时（带 row_limit），服务端真的把行砍了：manifest.truncated=true
    /// 应当直接失败，而不是带着 < ROW_CAP 行假装「查询成功」
    #[tokio::test]
    async fn a_chat_query_bails_loudly_when_the_service_says_truncated() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s_trunc",
                "status": { "state": "SUCCEEDED" },
                "manifest": {
                    "schema": { "columns": [
                        { "name": "n", "type_text": "BIGINT", "position": 0 }
                    ] },
                    "total_row_count": 1000,
                    "truncated": true
                },
                "result": { "data_array": [ ["1"], ["2"] ] }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let err = DatabricksEngine::new(conn(&server))
            .execute("SELECT n FROM big")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("truncated"), "{err}");
        assert!(err.contains("row_limit"), "{err}");
    }

    /// schema 读 manifest 报的 total_row_count 跟实际收到的行数对不上——服务端
    /// 偷偷改 protocol，或中途丢包：fail-fast，免得把残缺的 schema 灌进结构文档
    #[tokio::test]
    async fn a_schema_read_fails_when_total_row_count_does_not_match() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/2.0/sql/statements"))
            .and(body_string_contains("`main`.information_schema.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "statement_id": "s_count_mismatch",
                "status": { "state": "SUCCEEDED" },
                "manifest": {
                    "schema": { "columns": [
                        { "name": "table_schema", "type_text": "STRING", "position": 0 },
                        { "name": "table_name", "type_text": "STRING", "position": 1 },
                        { "name": "column_name", "type_text": "STRING", "position": 2 },
                        { "name": "data_type", "type_text": "STRING", "position": 3 },
                        { "name": "comment", "type_text": "STRING", "position": 4 }
                    ] },
                    "total_row_count": 99
                },
                "result": { "data_array": [
                    ["default", "orders", "id", "BIGINT", null]
                ] }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let err = DatabricksEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("schema read returned 1 rows"), "{err}");
        assert!(err.contains("99"), "{err}");
    }
}
