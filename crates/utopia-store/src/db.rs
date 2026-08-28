use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

/// 连接池上限的缺省。
///
/// 与 worker 并发的缺省（32）对齐：任务大部分时间在等 HTTP 应答、并不占着连接，
/// 但每分块的 epoch 检查、未匹配统计这些短查询会成串涌来，池子太小就变成排队。
/// 曾经写死 10，而 worker 默认 32——三倍超发，撞上来会先是请求变慢再是超时，
/// 而不是任何一处报错说"池子不够"。
const DEFAULT_MAX_CONNECTIONS: u32 = 32;

pub async fn connect(database_url: &str, max_connections: Option<u32>) -> anyhow::Result<PgPool> {
    let max = max_connections.unwrap_or(DEFAULT_MAX_CONNECTIONS).max(2);
    let pool = PgPoolOptions::new()
        .max_connections(max)
        // 取不到连接时早点响亮地失败，而不是把请求悬在默认的 30 秒上——
        // 池子配小了要看得出来是池子的问题
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await?;
    tracing::info!(max_connections = max, "数据库连接池已建立");
    Ok(pool)
}

pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    Ok(())
}
