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
        // 在这里是**要排除的对象**，与 PG 那边同名的概念不是一回事。
        //
        // **每列都要 CAST(... AS CHAR)。** MySQL 8.0 起 information_schema 是建在
        // 数据字典上的视图，这些列经二进制协议报成 VARBINARY，sqlx 严格类型解码
        // 会拒绝把它读进 String（`VARCHAR is not compatible with VARBINARY`）。
        // 原始 SQL 在命令行里看着好好的——命令行不做强类型解码，这一处只有连真
        // 服务器才现形。MariaDB 上 CAST 无害，两边同一条语句
        let cols: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR),
                    CAST(column_name AS CHAR), CAST(column_type AS CHAR),
                    CAST(column_comment AS CHAR)
             FROM information_schema.columns
             WHERE table_schema NOT IN
                   ('information_schema', 'mysql', 'performance_schema', 'sys')
             ORDER BY table_schema, table_name, ordinal_position",
        )
        .fetch_all(&pool)
        .await?;
        // 键是锦上添花：读不出来就照从前那样只给列，不让整次取 schema 失败（#502）。
        // 可见性：`statistics` 与 `key_column_usage` 对只有 SELECT 的用户照样有行
        // （实测 MariaDB 11.4）；`table_constraints` / `referential_constraints` 对这种
        // 用户是空的——所以键从前两张视图读，不要换成后两张
        let keys = match keys(&pool).await {
            Ok(k) => k,
            Err(e) => {
                tracing::warn!(error = %e, "读不出 MySQL 主键/外键，schema 不带键标记");
                Keys::default()
            }
        };
        pool.close().await;

        Ok(cols
            .into_iter()
            .map(|(schema, table, column, data_type, comment)| {
                let key = (schema.clone(), table.clone(), column.clone());
                let is_primary_key = keys.primary.contains(&key);
                let references_table = keys.foreign.get(&key).cloned();
                SchemaColumn {
                    schema,
                    table,
                    column,
                    data_type,
                    // 没有注释时这一列是空串而不是 NULL，照抄会让每张表都挂一个空注释
                    comment: comment.filter(|c| !c.trim().is_empty()),
                    is_primary_key,
                    references_table,
                }
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

type ColumnKey = (String, String, String);

/// `keys()` UNION ALL 的返回形状：前三列是 (schema, table, column)；后两列是
/// 仅 FK 行填的（PK 行是 NULL 占位）
type KeyRow = (String, String, String, Option<String>, Option<String>);

/// 一个库里的单列主键与单列外键，按 (schema, table, column) 查
#[derive(Default)]
struct Keys {
    primary: std::collections::HashSet<ColumnKey>,
    /// 外键列 → 它指向的 `schema.table`
    foreign: std::collections::HashMap<ColumnKey, String>,
}

/// 从 information_schema 读键，两支 UNION ALL：
///
/// - 单列主键：`statistics` 里 `index_name = 'PRIMARY'`，按表分组、只有一列的那些；
/// - 单列外键：`key_column_usage` 里 `referenced_table_name` 非空，按约束分组、只有
///   一列的那些。
///
/// 组合主键、组合外键的成员都不标——探索提示词拿 PK 当 ID、拿 FK 当关联路径，
/// 把组合键的一列单独标出来是误导。
///
/// **每张视图只扫一遍，用 GROUP BY 数列，不写逐行的相关子查询。** MariaDB（与
/// MySQL 5.7）每次读 information_schema 都现场重建这张表：逐行子查询在 1,000 张表时
/// 要 19 秒、2,000 张时 4 分钟，而取 schema 跑在挂载请求里、没有语句超时，慢到头
/// 就是挂住。分组写法在 2,000 张表上是 0.6 秒。
///
/// 外键约束名在一个库里唯一（InnoDB），按 (库, 表, 约束名) 分组分得开。一列同时在
/// 两个单列外键里（少见）：五列全排序、先到先得，结果不随服务器的返回顺序变。
///
/// 列名经二进制协议报成 VARBINARY（MySQL 8.0+），跟 fetch_schema 的列查询一样
/// 必须 CAST(... AS CHAR) 才能让 sqlx 用 String 读出来
async fn keys(pool: &sqlx::MySqlPool) -> anyhow::Result<Keys> {
    let rows: Vec<KeyRow> = sqlx::query_as(
        "SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR),
                CAST(MIN(column_name) AS CHAR),
                NULL AS ref_schema, NULL AS ref_table
           FROM information_schema.statistics
          WHERE index_name = 'PRIMARY'
            AND table_schema NOT IN
                ('information_schema', 'mysql', 'performance_schema', 'sys')
          GROUP BY table_schema, table_name
         HAVING COUNT(*) = 1
         UNION ALL
         SELECT CAST(table_schema AS CHAR), CAST(table_name AS CHAR),
                CAST(MIN(column_name) AS CHAR),
                CAST(MIN(referenced_table_schema) AS CHAR),
                CAST(MIN(referenced_table_name) AS CHAR)
           FROM information_schema.key_column_usage
          WHERE referenced_table_name IS NOT NULL
            AND table_schema NOT IN
                ('information_schema', 'mysql', 'performance_schema', 'sys')
          GROUP BY table_schema, table_name, constraint_name
         HAVING COUNT(*) = 1
         ORDER BY 1, 2, 3, 4, 5",
    )
    .fetch_all(pool)
    .await?;
    let mut keys = Keys::default();
    for (schema, table, column, ref_schema, ref_table) in rows {
        let key = (schema, table, column);
        match (ref_schema, ref_table) {
            (None, _) => {
                // 来自 UNION 的第一支，是 PK 行
                keys.primary.insert(key);
            }
            (Some(rs), Some(rt)) => {
                keys.foreign.entry(key).or_insert(format!("{rs}.{rt}"));
            }
            _ => {}
        }
    }
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::{cell_kind, Cell, MysqlEngine, QueryEngine, SchemaColumn};
    use sqlx::mysql::MySqlPoolOptions;
    use uuid::Uuid;

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

    /// 每个测试自己的库，名字带随机后缀：并行跑不撞，也不碰服务器上已有的库。
    /// 测完逐个 `DROP DATABASE`。**这是 MySQL 的「schema」，跟 PG 的命名空间
    /// 不同——MySQL 的 schema 就是 database**（见 fetch_schema 顶上的注释）
    struct Fx {
        url: String,
        pool: sqlx::MySqlPool,
        schemas: Vec<String>,
    }

    impl Fx {
        async fn new(schemas: usize) -> Option<Self> {
            let url = live_url()?;
            // 连接串照原样用：带不带库名都行，CREATE DATABASE 不需要当前库
            let pool = MySqlPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .expect("connect");
            let suffix = Uuid::now_v7().simple().to_string();
            let schemas: Vec<String> = (0..schemas)
                .map(|i| format!("`mysql_keys_{i}_{}`", &suffix[suffix.len() - 12..]))
                .collect();
            for s in &schemas {
                sqlx::query(&format!("CREATE DATABASE {s}"))
                    .execute(&pool)
                    .await
                    .expect("create schema");
            }
            Some(Self { url, pool, schemas })
        }

        async fn exec(&self, sql: &str) {
            sqlx::raw_sql(sql).execute(&self.pool).await.expect(sql);
        }

        async fn columns(&self, url: &str) -> Vec<SchemaColumn> {
            let mut cols = MysqlEngine::new(url).fetch_schema().await.expect("schema");
            // 只留本测试建的库。库名带反引号，先去掉
            let our: std::collections::HashSet<String> = self
                .schemas
                .iter()
                .map(|s| s.trim_matches('`').to_string())
                .collect();
            cols.retain(|c| our.contains(&c.schema));
            cols
        }

        async fn cleanup(self) {
            // 跨库外键会让 DROP DATABASE ... 顺序敏感——B 库里的表引用 A 库，
            // 先 drop A 库就报 FK 约束。临时关掉 session 级的外键检查再依次删
            sqlx::query("SET FOREIGN_KEY_CHECKS = 0")
                .execute(&self.pool)
                .await
                .expect("disable fk checks");
            for s in &self.schemas {
                let unquoted = s.trim_matches('`');
                sqlx::query(&format!("DROP DATABASE IF EXISTS `{unquoted}`"))
                    .execute(&self.pool)
                    .await
                    .expect("drop schema");
            }
            sqlx::query("SET FOREIGN_KEY_CHECKS = 1")
                .execute(&self.pool)
                .await
                .expect("enable fk checks");
            self.pool.close().await;
        }
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

    /// 单列主键标出来；组合主键、组合外键的成员都不标
    #[tokio::test]
    async fn a_single_column_key_is_marked_and_a_composite_one_is_not_mysql() {
        let Some(fx) = Fx::new(1).await else { return };
        let a = fx.schemas[0].clone();
        fx.exec(&format!(
            "CREATE TABLE {a}.parent (id INT PRIMARY KEY AUTO_INCREMENT, name VARCHAR(100) NOT NULL, note TEXT);
             CREATE TABLE {a}.line (
                 order_id INT NOT NULL,
                 ordinal INT NOT NULL,
                 payload TEXT,
                 PRIMARY KEY (order_id, ordinal)
             );
             CREATE TABLE {a}.line_note (
                 order_id INT,
                 ordinal INT,
                 FOREIGN KEY (order_id, ordinal) REFERENCES {a}.line (order_id, ordinal)
             );"
        ))
        .await;

        let cols = fx.columns(&fx.url).await;
        let id = col(&cols, a.trim_matches('`'), "parent", "id");
        assert!(id.is_primary_key && id.references_table.is_none());
        let name = col(&cols, a.trim_matches('`'), "parent", "name");
        assert!(!name.is_primary_key);
        let note = col(&cols, a.trim_matches('`'), "parent", "note");
        assert!(!note.is_primary_key && note.references_table.is_none());
        for c in ["order_id", "ordinal"] {
            let member = col(&cols, a.trim_matches('`'), "line", c);
            assert!(!member.is_primary_key, "组合主键的成员 line.{c} 不标");
            let fk_member = col(&cols, a.trim_matches('`'), "line_note", c);
            assert!(
                fk_member.references_table.is_none(),
                "组合外键的成员 line_note.{c} 不标"
            );
        }
        fx.cleanup().await;
    }

    /// 外键带上它指向的表：自引用、跨库；一列同时在两个单列外键里时取排序在前的那个
    #[tokio::test]
    async fn a_foreign_key_points_at_its_own_target_mysql() {
        let Some(fx) = Fx::new(2).await else { return };
        let (a, b) = (fx.schemas[0].clone(), fx.schemas[1].clone());
        let au = a.trim_matches('`');
        let bu = b.trim_matches('`');
        fx.exec(&format!(
            "CREATE TABLE {a}.p1 (id INT PRIMARY KEY AUTO_INCREMENT, parent INT, FOREIGN KEY (parent) REFERENCES {a}.p1 (id));
             CREATE TABLE {a}.p2 (id INT PRIMARY KEY AUTO_INCREMENT);
             CREATE TABLE {a}.c1 (x INT, FOREIGN KEY (x) REFERENCES {a}.p1 (id));
             CREATE TABLE {a}.c2 (y INT, FOREIGN KEY (y) REFERENCES {a}.p2 (id));
             CREATE TABLE {a}.c3 (z INT, FOREIGN KEY (z) REFERENCES {a}.p1 (id));
             CREATE TABLE {b}.orders (item_id INT NOT NULL, FOREIGN KEY (item_id) REFERENCES {a}.p2 (id));
             CREATE TABLE {a}.dual_fk (x INT,
                 FOREIGN KEY (x) REFERENCES {a}.p2 (id),
                 FOREIGN KEY (x) REFERENCES {a}.p1 (id));"
        ))
        .await;

        let cols = fx.columns(&fx.url).await;
        let target = |s: &str, t: &str, c: &str| col(&cols, s, t, c).references_table.clone();
        let p1 = format!("{au}.p1");
        let p2 = format!("{au}.p2");
        assert_eq!(target(au, "p1", "parent"), Some(p1.clone()), "自引用");
        assert_eq!(target(au, "c1", "x"), Some(p1.clone()));
        assert_eq!(target(au, "c2", "y"), Some(p2.clone()), "c2 指 p2 不是 p1");
        assert_eq!(target(au, "c3", "z"), Some(p1.clone()));
        assert_eq!(target(bu, "orders", "item_id"), Some(p2.clone()), "跨库");
        assert_eq!(
            target(au, "dual_fk", "x"),
            Some(p1.clone()),
            "两条外键取排序在前的"
        );
        let p2_id = col(&cols, au, "p2", "id");
        assert!(
            p2_id.is_primary_key && p2_id.references_table.is_none(),
            "p2.id 是 PK，不是 FK"
        );
        fx.cleanup().await;
    }

    /// 只有 SELECT 权限的连接照样读得到键：BI 连接大多就是这种用户。
    /// 建不了用户（测试账号没有 CREATE USER / GRANT）就跳过
    #[tokio::test]
    async fn a_read_only_login_still_sees_the_keys_mysql() {
        let Some(fx) = Fx::new(1).await else { return };
        let a = fx.schemas[0].clone();
        let au = a.trim_matches('`');
        fx.exec(&format!(
            "CREATE TABLE {a}.p (id INT PRIMARY KEY AUTO_INCREMENT);
             CREATE TABLE {a}.c (p_id INT, FOREIGN KEY (p_id) REFERENCES {a}.p (id));"
        ))
        .await;
        let user = format!("ro_{}", &Uuid::now_v7().simple().to_string()[..16]);
        let password = &Uuid::now_v7().simple().to_string()[..16];
        if let Err(e) = sqlx::query(&format!(
            "CREATE USER '{user}'@'%' IDENTIFIED BY '{password}'"
        ))
        .execute(&fx.pool)
        .await
        {
            eprintln!("跳过：建不了只读角色（{e}）");
            fx.cleanup().await;
            return;
        }
        let grant = format!("GRANT SELECT ON {a}.* TO '{user}'@'%';");
        match sqlx::query(&grant).execute(&fx.pool).await {
            Ok(_) => {}
            Err(e) => {
                // 测试用户没有 GRANT 权（root 之外）：跳过这一档
                eprintln!("跳过：GRANT 失败（{e}）");
                let _ = sqlx::query(&format!("DROP USER '{user}'@'%'"))
                    .execute(&fx.pool)
                    .await;
                fx.cleanup().await;
                return;
            }
        }

        // 换成只读用户，并去掉库名：它只在测试库上有 SELECT，连到连接串里写的
        // 那个库（比如 `/sales`）会被拒
        let ro_url = {
            let mut parsed = url::Url::parse(&fx.url).expect("parse mysql url");
            parsed.set_username(&user).expect("username");
            parsed.set_password(Some(password)).expect("password");
            parsed.set_path("/");
            parsed.to_string()
        };

        let cols = fx.columns(&ro_url).await;
        assert!(col(&cols, au, "p", "id").is_primary_key);
        assert_eq!(
            col(&cols, au, "c", "p_id").references_table,
            Some(format!("{au}.p"))
        );

        sqlx::query(&format!("DROP USER '{user}'@'%'"))
            .execute(&fx.pool)
            .await
            .expect("drop user");
        fx.cleanup().await;
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
