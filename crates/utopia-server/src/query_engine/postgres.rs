//! Postgres 族（顺带覆盖 Greenplum / Timescale 等 PG 兼容系）。线协议直连，
//! 是四个引擎里唯一有会话可设只读的那个。

use super::{QueryEngine, QueryResult, SchemaColumn, ROW_CAP, STATEMENT_TIMEOUT_SECS};
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use std::time::Duration;

pub struct PostgresEngine {
    conn: String,
}

impl PostgresEngine {
    pub fn new(conn: &str) -> Self {
        Self {
            conn: conn.to_string(),
        }
    }

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
        sqlx::query(&format!(
            "SET statement_timeout = '{STATEMENT_TIMEOUT_SECS}s'"
        ))
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
