//! 写回测试只操作自己的任务行，不启动会认领全库任务的 worker。

use super::{persist_outcome, persist_with_retry, Job, JobOutcome, DEFER_WINDOW_SECS};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use utopia_core::Deferred;
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
async fn a_refused_write_retries_the_computed_result() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let job = running_job(&pool, 1).await?;
    let object_name = format!("test_writeback_retry_{}", Uuid::now_v7().simple());

    let result: anyhow::Result<()> = async {
        // sequence 不随拒写事务回滚，计数能证明第一次写入确实失败。
        sqlx::raw_sql(&format!(
            "CREATE SEQUENCE {object_name};
             CREATE FUNCTION {object_name}() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN
                 IF nextval('{object_name}') = 1 THEN
                     RAISE EXCEPTION 'injected result writeback failure';
                 END IF;
                 RETURN NEW;
             END; $$;
             CREATE TRIGGER {object_name} BEFORE UPDATE ON jobs
             FOR EACH ROW WHEN (OLD.id = {id} AND OLD.kind = '{kind}'
                                AND OLD.status = 'running' AND NEW.status = 'done')
             EXECUTE FUNCTION {object_name}();",
            id = job.id,
            kind = job.kind,
        ))
        .execute(&pool)
        .await?;

        tokio::time::timeout(
            Duration::from_secs(10),
            persist_with_retry(&pool, &job, &JobOutcome::Done, Utc::now()),
        )
        .await?;
        let writes: i64 = sqlx::query_scalar(&format!("SELECT last_value FROM {object_name}"))
            .fetch_one(&pool)
            .await?;
        anyhow::ensure!(
            writes == 2,
            "expected one refused write and one successful retry"
        );
        let written = state(&pool, job.id).await?;
        anyhow::ensure!(written.status == "done" && written.last_error.is_none());
        anyhow::ensure!(written.attempts == job.attempts);
        anyhow::ensure!(written.locked_at == Some(job.locked_at));
        Ok(())
    }
    .await;

    let dropped = sqlx::raw_sql(&format!(
        "DROP TRIGGER IF EXISTS {object_name} ON jobs;
         DROP FUNCTION IF EXISTS {object_name}();
         DROP SEQUENCE IF EXISTS {object_name};"
    ))
    .execute(&pool)
    .await;
    let removed = remove(&pool, job.id).await;
    pool.close().await;
    result?;
    dropped?;
    removed?;
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
async fn a_repeated_ack_preserves_the_first_writeback() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let completed_at = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
    let deferred = anyhow::anyhow!("index busy").context(Deferred::new(Duration::from_secs(17)));
    let ordinary =
        anyhow::anyhow!("provider detail: request rejected").context("outer request context");

    for (error, starts_deferral) in [
        (None, false),
        (Some(&ordinary), false),
        (Some(&deferred), true),
    ] {
        let job = running_job(&pool, 2).await?;
        let outcome = match error {
            Some(error) => JobOutcome::failed(&job, error),
            None => JobOutcome::Done,
        };
        assert!(persist_outcome(&pool, &job, &outcome, completed_at).await?);
        let first_ack = state(&pool, job.id).await?;
        if starts_deferral {
            let since: DateTime<Utc> = sqlx::query_scalar("SELECT $1::text::timestamptz")
                .bind(first_ack.payload["deferred_since"].as_str())
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
