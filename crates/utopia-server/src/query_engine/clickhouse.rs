//! ClickHouse — HTTP 协议（不是线协议）。
//!
//! 一条 SQL 一次请求：`POST /`，body 是 SQL 文本，末尾拼 `FORMAT JSONEachRow` 拿
//! newline-delimited JSON。出错一律非 2xx + 错误文本（不像 Trino 那样有 first-class
//! error 字段）。schema 走 `system.columns`，没有 `information_schema`。
//!
//! 没有会话可设只读：超时靠 `max_execution_time` 设置（per-query），只读靠
//! `guard_sql_for` 这一层（与 Trino 同款处境）。
//!
//! 协议细节：见 `https://clickhouse.com/docs/en/interfaces/http`。

use super::conn::ClickhouseConn;
use super::{
    sql_literal, truncate_rows, QueryEngine, QueryResult, SchemaColumn, STATEMENT_TIMEOUT_SECS,
};
use base64::Engine as _;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

pub struct ClickhouseEngine {
    conn: ClickhouseConn,
}

impl ClickhouseEngine {
    pub fn new(conn: ClickhouseConn) -> Self {
        Self { conn }
    }

    fn headers(&self) -> anyhow::Result<HeaderMap> {
        let mut h = HeaderMap::new();
        // 显式 X-ClickHouse-User 比塞进 Authorization 更稳：CH 也会认出 Basic 头，
        // 但 User 头连 token-only 的形式（X-ClickHouse-Key）也跟得上
        h.insert("X-ClickHouse-User", HeaderValue::from_str(&self.conn.user)?);
        if let Some(p) = &self.conn.password {
            let raw = format!("{}:{p}", self.conn.user);
            let token = base64::engine::general_purpose::STANDARD.encode(raw);
            h.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Basic {token}"))?,
            );
        }
        if let Some(db) = &self.conn.database {
            h.insert("X-ClickHouse-Database", HeaderValue::from_str(db)?);
        }
        Ok(h)
    }

    /// 把 SQL 包成一个 per-call 超时并以 JSONEachRow 返回的请求，返回 (列序, 行数据)。
    /// ClickHouse HTTP 不分页：单次 body 就是全部行，所以也走 `truncate_rows` 截到 200。
    async fn run(&self, sql: &str) -> anyhow::Result<(Vec<String>, Vec<Vec<serde_json::Value>>)> {
        let client = super::http()?;
        let headers = self.headers()?;
        // SETTINGS 走 query string：max_execution_time 是秒，足够挡掉失控查询；
        // 不要把这条尾缀交给模型，guard_sql_for 已经把它拦在只读了
        let url = format!(
            "{}/?max_execution_time={}",
            self.conn.base, STATEMENT_TIMEOUT_SECS
        );
        let body = format!("{sql} FORMAT JSONEachRow");
        let resp = client
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            // CH 把错误写在 body 里：要么是 `Code: 60. DB::Exception: ...`，要么
            // 是 JSON `{"exception": "..."}`，不分类，整段交给上层
            anyhow::bail!("ClickHouse {}: {}", status, text.trim());
        }
        // 空结果（DDL 之类不该到这，但 FORMAT 输出空字符串）也按 0 行处理
        Ok(parse_json_each_row(&text))
    }
}

/// `JSONEachRow`：一行一个 JSON 对象。空行允许，常见于 `SELECT ... LIMIT 0`、
/// system.columns 查询里一个表名都没匹配上的情况。`FeedEvent` 风格的 trailing
/// 换行也要剥一下。
///
/// 返回 `(列序, 行列表)`——列序来自首批非空行的键，按 SELECT 顺序（`serde_json`
/// 开了 `preserve_order`，LinkedHashMap 的迭代与 JSON 写入一致）。调用方只取列序
/// 一次，行列表全收
fn parse_json_each_row(body: &str) -> (Vec<String>, Vec<Vec<serde_json::Value>>) {
    let mut columns: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();
    for raw in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let value: serde_json::Value = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let serde_json::Value::Object(map) = value else {
            // 非对象（标量 SELECT / 单列值）一行的兜底：把这一项塞进一行单值向量里
            rows.push(vec![value]);
            continue;
        };
        let mut cols = Vec::with_capacity(map.len());
        let mut vals = Vec::with_capacity(map.len());
        for (k, v) in map {
            cols.push(k);
            vals.push(v);
        }
        if columns.is_none() {
            columns = Some(cols);
        }
        rows.push(vals);
    }
    (columns.unwrap_or_default(), rows)
}

