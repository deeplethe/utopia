//! 写回测试只操作自己的任务行，不启动会认领全库任务的 worker。

use super::{persist_outcome, Job, JobOutcome, RequeueScope, DEFER_WINDOW_SECS};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use utopia_core::{Deferred, Terminal};
use uuid::Uuid;

async fn running_job(pool: &PgPool, attempts: i32) -> anyhow::Result<Job> {
    Ok(sqlx::query_as(
        "INSERT INTO jobs (kind, payload, status, attempts, max_attempts,
                           locked_at, last_error)
         VALUES ($1, '{}', 'running', $2, 3, now(), 'previous failure')
         RETURNING id, kind, payload, attempts, max_attempts, locked_at",
    )
    .bind(format!("test_writeback_{}", Uuid::now_v7()))
    .bind(attempts)
    .fetch_one(pool)
    .await?)
}

async fn claim_job(pool: &PgPool, id: i64) -> anyhow::Result<Job> {
    Ok(sqlx::query_as(
        "UPDATE jobs SET status = 'running', attempts = attempts + 1,
                         locked_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'queued'
         RETURNING id, kind, payload, attempts, max_attempts, locked_at",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct JobState {
    status: String,
    attempts: i32,
    locked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    run_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    payload: Value,
}

async fn state(pool: &PgPool, id: i64) -> anyhow::Result<JobState> {
    Ok(sqlx::query_as(
        "SELECT status, attempts, locked_at, last_error, run_at, updated_at, payload
         FROM jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

async fn remove(pool: &PgPool, id: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn an_old_claim_cannot_overwrite_any_new_running_result() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let completed_at = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let ordinary = anyhow::anyhow!("network failure").context("provider request");
    let terminal = anyhow::anyhow!("balance gone").context(Terminal);
    let deferred = anyhow::anyhow!("index busy").context(Deferred::new(Duration::from_secs(17)));

    // 窗口内和窗口外的旧 Deferred 都不能把新认领写成失败。
    for expired in [false, true] {
        let job = running_job(&pool, 1).await?;
        let payload = if expired {
            json!({"deferred_since": (completed_at - chrono::Duration::hours(2)).to_rfc3339()})
        } else {
            json!({})
        };
        // 先提交重排，再从新事务认领；不伪造认领时间，也不认领其他测试的任务。
        sqlx::query("UPDATE jobs SET status = 'queued', attempts = 0, payload = $2 WHERE id = $1")
            .bind(job.id)
            .bind(payload)
            .execute(&pool)
            .await?;
        let new_job = claim_job(&pool, job.id).await?;
        assert_eq!(new_job.attempts, job.attempts);
        assert_ne!(new_job.locked_at, job.locked_at);
        let new_execution = state(&pool, job.id).await?;

        for outcome in [
            JobOutcome::Done,
            JobOutcome::failed(&job, &ordinary),
            JobOutcome::failed(&job, &terminal),
            JobOutcome::failed(&job, &deferred),
        ] {
            assert!(!persist_outcome(&pool, &job, &outcome, completed_at).await?);
            assert_eq!(state(&pool, job.id).await?, new_execution);
        }
        remove(&pool, job.id).await?;
    }
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn manual_requeue_changes_claim_time_after_resetting_attempts() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let old_job = running_job(&pool, 1).await?;
    let completed_at = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let error = anyhow::anyhow!("permanent provider failure").context(Terminal);
    let old_outcome = JobOutcome::failed(&old_job, &error);
    assert!(persist_outcome(&pool, &old_job, &old_outcome, completed_at).await?);

    let count = super::requeue_failed(
        &pool,
        RequeueScope {
            kind: Some(&old_job.kind),
            ..RequeueScope::default()
        },
    )
    .await?;
    assert_eq!(count, 1);
    let requeued = state(&pool, old_job.id).await?;
    assert_eq!(requeued.attempts, 0);
    assert_eq!(requeued.locked_at, Some(old_job.locked_at));

    // 只认领本测试已经提交重排的行，使用数据库返回的原始时间。
    let new_job = claim_job(&pool, old_job.id).await?;
    assert_eq!(new_job.attempts, old_job.attempts);
    assert_ne!(new_job.locked_at, old_job.locked_at);
    let new_execution = state(&pool, new_job.id).await?;
    for outcome in [JobOutcome::Done, old_outcome] {
        assert!(!persist_outcome(&pool, &old_job, &outcome, completed_at).await?);
        assert_eq!(state(&pool, new_job.id).await?, new_execution);
    }

    assert!(persist_outcome(&pool, &new_job, &JobOutcome::Done, completed_at).await?);
    assert_eq!(state(&pool, new_job.id).await?.last_error, None);
    remove(&pool, new_job.id).await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn a_repeated_ack_preserves_the_first_writeback() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let completed_at = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let deferred = anyhow::anyhow!("index busy").context(Deferred::new(Duration::from_secs(17)));
    let ordinary =
        anyhow::anyhow!("provider detail: request rejected").context("outer request context");
    let terminal = anyhow::anyhow!("balance gone").context(Terminal);
    let terminal_and_deferred = anyhow::anyhow!("balance gone after waiting")
        .context(Deferred::new(Duration::from_secs(17)))
        .context(Terminal)
        .context("outer request context");

    for (attempts, error, expected_status, expected_attempts, retry_secs) in [
        (2, None, "done", 2, None),
        (2, Some(&ordinary), "queued", 2, Some(120)),
        (3, Some(&ordinary), "failed", 3, None),
        (2, Some(&terminal), "failed", 2, None),
        (1, Some(&terminal_and_deferred), "failed", 1, None),
        (2, Some(&deferred), "queued", 1, Some(17)),
    ] {
        let job = running_job(&pool, attempts).await?;
        let before = state(&pool, job.id).await?;
        let outcome = match error {
            Some(error) => JobOutcome::failed(&job, error),
            None => JobOutcome::Done,
        };
        assert!(persist_outcome(&pool, &job, &outcome, completed_at).await?);
        let first_ack = state(&pool, job.id).await?;
        assert_eq!(first_ack.status, expected_status);
        assert_eq!(first_ack.attempts, expected_attempts);
        assert_eq!(
            first_ack.last_error,
            error.map(|error| format!("{error:#}"))
        );
        if let Some(retry_secs) = retry_secs {
            assert_eq!(
                first_ack.run_at,
                completed_at + chrono::Duration::seconds(retry_secs)
            );
        }
        if expected_attempts == attempts {
            assert_eq!(first_ack.payload, before.payload);
        } else {
            // 只有有效 Deferred 退还预算，并首次写入等待窗口起点。
            let since: DateTime<Utc> = sqlx::query_scalar(
                "SELECT (payload->>'deferred_since')::timestamptz FROM jobs WHERE id = $1",
            )
            .bind(job.id)
            .fetch_one(&pool)
            .await?;
            assert_eq!(since, completed_at);
        }
        let duplicate_ack = persist_outcome(
            &pool,
            &job,
            &outcome,
            completed_at + chrono::Duration::hours(1),
        )
        .await?;
        assert!(!duplicate_ack);
        assert_eq!(state(&pool, job.id).await?, first_ack);
        remove(&pool, job.id).await?;
    }
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn deferred_window_uses_completion_time_and_current_database_payload() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // 故意用早于当前时间的完成时刻，写回等待不能令有效的 Deferred 过期。
    let completed_at = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let error = anyhow::anyhow!("index busy").context(Deferred::new(Duration::from_secs(17)));
    for seconds_inside_window in [1, 0] {
        let job = running_job(&pool, 2).await?;
        let since =
            completed_at - chrono::Duration::seconds(DEFER_WINDOW_SECS - seconds_inside_window);
        let payload = json!({"deferred_since": since.to_rfc3339(), "kept": "value"});
        // handler 可以在认领后更新 payload，窗口起点以已写入数据库的值为准。
        sqlx::query("UPDATE jobs SET payload = $2 WHERE id = $1")
            .bind(job.id)
            .bind(&payload)
            .execute(&pool)
            .await?;
        let outcome = JobOutcome::failed(&job, &error);
        assert!(persist_outcome(&pool, &job, &outcome, completed_at).await?);
        let written = state(&pool, job.id).await?;
        assert_eq!(written.status, "queued");
        assert_eq!(written.payload, payload);
        if seconds_inside_window == 1 {
            assert_eq!(written.attempts, 1, "valid Deferred does not spend budget");
            assert_eq!(written.run_at, completed_at + chrono::Duration::seconds(17));
        } else {
            assert_eq!(
                written.attempts, 2,
                "expired Deferred spends ordinary budget"
            );
            assert_eq!(
                written.run_at,
                completed_at + chrono::Duration::seconds(120)
            );
        }
        remove(&pool, job.id).await?;
    }
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn a_handler_that_already_marked_done_is_not_overwritten() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let job = running_job(&pool, 1).await?;
    // RSS hydration 会在自己的事务中提交 done。
    sqlx::query(
        "UPDATE jobs SET status = 'done', last_error = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(job.id)
    .execute(&pool)
    .await?;
    let handler_commit = state(&pool, job.id).await?;
    let error = anyhow::anyhow!("late error");
    for outcome in [JobOutcome::Done, JobOutcome::failed(&job, &error)] {
        assert!(!persist_outcome(&pool, &job, &outcome, Utc::now()).await?);
        assert_eq!(state(&pool, job.id).await?, handler_commit);
    }
    remove(&pool, job.id).await?;
    pool.close().await;
    Ok(())
}

#[tokio::test]
async fn a_closed_pool_does_not_keep_writeback_retry_alive() -> anyhow::Result<()> {
    // 已关闭的惰性池不会建立连接，此测试不依赖数据库。
    let pool = PgPool::connect_lazy("postgres://test:test@127.0.0.1:1/test")?;
    pool.close().await;
    let job = Job {
        id: 0,
        kind: "closed_pool_writeback".into(),
        payload: json!({}),
        attempts: 1,
        max_attempts: 3,
        locked_at: Utc::now(),
    };
    tokio::time::timeout(
        Duration::from_secs(1),
        super::persist_with_retry(&pool, &job, &JobOutcome::Done, Utc::now()),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn closing_the_pool_cancels_writeback_while_it_waits_for_a_row_lock() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let control = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    let job = running_job(&control, 1).await?;
    let before = state(&control, job.id).await?;
    let mut locked = control.begin().await?;
    let locker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *locked)
        .await?;
    sqlx::query("SELECT id FROM jobs WHERE id = $1 FOR UPDATE")
        .bind(job.id)
        .execute(&mut *locked)
        .await?;

    let writer_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&writer_pool)
        .await?;
    let mut writer = tokio::spawn({
        let pool = writer_pool.clone();
        let job = job.clone();
        async move {
            super::persist_with_retry(&pool, &job, &JobOutcome::Done, Utc::now()).await;
        }
    });

    let result: anyhow::Result<()> = async {
        // 观测自己的后端确实在等这把锁，再关闭池，不靠固定睡眠猜测执行进度。
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let waiting: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                     WHERE pid = $1 AND wait_event_type = 'Lock'
                       AND $2 = ANY(pg_blocking_pids(pid)))",
                )
                .bind(writer_pid)
                .bind(locker_pid)
                .fetch_one(&control)
                .await?;
                if waiting {
                    return anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;

        let (finished, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(&mut writer, writer_pool.close())
        })
        .await?;
        finished?;
        let unchanged = state(&control, job.id).await?;
        anyhow::ensure!(
            unchanged == before,
            "pool closure changed the locked job: {unchanged:?}"
        );
        anyhow::Ok(())
    }
    .await;

    // 失败路径也撤销自己的行锁并终止测试 task，不能把挂起的写回留给其他测试。
    writer.abort();
    let unlocked = locked.rollback().await;
    let closed = tokio::time::timeout(Duration::from_secs(2), writer_pool.close()).await;
    let removed = remove(&control, job.id).await;
    control.close().await;
    result?;
    unlocked?;
    closed?;
    removed?;
    Ok(())
}
