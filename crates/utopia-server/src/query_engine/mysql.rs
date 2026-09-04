//! MySQL 线协议族。**一条协议顺带覆盖一片**：TiDB、OceanBase、Doris、StarRocks
//! 都说这个协议，MariaDB 也是，所以这个文件的性价比在四个引擎里最高。
//!
//! 与 `postgres.rs` 的两处不同，都来自 MySQL 自己：
//!
//! - **没有 `row_to_json`。** PG 那边把整行交给库转成 JSON 文本，列序天然保留；
//!   这里只能逐列取值自己拼（同 HTTP 族的 `rows_to_json_lines`），于是多出一张
//!   类型映射表。那张表的判据是驱动的 `ColumnType::name` 与各类型的
//!   `compatible`，不是 MySQL 手册：两处分歧会静默地把整列变成 null——
//!   无符号整数的类型名带 ` UNSIGNED` 后缀，而 `DECIMAL` 被同时挡在 `f64`
//!   与 `String` 之外，只有 `BigDecimal` 读得出来（见 `Cell`）
//! - **超时的写法有两种。** MySQL 是 `max_execution_time`（毫秒），MariaDB 是
//!   `max_statement_time`（秒）。两个都试，都不认才报错：这一层挡的是全表扫描
//!   拖垮库，外包的 LIMIT 挡不住它（先扫完再截断），所以不能静默降级

use super::{
    coerce, rows_to_json_lines, truncate_rows, wrap_limit, QueryEngine, QueryResult, SchemaColumn,
    STATEMENT_TIMEOUT_SECS,
};
use sqlx::mysql::MySqlPoolOptions;
use sqlx::{Column, Row, TypeInfo};
use std::time::Duration;

pub struct MysqlEngine {
    conn: String,
}

impl MysqlEngine {
    pub fn new(conn: &str) -> Self {
        // sqlx 只认 mysql://，而 mariadb:// 是同一套协议的另一个写法
        let conn = match conn.strip_prefix("mariadb://") {
            Some(rest) => format!("mysql://{rest}"),
            None => conn.to_string(),
        };
        Self { conn }
    }

    async fn pool(&self) -> anyhow::Result<sqlx::MySqlPool> {
        Ok(MySqlPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&self.conn)
            .await?)
    }
}

/// 列类型 → 按哪一档去读。
///
/// **分派与读取分开**，因为只有前者测得了：造一个 `MySqlRow` 需要真的连上
/// 服务器，而这张表恰恰是这个文件里最容易写错的地方。
#[derive(Debug, PartialEq, Eq)]
enum Cell {
    Bool,
    /// 有符号整数。驱动的 `i64` 明确排除 UNSIGNED，所以无符号另走一档
    Int,
    /// `BIGINT UNSIGNED` 一类。驱动给的类型名**带 UNSIGNED 后缀**，
    /// 认不出来就会掉进兜底档，而兜底读不出整数——整列变 null
    UnsignedInt,
    Float,
    /// `DECIMAL`。驱动把它同时挡在 `f64`（"floating-point numbers have
    /// different semantics"）和 `String`（不在字符串兼容表里）之外，
    /// 只能用 `BigDecimal` 读。金额列几乎都是这个类型，掉档的代价最大
    Decimal,
    Json,
    Date,
    Time,
    DateTime,
    Timestamp,
    /// 兜底：先字符串，再字节。整数不走这里——`42` 变成 `"42"` 之后，
    /// 模型对它的算术不一样
    Text,
}

/// 类型名 → 档位。名字取自驱动的 `ColumnType::name`，那张表是这个函数的唯一依据：
/// `TINYINT(1)` 报 `BOOLEAN`、`DECIMAL` 与 `NEWDECIMAL` 都报 `DECIMAL`、
/// 无符号整数带 ` UNSIGNED` 后缀。
fn cell_kind(type_name: &str) -> Cell {
    let ty = type_name.to_ascii_uppercase();
    // 后缀先判：`INT UNSIGNED` 落到 `INT` 那一档会用 i64 去读，驱动直接拒绝
    if let Some(base) = ty.strip_suffix(" UNSIGNED") {
        return match base {
            "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" => Cell::UnsignedInt,
            "FLOAT" | "DOUBLE" => Cell::Float,
            "DECIMAL" => Cell::Decimal,
            _ => Cell::Text,
        };
    }
    match ty.as_str() {
        "BOOLEAN" | "BOOL" => Cell::Bool,
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "BIGINT" => Cell::Int,
        "FLOAT" | "DOUBLE" => Cell::Float,
        "DECIMAL" => Cell::Decimal,
        "JSON" => Cell::Json,
        "DATE" => Cell::Date,
        "TIME" => Cell::Time,
        "DATETIME" => Cell::DateTime,
        "TIMESTAMP" => Cell::Timestamp,
        _ => Cell::Text,
    }
}

