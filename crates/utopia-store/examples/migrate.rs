//! 把 `UTOPIA_DATABASE_URL` 指向的库迁移到最新，然后退出。
//!
//! 连库测试假定库已经迁移好：store 的一百多个集成测试和 server 的多数 fixture 一上来
//! 就建数据，不自己跑迁移，只有零星几个调 `db::migrate`。对着一个全新空库直接
//! `cargo test --workspace`，先跑到的测试会撞 relation "organizations" does not exist
//! （#869 验证时就是这样红了 32 个）。CI 从前靠 sqlx-cli 先 `migrate run` 一遍，可装它
//! 要一分多钟，而连库测试如今在关键路径上的 backend job 里跑；这个 example 复用测试
//! 本来就要编的 utopia-store，几乎不花额外时间。本地同理：`docker compose up -d db`
//! 之后 `cargo run -p utopia-store --example migrate`，连库测试就能跑。
//!
//! 它只前滚，不做 sqlx-cli 的另一件事——在已迁移过的库上再放一遍看是否安全，
//! 那仍是 migrations job 的活。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = std::env::var("UTOPIA_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("UTOPIA_DATABASE_URL 未设置"))?;
    // 建表建触发器要的权限比运行期高，和 utopia-server 启动时一样只开一个小池子、用完就关
    let pool = utopia_store::db::connect(&url, Some(2)).await?;
    utopia_store::db::migrate(&pool).await?;
    pool.close().await;
    // 不回显地址：它带密码，而这行会进 CI 日志
    println!("数据库迁移完成");
    Ok(())
}
