use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::time::Duration;

/// 连接池上限的缺省。
///
/// **它不再与 worker 并发相等**（worker 缺省已是 64，见迁移 0011），这是有意的：
/// 后台任务大部分时间在等模型应答，那段时间既不占连接、也被按模型的信号量压在
/// 十来个以内。池子要覆盖的是**真在干活**的那些——每块的 epoch 检查、向量检索、
/// 未匹配统计这类短查询，它们会成串涌来。
///
/// 曾经写死 10，而 worker 默认 32——三倍超发，撞上来会先是请求变慢再是超时，
/// 而不是任何一处报错说"池子不够"。所以这个数的判据是"同时在跑的短查询有多少"，
/// 不是"有多少个任务槽位"；worker 再往上提时该重量的是前者。
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