/// 一格的值 → JSON。取不出来就退字符串，再取不出来才 null。
fn cell_to_json(row: &sqlx::mysql::MySqlRow, i: usize, type_name: &str) -> serde_json::Value {
    use serde_json::Value;
    // 每个分支都取 `Option<T>`，NULL 在各自那一档里变成 JSON null——
    // 不做统一的前置探测：二进制列取 `Option<String>` 会报错而不是给 None，
    // 那种探测会把一个有值的 BLOB 判成空
    let ty = type_name.to_ascii_uppercase();
    match cell_kind(&ty) {
        Cell::Bool => row
            .try_get::<Option<bool>, _>(i)
            .map(|v| v.map_or(Value::Null, Value::Bool))
            .unwrap_or(Value::Null),
        Cell::Int => row
            .try_get::<Option<i64>, _>(i)
            .map(|v| v.map_or(Value::Null, |n| n.into()))
            .unwrap_or(Value::Null),
        Cell::UnsignedInt => row
            .try_get::<Option<u64>, _>(i)
            .map(|v| v.map_or(Value::Null, |n| n.into()))
            .unwrap_or(Value::Null),
        // BigDecimal → 文本 → coerce，与 Databricks / Snowflake 的 DECIMAL 同一条路。
        // 转数走 coerce 里的 f64，超出 f64 精度的值会留成字符串而不是变成一个近似数
        Cell::Decimal => row
            .try_get::<Option<sqlx::types::BigDecimal>, _>(i)
            .ok()
            .flatten()
            .map_or(Value::Null, |d| {
                coerce("DECIMAL", &Value::String(d.to_string()))
            }),
        Cell::Float => row
            .try_get::<Option<f64>, _>(i)
            .ok()
            .flatten()
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        Cell::Json => row
            .try_get::<Option<serde_json::Value>, _>(i)
            .ok()
            .flatten()
            .unwrap_or(Value::Null),
        Cell::Date => try_text(row, i, |r| {
            r.try_get::<Option<chrono::NaiveDate>, _>(i)
                .map(|v| v.map(|d| d.to_string()))
        }),
        Cell::Time => try_text(row, i, |r| {
            r.try_get::<Option<chrono::NaiveTime>, _>(i)
                .map(|v| v.map(|t| t.to_string()))
        }),
        Cell::DateTime => try_text(row, i, |r| {
            r.try_get::<Option<chrono::NaiveDateTime>, _>(i)
                .map(|v| v.map(|t| t.to_string()))
        }),
        Cell::Timestamp => try_text(row, i, |r| {
            r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(i)
                .map(|v| v.map(|t| t.to_rfc3339()))
        }),
        Cell::Text => {
            // CHAR / VARCHAR / TEXT / ENUM / SET 走这里。仍然过一遍 coerce：
            // 服务器把数字列声明成字符串的情况不少见
            let raw = row
                .try_get::<Option<String>, _>(i)
                .ok()
                .flatten()
                .map(Value::String);
            match raw {
                Some(v) => coerce(&ty, &v),
                // 二进制列（BLOB / VARBINARY / GEOMETRY）不是合法 UTF-8，
                // 给个长度而不是塞一堆转义字节进模型的上下文
                None => match row.try_get::<Option<Vec<u8>>, _>(i) {
                    Ok(Some(bytes)) => Value::String(format!("<{} bytes>", bytes.len())),
                    _ => Value::Null,
                },
            }
        }
    }
}

/// 按类型读文本，读不出来退回通用字符串（时间列被服务器配置成字符串时会走到）。
fn try_text<F>(row: &sqlx::mysql::MySqlRow, i: usize, f: F) -> serde_json::Value
where
    F: Fn(&sqlx::mysql::MySqlRow) -> Result<Option<String>, sqlx::Error>,
{
    match f(row) {
        Ok(Some(s)) => serde_json::Value::String(s),
        Ok(None) => serde_json::Value::Null,
        Err(_) => row
            .try_get::<Option<String>, _>(i)
            .ok()
            .flatten()
            .map_or(serde_json::Value::Null, serde_json::Value::String),
    }
}

