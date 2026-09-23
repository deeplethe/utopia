//! 索引由任务建（0035）：写入一侧只负责「排一条」，且排一次就够。
//!
//! - 第一次写下某个维度：排上一条 `build_vector_index`
//! - 再写：不再排（队列里已经有一条）
//! - 建好之后再写：什么都不排（进程里记住了）
//! - 超过 HNSW 上限的维度：不排，也建不了——这种失败不该重试
//!
//! 建索引本身与查询的一致性在 `the_nearest_chunk_is_found_however_it_is_reached` 里。

use sqlx::PgPool;
use utopia_store::vector_index::{self, Target, JOB_KIND, MAX_DIMS};

/// 这个测试独占的维度
const DIMS: usize = 9;

async fn queued(pool: &PgPool, dims: usize) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = $1 AND payload = $2 AND status = 'queued'",
    )
    .bind(JOB_KIND)
    .bind(serde_json::json!({ "table": "chunks", "dims": dims }))
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn the_first_write_of_a_dimension_queues_one_build() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // 上一次跑留下的都清掉，从「什么都没有」开始
    vector_index::drop(&pool, Target::Chunks, DIMS).await?;
    sqlx::query("DELETE FROM jobs WHERE kind = $1 AND payload->>'dims' = $2")
        .bind(JOB_KIND)
        .bind(DIMS.to_string())
        .execute(&pool)
        .await?;

    let run = async {
        let first = vector_index::request(&pool, Target::Chunks, DIMS).await?;
        assert!(first.is_some(), "第一次写下 9 维：排上一条");
        assert_eq!(queued(&pool, DIMS).await?, 1);
        let second = vector_index::request(&pool, Target::Chunks, DIMS).await?;
        assert_eq!(second, None, "队列里已经有一条，不再排");
        assert_eq!(queued(&pool, DIMS).await?, 1);

        let built = vector_index::build(&pool, Target::Chunks, DIMS).await?;
        assert!(built.created);
        assert_eq!(
            vector_index::status(&pool, Target::Chunks, DIMS).await?,
            Some(true)
        );
        // 任务跑完那条会被标 done；这里模拟一下，好让下一问只靠「进程里记住了」
        sqlx::query("UPDATE jobs SET status = 'done' WHERE kind = $1 AND payload->>'dims' = $2")
            .bind(JOB_KIND)
            .bind(DIMS.to_string())
            .execute(&pool)
            .await?;
        let third = vector_index::request(&pool, Target::Chunks, DIMS).await?;
        assert_eq!(third, None, "建好了：什么都不排");
        assert_eq!(queued(&pool, DIMS).await?, 0);

        // 超过上限的维度
        let too_wide = MAX_DIMS + 1;
        assert_eq!(
            vector_index::request(&pool, Target::Chunks, too_wide).await?,
            None,
            "建不了的不排"
        );
        assert_eq!(queued(&pool, too_wide).await?, 0);
        let err = vector_index::build(&pool, Target::Chunks, too_wide)
            .await
            .expect_err("HNSW on vector holds up to 2000 dims");
        assert!(
            matches!(err, utopia_core::AppError::Validation(_)),
            "是 Validation，分发层据此判 Terminal：{err:?}"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = vector_index::drop(&pool, Target::Chunks, DIMS).await;
    sqlx::query("DELETE FROM jobs WHERE kind = $1 AND payload->>'dims' = $2")
        .bind(JOB_KIND)
        .bind(DIMS.to_string())
        .execute(&pool)
        .await?;
    run
}

/// 两个维度同时建，旁边还有事务在碰 `chunks`。两条 `CREATE INDEX CONCURRENTLY` 各自要等
/// 表上其他事务结束，也各自算对方要等的事务，于是 Postgres 报 deadlock detected：本地复现
/// 一轮约一半概率，错开 10ms 以上就不会（#886 把测试合成一个进程时 3/3 撞上；server 的
/// worker 并发认领两条 build_vector_index 任务是同一件事，并发默认 64）。
/// `build` 里的会话级咨询锁让后到的那条等前一条建完。连做八轮，不加锁时几乎必红
#[tokio::test]
async fn two_dimensions_built_at_once_take_turns() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // 这个测试独占的两个维度
    const A: usize = 11;
    const B: usize = 13;
    let mut outcome = Ok(());
    for round in 0..8 {
        for dims in [A, B] {
            vector_index::drop(&pool, Target::Chunks, dims).await?;
        }
        // 摄取还在往 chunks 写：CIC 要等这些事务，死锁就靠它凑齐
        let writer = async {
            for _ in 0..20 {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT count(*) FROM chunks")
                    .execute(&mut *tx)
                    .await?;
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                tx.commit().await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let (a, b, w) = tokio::join!(
            vector_index::build(&pool, Target::Chunks, A),
            vector_index::build(&pool, Target::Chunks, B),
            writer
        );
        w?;
        match (a, b) {
            (Ok(a), Ok(b)) => assert!(
                a.created && b.created,
                "第 {round} 轮两条都建成：{a:?} {b:?}"
            ),
            (a, b) => {
                outcome = Err(anyhow::anyhow!("第 {round} 轮：{a:?} / {b:?}"));
                break;
            }
        }
    }
    for dims in [A, B] {
        let _ = vector_index::drop(&pool, Target::Chunks, dims).await;
    }
    outcome
}