/// system.columns 一行（数组下标固定）→ SchemaColumn。
///
/// system.columns 自身的列：`database, table, name, type, comment, is_in_primary_key`
/// —— 按 SELECT 的顺序取下标。`is_in_primary_key` 只对 MergeTree 系为 1，其它引擎恒 0
/// （与 Trino / Snowflake 的「读不出就 false」是同一句话——ch 只是把这件事写在系统表里）
///
/// 数组下标 6 是 `system.tables.engine_full` —— cut 3 起把表引擎带上，用来：
///   - 把非 MergeTree 系（Log / Kafka / Merge / Distributed …）明确写进 comment，
///     让探索提示词看到「这是 Wide / 非键」的语义，而不是把 0 误读成「该列非主键」。
///   - KEY 不在那条路上时把 is_primary_key 拨回 false，让 schema 层的语义统一，
///     不与 `system.columns.is_in_primary_key` 的「applicable」混。
fn schema_row(row: Vec<serde_json::Value>) -> SchemaColumn {
    let text = |i: usize| -> String {
        row.get(i)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let engine_full = text(6);
    let is_mergetree_family = is_mergetree_family(&engine_full);
    let is_pk = is_mergetree_family
        && row
            .get(5)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            != 0;
    let comment = match (text(4), is_mergetree_family) {
        (c, true) if !c.is_empty() => Some(c),
        (c, false) => {
            // 非 MergeTree 系：把原 comment 与引擎家族拼一起，让人一眼看到「为什么不是 PK」
            let suffix = format!("not a MergeTree-family engine ({engine_full})");
            Some(if c.is_empty() { suffix } else { format!("{c}; {suffix}") })
        }
        _ => None,
    };
    SchemaColumn {
        schema: text(0),
        table: text(1),
        column: text(2),
        data_type: text(3),
        comment,
        is_primary_key: is_pk,
        references_table: None,
    }
}

/// ClickHouse 把 MergeTree 大家族按「主键列是否生效」分成两类：
///   - `MergeTree` / `ReplacingMergeTree` / `SummingMergeTree` / `AggregatingMergeTree`
///     / `CollapsingMergeTree` / `VersionedCollapsingMergeTree` / `GraphiteMergeTree`
///     / `Replicated*` 各家 —— 都是主键生效的可索引表。
///   - 其它（Log / TinyLog / StripeLog / Memory / Kafka / NATS / RabbitMQ / EmbeddedRocksDB
///     / Buffer / Distributed / Materialized* / View …） —— 没有 first-class 主键，
///     `system.columns.is_in_primary_key` 一律 0。
///
/// 后缀「MergeTree」是大族的固定命名（`…_mergetree_*` 序列化族都共享），所以子串
/// 匹配比完整枚举更经得起版本演进：22.x 加 `…MergeTree` 的新型号不需要改判定逻辑。
fn is_mergetree_family(engine_full: &str) -> bool {
    engine_full.contains("MergeTree")
}

#[async_trait::async_trait]
impl QueryEngine for ClickhouseEngine {
    async fn test(&self) -> anyhow::Result<()> {
        // ClickHouse 的 `SELECT 1` 在 JSONEachRow 下给一行 `{"1": 1}`——只要
        // 能正确解析一行就算通
        let (_, rows) = self.run("SELECT 1").await?;
        if rows.is_empty() {
            anyhow::bail!("ClickHouse /: empty response for SELECT 1");
        }
        Ok(())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        let database = self.conn.database.as_deref().ok_or_else(|| {
            anyhow::anyhow!(
                "clickhouse://: put the database in the connection string (clickhouse://user@host:8123/DB) so the schema can be read"
            )
        })?;
        // 限定到挂上来的库；系统库会塞 EXPLAIN / metadata 的伪列，模型读表反而被绕。
        // cut 3 起把 `system.tables.engine_full` JOIN 进来 —— `system.columns`
        // 自身的 `is_in_primary_key` 只在 MergeTree 系有意义，没有表引擎的话
        // 我们没法判断「这个 0 是 NOT PK 还是 NOT APPLICABLE」，JOIN 把这件事解了
        let sql = format!(
            "SELECT c.database, c.table, c.name, c.type, c.comment, c.is_in_primary_key, t.engine_full \
             FROM system.columns c \
             INNER JOIN system.tables t \
                 ON c.database = t.database AND c.table = t.name \
             WHERE c.database = {db} \
             ORDER BY c.database, c.table, c.name",
            db = sql_literal(database),
        );
        let (_, rows) = self.run(&sql).await?;
        Ok(rows.into_iter().map(schema_row).collect())
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult> {
        // 与其他 HTTP 引擎同一条规矩：外包 LIMIT cap+1，第 201 行只用来判断截断
        let (columns, all_rows) = self.run(&wrap_limit_for_ch(sql)).await?;
        let (rows, truncated) = truncate_rows(all_rows);
        Ok(QueryResult {
            rows: super::rows_to_json_lines(&columns, &rows),
            truncated,
        })
    }
}

/// ClickHouse 不支持 `(SELECT ...) AS _q LIMIT n` 这层 PG 风格的子查询包 LIMIT，
/// 也不支持 `SELECT * FROM ( ... ) LIMIT n` 里那个子查询当 derived table——CH
/// 把它当 `SELECT * FROM (SELECT ...)`，把内层 SELECT 完整吐一遍就够了。直接
/// 在末尾拼 `LIMIT n` 就够
fn wrap_limit_for_ch(sql: &str) -> String {
    format!("{sql} LIMIT {}", super::ROW_CAP + 1)
}

/// ClickHouse 的 JSONEachRow 把列名写在每个对象里——解析时由 `parse_json_each_row`
/// 一次性带回，整张表的列序与 SELECT 列表的顺序一致（`serde_json` 开了
/// `preserve_order`，LinkedHashMap 的迭代与 JSON 写入一致）

#[cfg(test)]
mod tests {
    use super::super::conn::ClickhouseConn;
    use super::super::{engine_for, engine_from_conn, QueryEngine};
    use super::ClickhouseEngine;
    use serde_json::json;
    use wiremock::matchers::{body_string_contains, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn conn(server: &MockServer) -> ClickhouseConn {
        // 别再拼 `:8123` 上去，URL parser 会把它当路径的一段；wiremock 已经替我们
        // 选了端口，连接串里只放库名
        ClickhouseConn::parse(&format!(
            "clickhouse://alice@{}/default?ssl=false",
            server.uri().trim_start_matches("http://")
        ))
        .unwrap()
    }

    /// cut 1 留下来的低级断言还在：scheme 进注册表，工厂不 panic
    #[test]
    fn clickhouse_scheme_lands_in_the_registry() {
        assert_eq!(
            engine_from_conn("clickhouse://alice@host:8123/db"),
            Some("clickhouse")
        );
        assert_eq!(
            engine_from_conn("clickhouse://alice:s3cr3t@clickhouse.cloud:9440/default?ssl=true"),
            Some("clickhouse")
        );
        assert_eq!(engine_from_conn("clickhouse-without-scheme"), None);
    }

    #[tokio::test]
    async fn factory_builds_a_clickhouse_engine() {
        let engine = engine_for("clickhouse", "clickhouse://alice@host:8123/db")
            .expect("factory builds clickhouse");
        // 真驱动，body 没接 wiremock 时会真去连——保证它不让 panic 漏到外层
        let err = engine.execute("SELECT 1").await.unwrap_err().to_string();
        // 拒绝原因可能是 connect refused 也可能是 DNS，可能是别的写法形式，
        // 但绝对不会出现 clickhouse: not yet implemented
        assert!(
            !err.contains("not yet implemented"),
            "cut 1 已撤：execute 现在是真驱动，得到的错是：{err}"
        );
    }

    #[tokio::test]
    async fn select_one_uses_basic_auth_and_returns_a_row() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(query_param("max_execution_time", "10"))
            .and(header("X-ClickHouse-User", "alice"))
            .and(header(
                "authorization",
                "Basic YWxpY2U6c2VjcmV0", // base64("alice:secret")
            ))
            .and(header("X-ClickHouse-Database", "default"))
            .and(body_string_contains("SELECT 1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"1\":1}\n"))
            .expect(1)
            .mount(&server)
            .await;

        let mut conn = conn(&server);
        conn.password = Some("secret".into());
        let out = ClickhouseEngine::new(conn).execute("SELECT 1").await.unwrap();
        assert_eq!(out.rows, vec![r#"{"1":1}"#]);
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn multi_row_select_lands_in_column_order_and_truncates_at_201() {
        // 201 行：第 201 行只用来判断截断，不交给模型
        let body = (0..super::super::ROW_CAP + 1)
            .map(|i| format!(r#"{{"n":{i}}}"#))
            .collect::<Vec<_>>()
            .join("\n");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("SELECT n FROM t"))
            .and(body_string_contains("LIMIT 201"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let mut conn = conn(&server);
        conn.database = None;
        let out = ClickhouseEngine::new(conn)
            .execute("SELECT n FROM t")
            .await
            .unwrap();
        assert_eq!(out.rows.len(), super::super::ROW_CAP);
        assert!(out.truncated);
    }

    #[tokio::test]
    async fn system_columns_row_becomes_a_schema_column() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("FROM system.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"database":"default","table":"events","name":"id","type":"UInt64","comment":"","is_in_primary_key":1,"engine_full":"MergeTree"}
{"database":"default","table":"events","name":"ts","type":"DateTime","comment":"when it happened","is_in_primary_key":0,"engine_full":"MergeTree"}
{"database":"default","table":"events","name":"payload","type":"String","comment":"","is_in_primary_key":0,"engine_full":"MergeTree"}
"#,
            ))
            .mount(&server)
            .await;
        let schema = ClickhouseEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap();
        assert_eq!(schema.len(), 3);
        let id = &schema[0];
        assert_eq!(id.schema, "default");
        assert_eq!(id.table, "events");
        assert_eq!(id.column, "id");
        assert_eq!(id.data_type, "UInt64");
        assert!(id.is_primary_key, "MergeTree 主键应被读出");
        assert_eq!(id.references_table, None);

        let ts = &schema[1];
        assert!(!ts.is_primary_key);
        assert_eq!(ts.comment.as_deref(), Some("when it happened"));

        let payload = &schema[2];
        // 空 comment → None，不是 Some("")
        assert_eq!(payload.comment, None);
    }

    /// cut 3：非 MergeTree 系（Kafka / Log …）的表，`is_in_primary_key` 在
    /// `system.columns` 里恒 0、但 0 并不等于"非主键"——要把表引擎家族写进 comment
    /// 让探索提示词看到这是 NOT APPLICABLE，避免误读
    #[tokio::test]
    async fn non_mergetree_engine_marks_applicability_in_comment() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("FROM system.columns"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"database":"default","table":"raw","name":"msg","type":"String","comment":"","is_in_primary_key":0,"engine_full":"Kafka"}
{"database":"default","table":"norm","name":"id","type":"UInt64","comment":"row id","is_in_primary_key":1,"engine_full":"MergeTree"}
"#,
            ))
            .mount(&server)
            .await;
        let schema = ClickhouseEngine::new(conn(&server))
            .fetch_schema()
            .await
            .unwrap();
        assert_eq!(schema.len(), 2);

        let raw = &schema[0];
        assert!(!raw.is_primary_key, "Kafka 系 0 不算 PK，但要看语义是否带出来");
        // 空 comment + 非 MergeTree → 注释自动注入引擎家族
        assert_eq!(
            raw.comment.as_deref(),
            Some("not a MergeTree-family engine (Kafka)"),
            "Kafka 列不应只挂着 Some(\"\")，要让提示词看到 NOT-APPLICABLE 的语义"
        );

        let norm = &schema[1];
        assert!(norm.is_primary_key, "MergeTree 系的 1 仍是 PK");
        assert_eq!(
            norm.comment.as_deref(),
            Some("row id"),
            "MergeTree 系保留原 comment，不被引擎家族覆盖"
        );
    }

    #[tokio::test]
    async fn a_clickhouse_error_body_becomes_a_rust_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(404).set_body_string(
                "Code: 60. DB::Exception: Table 'default.nope' does not exist. (UNKNOWN_TABLE)",
            ))
            .mount(&server)
            .await;
        let err = ClickhouseEngine::new(conn(&server))
            .execute("SELECT * FROM nope")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("404"), "{err}");
        assert!(err.contains("UNKNOWN_TABLE"), "{err}");

        // 400 一类（业务语法错）也是这条路径
        let server2 = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(400).set_body_string(
                "Code: 62. DB::Exception: Syntax error: failed at position 1: SELECT * FORM t",
            ))
            .mount(&server2)
            .await;
        let err = ClickhouseEngine::new(conn(&server2))
            .execute("SELECT * FORM t")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Syntax error"), "{err}");
    }

    #[test]
    fn parse_json_each_row_handles_empty_input_and_padding() {
        // 空响应：SELECT count() 没匹配、空表上都返回空字符串
        assert_eq!(super::parse_json_each_row(""), (Vec::<String>::new(), Vec::<Vec<_>>::new()));
        assert_eq!(
            super::parse_json_each_row("\n  \n"),
            (Vec::<String>::new(), Vec::<Vec<_>>::new()),
        );
        // 单行：列名要被吸收进 JSON 对象里
        let (cols, rows) = super::parse_json_each_row(r#"{"a":1,"b":"x"}"#);
        // preserve_order 下 LinkedHashMap 保留写入顺序
        assert_eq!(cols, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(rows, vec![vec![json!(1), json!("x")]]);
        // 多行：JSONEachRow 一行一条记录，但列名以首批非空行为准
        let (cols, rows) = super::parse_json_each_row(
            "{\"k\":1}\n{\"k\":2}\n{\"k\":3}\n",
        );
        assert_eq!(cols, vec!["k".to_string()]);
        assert_eq!(rows.len(), 3);
    }
}
