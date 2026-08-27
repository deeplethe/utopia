//! 问数查询引擎：trait 接缝（BlobStore 同手法）+ 引擎无关的安全闸。
//!
//! 引擎按协议族扩，不按产品名扩：postgres（本文件）→ mysql 线协议族（白捡
//! TiDB/OceanBase/Doris/StarRocks）→ HTTP 族（ClickHouse、Trino——后者一个顶起
//! Iceberg/Delta/Hive 整个湖仓生态）。挂载模型与注册表引擎无关，加引擎零迁移。
//!
//! 安全闸（纵深防御，不信任模型）：
//! 1. sqlparser 解析：仅放行单条 SELECT/WITH（含 CTE），拒绝 DML/DDL/多语句/SELECT INTO
//! 2. 强制外包一层 LIMIT（cap+1 探测截断）
//! 3. 会话级只读 + 语句超时（引擎各自的机制，parser 万一漏网也写不进去）
//! 4. 结果统一为 JSON Lines（各引擎都有原生 JSON 行输出，也是模型最好消化的格式）

use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::time::Duration;

/// 行数上限（外包 LIMIT cap+1，第 201 行只用来判断截断）。
pub const ROW_CAP: usize = 200;
const STATEMENT_TIMEOUT_SECS: u32 = 10;

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

/// 引擎工厂。conn 凭据只在服务端流转。
pub fn engine_for(engine: &str, conn: &str) -> anyhow::Result<Box<dyn QueryEngine>> {
    match engine {
        "postgres" => Ok(Box::new(PostgresEngine { conn: conn.to_string() })),
        other => anyhow::bail!("Unsupported engine: {other}"),
    }
}

/// 安全闸第 1 层：解析并校验，返回规整后的语句文本。
pub fn guard_sql(sql: &str) -> anyhow::Result<String> {
    let cleaned = sql.trim().trim_end_matches(';').trim();
    if cleaned.is_empty() {
        anyhow::bail!("Empty SQL");
    }
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, cleaned)
        .map_err(|e| anyhow::anyhow!("SQL parse error: {e}"))?;
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
        Statement::Drop { .. } => "DROP",
        Statement::CreateTable { .. } | Statement::CreateView { .. } => "CREATE",
        Statement::AlterTable { .. } => "ALTER",
        Statement::Truncate { .. } => "TRUNCATE",
        Statement::Copy { .. } => "COPY",
        _ => "a non-SELECT statement",
    }
}

// ---------------------------------------------------------------------------
// Postgres 族（顺带覆盖 Greenplum/Timescale 等 PG 兼容系）
// ---------------------------------------------------------------------------

pub struct PostgresEngine {
    conn: String,
}

impl PostgresEngine {
    async fn pool(&self) -> anyhow::Result<sqlx::PgPool> {
        Ok(PgPoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect(&self.conn)
            .await?)
    }
}

#[async_trait::async_trait]
impl QueryEngine for PostgresEngine {
    async fn test(&self) -> anyhow::Result<()> {
        let pool = self.pool().await?;
        sqlx::query("SELECT 1").execute(&pool).await?;
        pool.close().await;
        Ok(())
    }

    async fn fetch_schema(&self) -> anyhow::Result<Vec<SchemaColumn>> {
        let pool = self.pool().await?;
        let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT c.table_schema, c.table_name, c.column_name,
                    c.data_type, pgd.description
             FROM information_schema.columns c
             LEFT JOIN pg_catalog.pg_statio_all_tables st
               ON st.schemaname = c.table_schema AND st.relname = c.table_name
             LEFT JOIN pg_catalog.pg_description pgd
               ON pgd.objoid = st.relid AND pgd.objsubid = c.ordinal_position
             WHERE c.table_schema NOT IN ('pg_catalog', 'information_schema')
             ORDER BY c.table_schema, c.table_name, c.ordinal_position",
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
                comment,
            })
            .collect())
    }

    async fn execute(&self, sql: &str) -> anyhow::Result<QueryResult> {
        let pool = self.pool().await?;
        // 纵深防御第 3 层：会话级只读 + 超时（parser 漏网也写不进去、跑不死库）
        sqlx::query("SET default_transaction_read_only = on")
            .execute(&pool)
            .await?;
        sqlx::query(&format!("SET statement_timeout = '{STATEMENT_TIMEOUT_SECS}s'"))
            .execute(&pool)
            .await?;
        // 第 2 层：外包 LIMIT；row_to_json 让 PG 全权处理类型→JSON（文本键序保留列序）
        let wrapped = format!(
            "SELECT row_to_json(_q)::text AS _j FROM ( {sql} ) AS _q LIMIT {}",
            ROW_CAP + 1
        );
        let fetched = sqlx::query(&wrapped).fetch_all(&pool).await?;
        pool.close().await;

        let truncated = fetched.len() > ROW_CAP;
        let rows = fetched
            .into_iter()
            .take(ROW_CAP)
            .map(|r| r.try_get::<String, _>("_j").unwrap_or_else(|_| "{}".into()))
            .collect();
        Ok(QueryResult { rows, truncated })
    }
}

#[cfg(test)]
mod tests {
    use super::guard_sql;

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
}
