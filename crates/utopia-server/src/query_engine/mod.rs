//! 问数查询引擎：trait 接缝（BlobStore 同手法）+ 引擎无关的安全闸。
//!
//! 引擎按协议族扩，不按产品名扩：postgres 线协议（`postgres.rs`）、MySQL 线协议
//! （`mysql.rs`，一条协议顺带覆盖 TiDB / OceanBase / Doris / StarRocks / MariaDB）
//! → HTTP 族——
//! `trino.rs` 一个顶起 Iceberg / Delta / Hive 整个湖仓生态，`databricks.rs`、
//! `snowflake.rs` 各走自家的 SQL REST API。挂载模型与注册表引擎无关，加引擎只放宽
//! 一条 CHECK。连接串是唯一的输入：引擎由 scheme 决定（[`engine_from_conn`]），
//! 剩下的部分各引擎自己拆（`conn.rs`），凭据只在服务端流转。
//!
//! 安全闸（纵深防御，不信任模型）：
//! 1. sqlparser 解析：仅放行单条 SELECT/WITH（含 CTE），拒绝 DML/DDL/多语句/SELECT INTO。
//!    按引擎选方言；sqlparser 没有 Trino 方言，Generic 是它的超集
//! 2. 强制外包一层 LIMIT（cap+1 探测截断）
//! 3. 会话级只读 + 语句超时（引擎各自的机制，parser 万一漏网也写不进去）。
//!    HTTP 族没有会话，只有语句超时——只读靠第 1 层，这是它们比线协议少的那一层
//! 4. 结果统一为 JSON Lines：PG 让库自己转；HTTP 族拿到列名与值后在这里拼，列序保留

mod conn;
mod databricks;
mod mysql;
mod postgres;
mod snowflake;
mod trino;

use sqlparser::ast::Statement;
use sqlparser::dialect::{
    DatabricksDialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SnowflakeDialect,
};
use sqlparser::parser::Parser;
use std::time::Duration;

/// 行数上限（外包 LIMIT cap+1，第 201 行只用来判断截断）。
pub const ROW_CAP: usize = 200;
pub(crate) const STATEMENT_TIMEOUT_SECS: u32 = 10;
/// HTTP 族：单次请求的超时，与整条语句从提交到拿完结果的轮询预算
pub(crate) const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const HTTP_POLL_BUDGET: Duration = Duration::from_secs(30);

/// 注册表里 `engine` 列的取值。迁移里的 CHECK 与这张表要一致
pub const ENGINES: &[&str] = &["postgres", "mysql", "trino", "databricks", "snowflake"];

#[derive(Debug)]
pub struct QueryResult {
    /// 每行一个 JSON 对象文本（键序 = 查询列序）
    pub rows: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug)]
pub struct SchemaColumn {
    pub schema: String,
    pub table: String,
    pub column: String,
    pub data_type: String,
    pub comment: Option<String>,
}

#[async_trait::async_trait]
pub trait QueryEngine: Send + Sync {
    async fn test(&self) -> anyhow::Result<()>;
    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>>;
    /// 执行已过闸的 SELECT。实现自身仍需强制只读会话与超时（纵深防御）。
    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult>;
}

/// scheme → 引擎名。界面只有一个连接串输入框，这里是它唯一的分派点。
pub fn engine_from_conn(conn: &str) -> Option<&'static str> {
    let scheme = conn.trim().split("://").next()?.to_ascii_lowercase();
    match scheme.as_str() {
        "postgres" | "postgresql" => Some("postgres"),
        // 一条线协议顺带覆盖 TiDB / OceanBase / Doris / StarRocks——它们都说
        // MySQL 协议，连接串照写 mysql:// 即可
        "mysql" | "mariadb" => Some("mysql"),
        "trino" | "presto" => Some("trino"),
        "databricks" => Some("databricks"),
        "snowflake" => Some("snowflake"),
        _ => None,
    }
}

/// 引擎工厂。conn 凭据只在服务端流转。
pub fn engine_for(engine: &str, conn: &str) -> anyhow::Result<Box<dyn QueryEngine>> {
    match engine {
        "postgres" => Ok(Box::new(postgres::PostgresEngine::new(conn))),
        "mysql" => Ok(Box::new(mysql::MysqlEngine::new(conn))),
        "trino" => Ok(Box::new(trino::TrinoEngine::new(conn::TrinoConn::parse(
            conn,
        )?))),
        "databricks" => Ok(Box::new(databricks::DatabricksEngine::new(
            conn::DatabricksConn::parse(conn)?,
        ))),
        "snowflake" => Ok(Box::new(snowflake::SnowflakeEngine::new(
            conn::SnowflakeConn::parse(conn)?,
        ))),
        other => anyhow::bail!("Unsupported engine: {other}"),
    }
}

