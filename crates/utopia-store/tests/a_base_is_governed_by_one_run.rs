//! 一个库同一时刻只有一个治理任务（会话级咨询锁）。第二个任务抢不到锁就退出并晚点再排，
//! 而不是和第一个一起读同一个队头、把同一簇裁两遍。

use sqlx::PgPool;
use utopia_store::governance;
use uuid::Uuid;

#[tokio::test]
async fn the_second_run_on_a_base_does_not_get_the_lock_until_the_first_releases_it(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = Uuid::now_v7();
    let other = Uuid::now_v7();

    let first = governance::try_lock_base(&pool, kb)
        .await?
        .expect("a free base is locked at once");
    assert!(
        governance::try_lock_base(&pool, kb).await?.is_none(),
        "a second run on the same base must not get the lock"
    );
    assert!(
        governance::try_lock_base(&pool, other).await?.is_some(),
        "another base is another lock"
    );
    first.release().await;
    let again = governance::try_lock_base(&pool, kb).await?;
    assert!(again.is_some(), "released, the base can be governed again");
    again.unwrap().release().await;
    pool.close().await;
    Ok(())
}