#[async_trait::async_trait]
impl QueryEngine for MysqlEngine {
    async fn test(&self) -> anyhow::Result<()> {
        let pool = self.pool().await?;
        sqlx::query("SELECT 1").execute(&pool).await?;
        pool.close().await;
        Ok(())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        let pool = self.pool().await?;
        // MySQL 的 schema 就是 database。系统库按名字排除——information_schema
        // 在这里是**要排除的对象**，与 PG 那边同名的概念不是一回事
        let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT table_schema, table_name, column_name, column_type, column_comment
             FROM information_schema.columns
             WHERE table_schema NOT IN
                   ('information_schema', 'mysql', 'performance_schema', 'sys')
             ORDER BY table_schema, table_name, ordinal_position",
        )
        .fetch_all(&pool)
        .await?;
        pool.close().await;
        Ok(rows
            .into_iter()
            .map(|(schema, table, column, data_type, comment)| SchemaColumn {
                schema,
                table,
                column,
                data_type,
                // 没有注释时这一列是空串而不是 NULL，照抄会让每张表都挂一个空注释
                comment: comment.filter(|c| !c.trim().is_empty()),
            })
            .collect())
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult> {
        let pool = self.pool().await?;
        // 纵深防御第 3 层：会话只读 + 语句超时。只读在自动提交下逐条生效，
        // parser 万一漏网也写不进去
        sqlx::query("SET SESSION TRANSACTION READ ONLY")
            .execute(&pool)
            .await?;
        let millis = STATEMENT_TIMEOUT_SECS * 1000;
        let mysql_form = sqlx::query(&format!("SET SESSION max_execution_time = {millis}"))
            .execute(&pool)
            .await;
        if mysql_form.is_err() {
            // MariaDB：秒，且是浮点
            sqlx::query(&format!(
                "SET SESSION max_statement_time = {STATEMENT_TIMEOUT_SECS}"
            ))
            .execute(&pool)
            .await
            .map_err(|e| {
                anyhow::anyhow!("Could not set a statement timeout on this server: {e}")
            })?;
        }

        let fetched = sqlx::query(&wrap_limit(sql)).fetch_all(&pool).await?;
        pool.close().await;

        let (fetched, truncated) = truncate_rows(fetched);
        let Some(first) = fetched.first() else {
            return Ok(QueryResult {
                rows: Vec::new(),
                truncated,
            });
        };
        // 列名与类型只取一次：同一结果集每行的列都一样
        let columns: Vec<String> = first
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        let types: Vec<String> = first
            .columns()
            .iter()
            .map(|c| c.type_info().name().to_string())
            .collect();
        let values: Vec<Vec<serde_json::Value>> = fetched
            .iter()
            .map(|row| {
                (0..columns.len())
                    .map(|i| cell_to_json(row, i, &types[i]))
                    .collect()
            })
            .collect();
        Ok(QueryResult {
            rows: rows_to_json_lines(&columns, &values),
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{cell_kind, Cell, MysqlEngine, QueryEngine};

    /// 对着真服务器跑的那一档。没有 `UTOPIA_TEST_MYSQL_URL` 就跳过——
    /// 这三样（information_schema 的列名、两种超时写法、取值往返）是
    /// 类型表之外唯一测不到的部分，而它们只在真服务器上才有答案。
    ///
    /// 起一个来跑：
    /// `docker run -d -e MARIADB_ROOT_PASSWORD=pw -p 13306:3306 mariadb:11.4`
    /// 然后 `UTOPIA_TEST_MYSQL_URL=mysql://root:pw@127.0.0.1:13306/sales`。
    /// 建表语句见 #316。
    fn live_url() -> Option<String> {
        std::env::var("UTOPIA_TEST_MYSQL_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
    }

    #[tokio::test]
    async fn a_live_server_answers_with_typed_values() {
        let Some(url) = live_url() else {
            return;
        };
        let engine = MysqlEngine::new(&url);
        engine.test().await.expect("SELECT 1");

        // information_schema 的列名与 PG 不同（column_type / column_comment），
        // 写错了不会报错，只会让 schema 文档少一半
        let schema = engine.fetch_schema().await.expect("schema");
        let amount = schema
            .iter()
            .find(|c| c.table == "orders" && c.column == "amount")
            .expect("orders.amount");
        assert!(
            amount.data_type.starts_with("decimal"),
            "column_type 要给出带精度的形态，拿到的是 {}",
            amount.data_type
        );
        assert_eq!(
            amount.comment.as_deref(),
            Some("Order total in CNY"),
            "注释要跟着列走"
        );
        assert!(
            schema.iter().all(|c| c.comment.as_deref() != Some("")),
            "空注释是空串不是 NULL，要过滤掉"
        );

        // 取值往返：每一种掉档都会在这里现形——数变成字符串，或者整列 null
        let r = engine
            .execute(
                "SELECT id, region, amount, qty, flag, placed_on FROM sales.orders ORDER BY id",
            )
            .await
            .expect("execute");
        assert_eq!(r.rows.len(), 3);
        let first: serde_json::Value = serde_json::from_str(&r.rows[0]).unwrap();
        assert_eq!(first["id"], serde_json::json!(1));
        assert_eq!(first["region"], serde_json::json!("east"));
        // DECIMAL：没有 BigDecimal 这一格就是 null
        assert_eq!(first["amount"], serde_json::json!(1234.56));
        // BIGINT UNSIGNED：i64 装不下，且驱动拒绝用 i64 读它
        assert_eq!(first["qty"], serde_json::json!(18446744073709551615u64));
        // TINYINT(1) 报的类型名是 BOOLEAN
        assert_eq!(first["flag"], serde_json::json!(true));
        assert_eq!(first["placed_on"], serde_json::json!("2023-06-01"));

        // 全 NULL 的那一行：每一格都该是 JSON null，而不是某个类型的零值
        let third: serde_json::Value = serde_json::from_str(&r.rows[2]).unwrap();
        for k in ["region", "amount", "qty", "flag", "placed_on"] {
            assert_eq!(third[k], serde_json::Value::Null, "{k} 该是 null");
        }

        // 写路径仍然被闸挡住（第 1 层），只读会话是第 3 层
        assert!(super::super::guard_sql_for("mysql", "DELETE FROM sales.orders").is_err());
    }

    #[test]
    fn mariadb_is_the_same_protocol_under_another_name() {
        // sqlx 只认 mysql://，而界面允许写 mariadb://——不改写的话驱动会以
        // 「未知 scheme」拒掉一个完全正常的连接串
        assert_eq!(
            MysqlEngine::new("mariadb://u:p@h:3306/db").conn,
            "mysql://u:p@h:3306/db"
        );
        assert_eq!(
            MysqlEngine::new("mysql://u:p@h:3306/db").conn,
            "mysql://u:p@h:3306/db"
        );
        // 只剥前缀：密码里出现同样的字符不该被动到
        assert_eq!(
            MysqlEngine::new("mysql://u:mariadb://x@h/db").conn,
            "mysql://u:mariadb://x@h/db"
        );
    }

    /// 这三个断言各对应一种「整列变 null」。名字来自驱动的 `ColumnType::name`——
    /// 那张表是判据，不是猜的。
    #[test]
    fn every_number_shape_lands_in_a_readable_slot() {
        for t in ["TINYINT", "SMALLINT", "MEDIUMINT", "INT", "BIGINT", "int"] {
            assert_eq!(cell_kind(t), Cell::Int, "{t}");
        }
        // 驱动的 i64 明确排除 UNSIGNED，而类型名带着后缀过来。
        // 少了这一档，一个 BIGINT UNSIGNED 列会掉进兜底，兜底读不出整数
        for t in [
            "TINYINT UNSIGNED",
            "SMALLINT UNSIGNED",
            "MEDIUMINT UNSIGNED",
            "INT UNSIGNED",
            "BIGINT UNSIGNED",
        ] {
            assert_eq!(cell_kind(t), Cell::UnsignedInt, "{t}");
        }
        // DECIMAL 被驱动同时挡在 f64 与 String 之外，只有 BigDecimal 读得出。
        // 金额列几乎都是这个类型，掉档的代价最大
        assert_eq!(cell_kind("DECIMAL"), Cell::Decimal);
        assert_eq!(cell_kind("DECIMAL UNSIGNED"), Cell::Decimal);
        assert_eq!(cell_kind("DOUBLE"), Cell::Float);
        assert_eq!(cell_kind("DOUBLE UNSIGNED"), Cell::Float);
        // TINYINT(1) 在这个驱动里就报 BOOLEAN，不是 TINYINT
        assert_eq!(cell_kind("BOOLEAN"), Cell::Bool);
        assert_eq!(cell_kind("VARCHAR"), Cell::Text);
        assert_eq!(cell_kind("BLOB"), Cell::Text);
    }

    #[test]
    fn each_time_type_keeps_its_own_shape() {
        // 四种时间各读各的。DATE 当成 DATETIME 读会报错退回字符串，
        // 拿到的就不是 2023-06-01 而是驱动的原始形态
        assert_eq!(cell_kind("DATE"), Cell::Date);
        assert_eq!(cell_kind("TIME"), Cell::Time);
        assert_eq!(cell_kind("DATETIME"), Cell::DateTime);
        assert_eq!(cell_kind("TIMESTAMP"), Cell::Timestamp);
    }
}