/// 安全闸第 1 层：按引擎方言解析并校验，返回规整后的语句文本。
pub fn guard_sql_for(engine: &str, sql: &str) -> anyhow::Result<String> {
    let cleaned = sql.trim().trim_end_matches(';').trim();
    if cleaned.is_empty() {
        anyhow::bail!("Empty SQL");
    }
    let parsed = match engine {
        "databricks" => Parser::parse_sql(&DatabricksDialect {}, cleaned),
        "snowflake" => Parser::parse_sql(&SnowflakeDialect {}, cleaned),
        "trino" => Parser::parse_sql(&GenericDialect {}, cleaned),
        "mysql" => Parser::parse_sql(&MySqlDialect {}, cleaned),
        _ => Parser::parse_sql(&PostgreSqlDialect {}, cleaned),
    };
    let statements = parsed.map_err(|e| anyhow::anyhow!("SQL parse error: {e}"))?;
    if statements.len() != 1 {
        anyhow::bail!("Exactly one statement is allowed");
    }
    match &statements[0] {
        Statement::Query(_) => Ok(cleaned.to_string()),
        other => anyhow::bail!(
            "Read-only: only SELECT/WITH queries are allowed (got {})",
            statement_kind(other)
        ),
    }
}

fn statement_kind(s: &Statement) -> &'static str {
    match s {
        Statement::Insert { .. } => "INSERT",
        Statement::Update { .. } => "UPDATE",
        Statement::Delete { .. } => "DELETE",
        Statement::CreateTable { .. } => "CREATE TABLE",
        Statement::Drop { .. } => "DROP",
        Statement::AlterTable { .. } => "ALTER TABLE",
        Statement::Truncate { .. } => "TRUNCATE",
        _ => "a non-SELECT statement",
    }
}

/// 第 2 层：外包一层 LIMIT。三个 HTTP 引擎都认这个写法；PG 有自己的 row_to_json 版本
pub(crate) fn wrap_limit(sql: &str) -> String {
    format!("SELECT * FROM ( {sql} ) AS _q LIMIT {}", ROW_CAP + 1)
}

/// 第 201 行只用来判断截断，不交给模型
pub(crate) fn truncate_rows<T>(mut rows: Vec<T>) -> (Vec<T>, bool) {
    let truncated = rows.len() > ROW_CAP;
    rows.truncate(ROW_CAP);
    (rows, truncated)
}

/// HTTP 族共用：「列名 + 行值」拼成 JSON Lines。手拼而不是 `serde_json::Map`，
/// 后者不开 `preserve_order` 就按键排序，而列序是查询写下的顺序，模型读表靠它
pub(crate) fn rows_to_json_lines(
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
) -> Vec<String> {
    rows.iter()
        .map(|row| {
            let mut line = String::from("{");
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    line.push(',');
                }
                line.push_str(&serde_json::to_string(col).unwrap_or_else(|_| "\"?\"".into()));
                line.push(':');
                let value = row.get(i).cloned().unwrap_or(serde_json::Value::Null);
                line.push_str(&value.to_string());
            }
            line.push('}');
            line
        })
        .collect()
}

/// Databricks 的 JSON_ARRAY 与 Snowflake 的 data 把每个值都给成字符串（或 null）。
/// 按列类型把数与布尔还原，其余留字符串——模型对 `"42"` 和 `42` 的算术不一样
pub(crate) fn coerce(type_name: &str, raw: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::String(s) = raw else {
        return raw.clone();
    };
    let ty = type_name.to_ascii_uppercase();
    const NUMERIC: &[&str] = &[
        "INT", "LONG", "SHORT", "BYTE", "FLOAT", "DOUBLE", "DECIMAL", "NUMBER", "FIXED", "REAL",
        "NUMERIC",
    ];
    // INTERVAL 也含 "INT"：解析不成数就原样留下，不会误伤
    if NUMERIC.iter().any(|k| ty.contains(k)) {
        if let Ok(n) = s.parse::<i64>() {
            return n.into();
        }
        if let Ok(f) = s.parse::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(f) {
                return serde_json::Value::Number(n);
            }
        }
    }
    if ty.starts_with("BOOL") {
        match s.as_str() {
            "true" | "TRUE" => return true.into(),
            "false" | "FALSE" => return false.into(),
            _ => {}
        }
    }
    raw.clone()
}

/// 单引号字面量的转义：schema 名进 information_schema 的 WHERE 子句
pub(crate) fn sql_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// HTTP 族共用的客户端。
///
/// **代理策略是显式的**：回环地址与 `NO_PROXY` 里的主机直连，其余按 `HTTPS_PROXY` /
/// `HTTP_PROXY` / `ALL_PROXY` 走。不用 reqwest 的系统代理探测——Windows 上它读注册表，
/// 而注册表里 `127.*` 这种绕过写法它认不全，本机的替身服务会被送进代理拿回 502。
/// 服务进程该看环境变量，这条规矩与 docker-compose 里的写法一致
pub(crate) fn http() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .user_agent("utopia")
        .proxy(reqwest::Proxy::custom(|url: &reqwest::Url| proxy_for(url)))
        .build()?)
}

