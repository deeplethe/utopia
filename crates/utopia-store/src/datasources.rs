//! 问数数据源：系统层注册（凭据集中、跨 KB 复用）+ 知识库层挂载（权限跟 KB 走）。
//! 查询执行时的安全闸（只读会话、SQL 解析白名单、LIMIT/超时）在 server 侧。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::models::DataSourceView;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// data_sources 的行投影，list 与 mounted 共用。
type DataSourceRow = (
    Uuid,
    String,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<bool>,
);

/// 连接串 → 无凭据摘要（host[:port]/path）。解析失败给占位符，绝不回显原串。
/// 端口没写就不补：四种 scheme 的默认端口各不相同，补错比不补更误导
pub fn conn_summary(conn: &str) -> String {
    url::Url::parse(conn)
        .ok()
        .map(|u| {
            format!(
                "{}{}{}",
                u.host_str().unwrap_or("?"),
                u.port().map(|p| format!(":{p}")).unwrap_or_default(),
                u.path()
            )
        })
        .unwrap_or_else(|| "(unparsed)".into())
}

/// 行 → 视图。**连接串在这里换成无凭据摘要**，是它不外流的那道关口；
/// 四条查询共用一份，免得哪天新加一条忘了换
fn row_to_view(
    (id, name, engine, conn, created_at, last_test_at, last_test_ok): DataSourceRow,
) -> DataSourceView {
    DataSourceView {
        id,
        name,
        engine,
        summary: conn_summary(&conn),
        created_at,
        last_test_at,
        last_test_ok,
    }
}

/// 部署里全部的源。**只给系统管理员的注册台用。**
/// 从前可挂载列表也走它，那是 0014 关掉的门——KB 侧现在走
/// `granted_to_workspace`。
pub async fn list(pool: &PgPool) -> AppResult<Vec<DataSourceView>> {
    let rows: Vec<DataSourceRow> = sqlx::query_as(
        "SELECT id, name, engine, conn_string, created_at, last_test_at, last_test_ok
             FROM data_sources ORDER BY name",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_view).collect())
}

pub async fn create(
    pool: &PgPool,
    name: &str,
    engine: &str,
    conn_string: &str,
    created_by: Uuid,
) -> AppResult<Uuid> {
    if name.trim().is_empty() {
        return Err(AppError::invalid(
            "ds_name_required",
            "Data source name is required",
        ));
    }
    // 引擎由调用方按连接串的 scheme 定（`query_engine::engine_from_conn`）；
    // 允许的取值在迁移 0020 的 CHECK 里，这里不再复制一份
    if engine.is_empty() || conn_string.trim().is_empty() {
        return Err(AppError::invalid(
            "bad_conn_string",
            "A connection string is required",
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
    let rows: Vec<DataSourceRow> = sqlx::query_as(
        "SELECT d.id, d.name, d.engine, d.conn_string, d.created_at,
                    d.last_test_at, d.last_test_ok
             FROM kb_data_sources m JOIN data_sources d ON d.id = m.data_source_id
             WHERE m.kb_id = $1 ORDER BY d.name",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_view).collect())
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

/// 这个工作区被授权用哪些源（0014）。
///
/// **可挂载列表从此走这里，不再走 `list`。** 从前那条路给 KB 管理员看的是
/// `datasources::list(pool)`——全部署每一个源，不过滤。于是任何库的管理员
/// 都能把任意生产库挂进自己库，而挂上之后该库每个 Viewer 都能对它跑只读 SQL。
pub async fn granted_to_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
) -> AppResult<Vec<DataSourceView>> {
    let rows: Vec<DataSourceRow> = sqlx::query_as(
        "SELECT d.id, d.name, d.engine, d.conn_string, d.created_at,
                d.last_test_at, d.last_test_ok
           FROM data_source_grants g JOIN data_sources d ON d.id = g.data_source_id
          WHERE g.workspace_id = $1 ORDER BY d.name",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(row_to_view).collect())
}

/// 这个源授权给了哪些工作区。管理台读它。
pub async fn grants_for_source(
    pool: &PgPool,
    data_source_id: Uuid,
) -> AppResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT w.id, w.name FROM data_source_grants g
           JOIN workspaces w ON w.id = g.workspace_id
          WHERE g.data_source_id = $1 ORDER BY w.name",
    )
    .bind(data_source_id)
    .fetch_all(pool)
    .await?)
}

/// 授权一个工作区用这个源。幂等。
pub async fn grant(
    pool: &PgPool,
    data_source_id: Uuid,
    workspace_id: Uuid,
    actor: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO data_source_grants (data_source_id, workspace_id, granted_by)
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(data_source_id)
    .bind(workspace_id)
    .bind(actor)
    .execute(pool)
    .await?;
    Ok(())
}

/// 收回授权，**连同该工作区里已经挂上的那些一起卸掉**。
///
/// 只删授权行不够：`mounted` 读的是 `kb_data_sources`，问数也读它。留着挂载
/// 就等于收回了授权而访问照旧——一个不生效的权限撤销比没有还危险。
///
/// 一个事务里做完：两条 DELETE 之间如果崩了，留下的正是「无授权却挂着」那种
/// 状态，而那恰恰是这条迁移要消灭的东西。
pub async fn revoke(pool: &PgPool, data_source_id: Uuid, workspace_id: Uuid) -> AppResult<u64> {
    let mut tx = pool.begin().await?;
    let unmounted = sqlx::query(
        "DELETE FROM kb_data_sources
          WHERE data_source_id = $1
            AND kb_id IN (SELECT id FROM knowledge_bases WHERE workspace_id = $2)",
    )
    .bind(data_source_id)
    .bind(workspace_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM data_source_grants WHERE data_source_id = $1 AND workspace_id = $2")
        .bind(data_source_id)
        .bind(workspace_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(unmounted)
}

/// 这个源对这个知识库是不是授权过的。**挂载前必查**——守卫不能只在列表那一侧：
/// 列表过滤挡的是「看得见」，而挂载端点是照着 id 调的，谁都能自己拼一个。
pub async fn is_granted(pool: &PgPool, kb_id: Uuid, data_source_id: Uuid) -> AppResult<bool> {
    let found: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM data_source_grants g
           JOIN knowledge_bases kb ON kb.workspace_id = g.workspace_id
          WHERE kb.id = $1 AND g.data_source_id = $2",
    )
    .bind(kb_id)
    .bind(data_source_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}
