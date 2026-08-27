//! 问数数据源：系统层注册（凭据集中、跨 KB 复用）+ 知识库层挂载（权限跟 KB 走）。
//! 查询执行时的安全闸（只读会话、SQL 解析白名单、LIMIT/超时）在 server 侧。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::models::DataSourceView;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 连接串 → 无凭据摘要（host:port/db）。解析失败给占位符，绝不回显原串。
pub fn conn_summary(conn: &str) -> String {
    url::Url::parse(conn)
        .ok()
        .map(|u| {
            format!(
                "{}:{}{}",
                u.host_str().unwrap_or("?"),
                u.port().map(|p| p.to_string()).unwrap_or_else(|| "5432".into()),
                u.path()
            )
        })
        .unwrap_or_else(|| "(unparsed)".into())
}

pub async fn list(pool: &PgPool) -> AppResult<Vec<DataSourceView>> {
    let rows: Vec<(Uuid, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<bool>)> =
        sqlx::query_as(
            "SELECT id, name, engine, conn_string, created_at, last_test_at, last_test_ok
             FROM data_sources ORDER BY name",
        )
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, engine, conn, created_at, last_test_at, last_test_ok)| DataSourceView {
            id,
            name,
            engine,
            summary: conn_summary(&conn),
            created_at,
            last_test_at,
            last_test_ok,
        })
        .collect())
}

pub async fn create(
    pool: &PgPool,
    name: &str,
    engine: &str,
    conn_string: &str,
    created_by: Uuid,
) -> AppResult<Uuid> {
    if name.trim().is_empty() {
        return Err(AppError::Validation("Data source name is required".into()));
    }
    if engine != "postgres" {
        return Err(AppError::Validation(
            "Only the postgres engine is supported for now".into(),
        ));
    }
    if !conn_string.starts_with("postgres://") && !conn_string.starts_with("postgresql://") {
        return Err(AppError::Validation(
            "Connection string must start with postgres://".into(),
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO data_sources (id, name, engine, conn_string, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(name.trim())
    .bind(engine)
    .bind(conn_string.trim())
    .bind(created_by)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM data_sources WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 连接串只在服务端内部流转（测试连接/查询执行）。
pub async fn conn_string(pool: &PgPool, id: Uuid) -> AppResult<String> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT conn_string FROM data_sources WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    row.map(|(c,)| c).ok_or(AppError::NotFound)
}

pub async fn record_test(pool: &PgPool, id: Uuid, ok: bool) -> AppResult<()> {
    sqlx::query("UPDATE data_sources SET last_test_at = now(), last_test_ok = $2 WHERE id = $1")
        .bind(id)
        .bind(ok)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// KB 挂载
// ---------------------------------------------------------------------------

pub async fn mount(pool: &PgPool, kb_id: Uuid, data_source_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO kb_data_sources (kb_id, data_source_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(kb_id)
    .bind(data_source_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unmount(pool: &PgPool, kb_id: Uuid, data_source_id: Uuid) -> AppResult<()> {
    sqlx::query("DELETE FROM kb_data_sources WHERE kb_id = $1 AND data_source_id = $2")
        .bind(kb_id)
        .bind(data_source_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mounted(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<DataSourceView>> {
    let rows: Vec<(Uuid, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>, Option<bool>)> =
        sqlx::query_as(
            "SELECT d.id, d.name, d.engine, d.conn_string, d.created_at,
                    d.last_test_at, d.last_test_ok
             FROM kb_data_sources m JOIN data_sources d ON d.id = m.data_source_id
             WHERE m.kb_id = $1 ORDER BY d.name",
        )
        .bind(kb_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, engine, conn, created_at, last_test_at, last_test_ok)| DataSourceView {
            id,
            name,
            engine,
            summary: conn_summary(&conn),
            created_at,
            last_test_at,
            last_test_ok,
        })
        .collect())
}

/// (engine, conn_string)：查询执行/测试/拉 schema 用（凭据不出服务端）。
pub async fn engine_and_conn(pool: &PgPool, id: Uuid) -> AppResult<(String, String)> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT engine, conn_string FROM data_sources WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    row.ok_or(AppError::NotFound)
}