fn proxy_for(url: &reqwest::Url) -> Option<reqwest::Url> {
    let host = url.host_str()?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(|c| c == '[' || c == ']')
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if loopback || no_proxy_matches(host) {
        return None;
    }
    let keys: &[&str] = if url.scheme() == "https" {
        &["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
    } else {
        &["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
    };
    keys.iter()
        .find_map(|k| std::env::var(k).ok())
        .filter(|v| !v.trim().is_empty())
        .and_then(|v| reqwest::Url::parse(v.trim()).ok())
}

/// `NO_PROXY=localhost,127.0.0.1,.internal,corp.example` 的常见写法：整名相等，
/// 或者以点开头的后缀匹配
fn no_proxy_matches(host: &str) -> bool {
    let raw = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default();
    raw.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "*")
        .any(|p| {
            let p = p.trim_start_matches('.');
            host.eq_ignore_ascii_case(p)
                || host
                    .to_ascii_lowercase()
                    .ends_with(&format!(".{}", p.to_ascii_lowercase()))
        })
        || raw.split(',').any(|p| p.trim() == "*")
}

#[cfg(test)]
mod tests {
    use super::{coerce, engine_from_conn, guard_sql_for, rows_to_json_lines};
    use serde_json::json;

    fn guard_sql(sql: &str) -> anyhow::Result<String> {
        guard_sql_for("postgres", sql)
    }

    #[test]
    fn allows_select_and_cte() {
        assert!(guard_sql("SELECT region, sum(amount) FROM orders GROUP BY 1").is_ok());
        assert!(guard_sql("WITH t AS (SELECT 1 AS x) SELECT * FROM t;").is_ok());
    }

    #[test]
    fn rejects_writes_and_ddl() {
        for bad in [
            "UPDATE orders SET amount = 0",
            "DELETE FROM orders",
            "INSERT INTO orders (region) VALUES ('east')",
            "DROP TABLE orders",
            "TRUNCATE orders",
            "CREATE TABLE t (id int)",
            "ALTER TABLE orders ADD COLUMN x int",
        ] {
            assert!(guard_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn rejects_multi_statement() {
        assert!(guard_sql("SELECT 1; DROP TABLE orders").is_err());
        assert!(guard_sql("").is_err());
    }

    #[test]
    fn every_dialect_keeps_the_same_gate() {
        for engine in ["postgres", "mysql", "trino", "databricks", "snowflake"] {
            assert!(
                guard_sql_for(engine, "SELECT a FROM t WHERE b > 1").is_ok(),
                "{engine}"
            );
            assert!(guard_sql_for(engine, "DELETE FROM t").is_err(), "{engine}");
            assert!(
                guard_sql_for(engine, "SELECT 1; SELECT 2").is_err(),
                "{engine}"
            );
        }
        // 各家的方言细节：反引号、双冒号转型，都要过得去
        assert!(guard_sql_for("databricks", "SELECT `region` FROM main.sales.orders").is_ok());
        assert!(guard_sql_for("snowflake", "SELECT amount::number FROM db.public.orders").is_ok());
        assert!(guard_sql_for("trino", "SELECT count(*) FROM hive.default.orders").is_ok());
        // MySQL 的反引号与 PG 方言不兼容，走自己的方言才过得去
        assert!(guard_sql_for("mysql", "SELECT `region` FROM `sales`.`orders`").is_ok());
    }

    #[test]
    fn engine_follows_the_scheme() {
        assert_eq!(engine_from_conn("postgres://u:p@h/db"), Some("postgres"));
        assert_eq!(engine_from_conn("postgresql://u:p@h/db"), Some("postgres"));
        assert_eq!(engine_from_conn("trino://u@h:8443/hive"), Some("trino"));
        assert_eq!(engine_from_conn("presto://u@h/hive"), Some("trino"));
        assert_eq!(
            engine_from_conn("databricks://:t@h/sql/1.0/warehouses/x"),
            Some("databricks")
        );
        assert_eq!(
            engine_from_conn("snowflake://:t@a.snowflakecomputing.com/db"),
            Some("snowflake")
        );
        assert_eq!(engine_from_conn("mysql://u:p@h:3306/db"), Some("mysql"));
        // 同一套协议的另一个写法，引擎里会被改写成 mysql:// 再交给驱动
        assert_eq!(engine_from_conn("mariadb://u@h/db"), Some("mysql"));
        assert_eq!(engine_from_conn("garbage"), None);
    }

    #[test]
    fn json_lines_keep_column_order() {
        let cols = vec!["zeta".to_string(), "alpha".to_string()];
        let rows = vec![vec![json!(1), json!("x")], vec![json!(null)]];
        assert_eq!(
            rows_to_json_lines(&cols, &rows),
            vec![r#"{"zeta":1,"alpha":"x"}"#, r#"{"zeta":null,"alpha":null}"#]
        );
    }

    #[test]
    fn strings_come_back_as_numbers_when_the_column_says_so() {
        assert_eq!(coerce("DOUBLE", &json!("12.5")), json!(12.5));
        assert_eq!(coerce("fixed", &json!("42")), json!(42));
        assert_eq!(coerce("BOOLEAN", &json!("true")), json!(true));
        assert_eq!(coerce("STRING", &json!("42")), json!("42"));
        assert_eq!(coerce("INTERVAL", &json!("1 day")), json!("1 day"));
        assert_eq!(coerce("DOUBLE", &json!(null)), json!(null));
    }
}
