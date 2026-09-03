//! 外层兜底**调得上去**，而且两处缺省说的是同一个数（迁移 0011）。
//!
//! 为什么非要连库：这个 bug 的形态是**约束在 SQL、校验在 Rust，两边各自漂移**。
//! `set_worker_concurrency` 放行 1..=256，而列上的 CHECK 曾是 `BETWEEN 1 AND 32`——
//! 从设置页填 33 到 256 之间任何一个值，Rust 说行、数据库说不行，用户看到的是
//! 一条 CHECK 约束报错。`cargo check` 和 clippy 对此一个字都不说，因为两边
//! 根本不在同一种语言里。
//!
//! 约束的上限当初等于列的缺省（都是 32），于是**兜底一格都调不上去**——
//! 而 0001 的注释写着它"要明显大于各模型限额之和，否则被限流的任务会占满槽位
//! 把别的饿死"。约束堵死了它自己的设计。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。只读 + 改完还原，绝不留下痕迹。

use sqlx::PgPool;

/// Rust 放行的值，数据库必须也放行。
///
/// 逐个试边界而不是只试一个：漂移可能出在任何一档上，而这几次查询很便宜。
#[tokio::test]
async fn every_value_rust_accepts_the_database_accepts_too() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;

    let before = utopia_store::access::worker_concurrency(&pool).await?;

    let run = async {
        // 33 是当初被卡死的第一格；256 是 Rust 侧的上限
        for v in [1_i32, 33, 64, 255, 256] {
            utopia_store::access::set_worker_concurrency(&pool, v)
                .await
                .map_err(|e| anyhow::anyhow!("Rust 放行了 {v}，数据库却拒绝：{e}"))?;
            let got = utopia_store::access::worker_concurrency(&pool).await?;
            assert_eq!(got, v, "写进去 {v} 读回来却是 {got}");
        }

        // 反向：Rust 拒绝的，不该悄悄落库
        for v in [0_i32, 257, -1] {
            assert!(
                utopia_store::access::set_worker_concurrency(&pool, v)
                    .await
                    .is_err(),
                "{v} 越界了却被接受"
            );
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    // 还原：这是共享的部署设置，测试不该改变别人的运行参数
    utopia_store::access::set_worker_concurrency(&pool, before).await?;
    run
}

/// 表里没有行时 Rust 的兜底值，必须与列的缺省是同一个数。
///
/// 两处分开写（一处 SQL、一处 Rust），改一处不会带上另一处。不一致的后果很隐蔽：
/// 有行的库跑一个数、空表的库跑另一个数，而两者都不报错。
#[tokio::test]
async fn the_two_defaults_say_the_same_number() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;

    // 列的缺省：直接问 information_schema，不猜
    let column_default: Option<String> = sqlx::query_scalar(
        "SELECT column_default FROM information_schema.columns
          WHERE table_name = 'deployment_settings' AND column_name = 'worker_concurrency'",
    )
    .fetch_one(&pool)
    .await?;
    let column_default: i32 = column_default
        .as_deref()
        .and_then(|s| s.split("::").next())
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| anyhow::anyhow!("读不出列缺省：{column_default:?}"))?;

    // Rust 的兜底：把行藏起来再问一次
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM deployment_settings")
        .execute(&mut *tx)
        .await?;
    let fallback: Option<(i32,)> =
        sqlx::query_as("SELECT worker_concurrency FROM deployment_settings LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;
    assert!(fallback.is_none(), "行没删掉，下面这句就白测了");
    tx.rollback().await?; // **一定回滚**：deployment_settings 是单例，删了服务就没设置了

    // access.rs 的 unwrap_or 里那个数
    let rust_fallback = 64;
    assert_eq!(
        column_default, rust_fallback,
        "列缺省是 {column_default}，Rust 兜底是 {rust_fallback}——两处漂开了"
    );
    Ok(())
}
