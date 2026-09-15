//! 一个库同一时刻只跑一个治理任务（0043）。后开始的任务开头会放掉「正在裁」的标记，
//! 两个并排跑就会裁同一批、写重复的决定。锁不碰库里任何行，只要一条连接。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败（见 `utopia_store::test_db`）。

use sqlx::PgPool;
use utopia_store::governance::claim_run;
use uuid::Uuid;

#[tokio::test]
async fn a_second_run_on_the_same_base_waits_and_a_dropped_run_lets_go() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (kb, other) = (Uuid::now_v7(), Uuid::now_v7());

    let first = claim_run(&pool, kb)
        .await?
        .expect("the first run takes the base");
    assert!(
        claim_run(&pool, kb).await?.is_none(),
        "a second run on the same base waits"
    );
    let elsewhere = claim_run(&pool, other)
        .await?
        .expect("another base is not held");

    first.release().await;
    let again = claim_run(&pool, kb)
        .await?
        .expect("a released base can be taken again");

    // 任务半路被丢掉：锁跟着连接一起走，不会带着锁回池
    drop(again);
    let mut taken = None;
    for _ in 0..50 {
        taken = claim_run(&pool, kb).await?;
        if taken.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(taken.is_some(), "a dropped run lets the base go");

    elsewhere.release().await;
    if let Some(t) = taken {
        t.release().await;
    }
    Ok(())
}
