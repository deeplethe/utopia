//! #1106：真实 worker 保留处理结果，只重试写回，不再跑 handler。
//!
//! opt-in，且只接受名字以 `utopia_job_writeback_1106_` 开头的空测试库：
//! `run_worker` 会回收全库 running、认领全库 queued，不能与其他测试共用库。
//! 写回拒绝计数用 PostgreSQL sequence，抛错回滚不会把计数一起回滚。

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::{atomic::AtomicUsize, Arc, Mutex};
use std::time::Duration;
use utopia_core::{Deferred, Terminal};
use utopia_store::jobs;

const KIND: &str = "test_job_writeback_1106";

#[derive(Debug, sqlx::FromRow)]
struct JobState {
    status: String,
    attempts: i32,
    last_error: Option<String>,
    run_at: DateTime<Utc>,
    locked_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
    payload: serde_json::Value,
}

async fn state(pool: &PgPool, id: i64) -> anyhow::Result<JobState> {
    Ok(sqlx::query_as(
        "SELECT status, attempts, last_error, run_at, locked_at, updated_at, payload
           FROM jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

async fn wait_for_state(pool: &PgPool, id: i64, expected: &str) -> anyhow::Result<JobState> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let current = state(pool, id).await?;
            if current.status == expected {
                return Ok(current);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

async fn wait_for_two_rejections(pool: &PgPool) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let rejected: i64 = sqlx::query_scalar(
                "SELECT CASE WHEN is_called THEN last_value ELSE 0 END
                   FROM test_job_writeback_1106_rejections",
            )
            .fetch_one(pool)
            .await?;
            if rejected >= 2 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

fn handler_result(mode: &str) -> anyhow::Result<()> {
    let error = anyhow::anyhow!("original handler error: {mode}");
    match mode {
        "success" => Ok(()),
        "panic" => panic!("injected handler panic"),
        "ordinary" | "exhausted" => Err(error),
        "terminal" => Err(error.context(Terminal)),
        "deferred" | "expired_deferred" => {
            Err(error.context(Deferred::new(Duration::from_secs(600))))
        }
        "terminal_and_deferred" => Err(error
            .context(Deferred::new(Duration::from_secs(600)))
            .context(Terminal)),
        _ => anyhow::bail!("unexpected test outcome: {mode}"),
    }
}

async fn enqueue_case(pool: &PgPool, mode: &str, budget: i32) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await?;
    let mut payload = json!({"mode": mode});
    if mode == "expired_deferred" {
        payload["deferred_since"] = json!((Utc::now() - chrono::Duration::days(1)).to_rfc3339());
    }
    let id = jobs::enqueue_with_max_attempts_tx(&mut tx, KIND, payload, budget).await?;
    sqlx::query("UPDATE jobs SET last_error = 'obsolete handler error' WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE test_job_writeback_1106_gate SET job_id = $1, rejecting = true")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT setval('test_job_writeback_1106_rejections', 1, false)")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id)
}

async fn allow_writeback(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("UPDATE test_job_writeback_1106_gate SET rejecting = false")
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "opt-in worker regression; requires a dedicated empty utopia_job_writeback_1106_* database"]
async fn a_job_result_survives_a_writeback_failure() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let control = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&control)
        .await?;
    anyhow::ensure!(
        database.starts_with("utopia_job_writeback_1106_"),
        "this worker regression requires its own utopia_job_writeback_1106_* database"
    );
    utopia_store::db::migrate(&control).await?;
    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM jobs) OR EXISTS (SELECT 1 FROM organizations)",
    )
    .fetch_one(&control)
    .await?;
    anyhow::ensure!(
        !occupied,
        "this worker regression requires an empty database"
    );

    sqlx::raw_sql(
        "CREATE TABLE test_job_writeback_1106_gate (job_id bigint, rejecting boolean NOT NULL);
         INSERT INTO test_job_writeback_1106_gate VALUES (NULL, false);
         CREATE SEQUENCE test_job_writeback_1106_rejections;
         CREATE FUNCTION test_job_writeback_1106_reject() RETURNS trigger
         LANGUAGE plpgsql AS $$
         BEGIN
             IF OLD.kind = 'test_job_writeback_1106' AND OLD.status = 'running'
                AND NEW.status IN ('done', 'failed', 'queued')
                AND EXISTS (SELECT 1 FROM test_job_writeback_1106_gate
                             WHERE job_id = OLD.id AND rejecting) THEN
                 PERFORM nextval('test_job_writeback_1106_rejections');
                 RAISE EXCEPTION 'injected result writeback failure';
             END IF;
             RETURN NEW;
         END;
         $$;
         CREATE TRIGGER test_job_writeback_1106_reject
         BEFORE UPDATE ON jobs FOR EACH ROW
         EXECUTE FUNCTION test_job_writeback_1106_reject();",
    )
    .execute(&control)
    .await?;

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let handler_calls = calls.clone();
    let worker = tokio::spawn(jobs::run_worker(
        pool.clone(),
        Arc::new(AtomicUsize::new(1)),
        move |job| {
            let calls = handler_calls.clone();
            async move {
                anyhow::ensure!(job.kind == KIND, "unexpected job kind: {}", job.kind);
                calls.lock().unwrap().push(job.id);
                handler_result(job.payload["mode"].as_str().unwrap_or("unknown"))
            }
        },
    ));

    let result = async {
        let mut expected_calls = Vec::new();
        for (mode, expected_status, expected_attempts, budget, retry_secs) in [
            ("success", "done", 1, 3, None),
            ("panic", "failed", 1, 1, None),
            ("ordinary", "queued", 1, 3, Some(30)),
            ("exhausted", "failed", 1, 1, None),
            ("terminal", "failed", 1, 3, None),
            ("deferred", "queued", 0, 3, Some(600)),
            ("terminal_and_deferred", "failed", 1, 3, None),
            ("expired_deferred", "queued", 1, 3, Some(30)),
        ] {
            let id = enqueue_case(&control, mode, budget).await?;
            expected_calls.push(id);
            wait_for_two_rejections(&control).await?;
            let blocked = state(&control, id).await?;
            anyhow::ensure!(blocked.status == "running" && blocked.attempts == 1);
            let claimed_at = blocked
                .locked_at
                .ok_or_else(|| anyhow::anyhow!("{mode}: the claim has no lock timestamp"))?;
            // LISTEN 占一条连接；写回退避必须释放另一条，不能耗尽小连接池。
            let mut available =
                tokio::time::timeout(Duration::from_secs(1), pool.acquire()).await??;
            sqlx::query("SELECT 1").execute(&mut *available).await?;
            drop(available);
            anyhow::ensure!(blocked.last_error.as_deref() == Some("obsolete handler error"));

            // 只有一个槽位时，已执行但未确认的任务仍须阻止继续派发。
            let waiting = if mode == "success" {
                let waiting = jobs::enqueue(&control, KIND, json!({"mode": "success"})).await?;
                tokio::time::sleep(Duration::from_millis(300)).await;
                let waiting_state = state(&control, waiting).await?;
                anyhow::ensure!(
                    waiting_state.status == "queued" && waiting_state.attempts == 0,
                    "writeback lost its slot"
                );
                Some(waiting)
            } else {
                None
            };

            allow_writeback(&control).await?;
            let final_state = wait_for_state(&control, id, expected_status).await?;
            anyhow::ensure!(
                final_state.attempts == expected_attempts
                    && final_state.locked_at == Some(claimed_at),
                "{mode}: writeback retry changed the execution identity or budget: {final_state:?}"
            );
            if mode == "success" {
                anyhow::ensure!(
                    final_state.last_error.is_none(),
                    "success must clear old errors"
                );
            } else if mode == "panic" {
                let error = final_state.last_error.as_deref().unwrap_or("");
                anyhow::ensure!(
                    error.contains("任务处理器 panic") && error.contains("injected handler panic"),
                    "panic: the original panic message was not preserved: {error}"
                );
            } else {
                let original_error = format!("{:#}", handler_result(mode).unwrap_err());
                anyhow::ensure!(
                    final_state.last_error.as_deref() == Some(original_error.as_str()),
                    "{mode}: the original error chain was not preserved"
                );
            }
            if let Some(retry_secs) = retry_secs {
                let scheduled_ms = (final_state.run_at - claimed_at).num_milliseconds();
                anyhow::ensure!(
                    (retry_secs * 1000 - 1000..=retry_secs * 1000 + 1500).contains(&scheduled_ms),
                    "{mode}: writing the same result again moved run_at: {scheduled_ms} ms"
                );
                // 推迟本测试的业务重试，避免后续阶段再次执行，保留原有预算。
                sqlx::query("UPDATE jobs SET run_at = now() + interval '1 day' WHERE id = $1")
                    .bind(id)
                    .execute(&control)
                    .await?;
            }
            if mode == "deferred" {
                anyhow::ensure!(final_state.payload["deferred_since"].is_string());
                let deferred_since: DateTime<Utc> = sqlx::query_scalar(
                    "SELECT (payload->>'deferred_since')::timestamptz FROM jobs WHERE id = $1",
                )
                .bind(id)
                .fetch_one(&control)
                .await?;
                let started_after_claim_ms = (deferred_since - claimed_at).num_milliseconds();
                anyhow::ensure!(
                    (0..=1500).contains(&started_after_claim_ms),
                    "writeback retry moved the Deferred window: {started_after_claim_ms} ms"
                );
                anyhow::ensure!(
                    final_state.updated_at >= deferred_since + chrono::Duration::seconds(2),
                    "updated_at must record the delayed database update, not handler completion"
                );
            }
            if let Some(waiting) = waiting {
                expected_calls.push(waiting);
                wait_for_state(&control, waiting, "done").await?;
            }
        }

        // 两次执行的 attempts 相同，旧成功和旧失败也不能覆盖新认领。
        for mode in ["success", "terminal_and_deferred", "deferred"] {
            let id = enqueue_case(&control, mode, 3).await?;
            expected_calls.push(id);
            wait_for_two_rejections(&control).await?;
            let old_state = state(&control, id).await?;
            let old_claimed_at = old_state
                .locked_at
                .ok_or_else(|| anyhow::anyhow!("old claim has no lock timestamp"))?;
            // 先提交重排，再获取新认领时间，与实际恢复路径一致。
            let mut requeue = control.begin().await?;
            sqlx::query("UPDATE test_job_writeback_1106_gate SET rejecting = false")
                .execute(&mut *requeue)
                .await?;
            sqlx::query(
                "UPDATE jobs SET status = 'queued', attempts = 0, locked_at = NULL,
                    updated_at = now() WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *requeue)
            .await?;
            sqlx::query("UPDATE test_job_writeback_1106_gate SET rejecting = true")
                .execute(&mut *requeue)
                .await?;
            requeue.commit().await?;
            let (new_claimed_at, new_attempts): (DateTime<Utc>, i32) = sqlx::query_as(
                "UPDATE jobs SET status = 'running', attempts = attempts + 1,
                    locked_at = now(), last_error = 'new claim owns this row', updated_at = now()
                  WHERE id = $1 AND status = 'queued'
                  RETURNING locked_at, attempts",
            )
            .bind(id)
            .fetch_one(&control)
            .await?;
            anyhow::ensure!(new_claimed_at != old_claimed_at);
            anyhow::ensure!(new_attempts == old_state.attempts);
            allow_writeback(&control).await?;
            let next = jobs::enqueue(&control, KIND, json!({"mode": "success"})).await?;
            expected_calls.push(next);
            wait_for_state(&control, next, "done").await?;
            let untouched = state(&control, id).await?;
            anyhow::ensure!(
                untouched.status == "running"
                    && untouched.attempts == 1
                    && untouched.locked_at == Some(new_claimed_at)
                    && untouched.last_error.as_deref() == Some("new claim owns this row"),
                "stale {mode} changed the newer claim: {untouched:?}"
            );
        }

        // 结果正在写回退避时关闭连接池，不能留下仍在重试的任务。
        let closing_job = enqueue_case(&control, "success", 3).await?;
        expected_calls.push(closing_job);
        wait_for_two_rejections(&control).await?;
        let actual_calls = calls.lock().unwrap().clone();
        anyhow::ensure!(
            actual_calls == expected_calls,
            "writeback retries reran a handler: expected {expected_calls:?}, got {actual_calls:?}"
        );
        anyhow::Ok(())
    }
    .await;

    // 断言失败也先关闭池，再停止调度器，让独立写回任务退出，避免污染下一次测试。
    let closing = tokio::spawn({
        let pool = pool.clone();
        async move { pool.close().await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    worker.abort();
    let _ = worker.await;
    tokio::time::timeout(Duration::from_secs(5), closing).await??;
    // 关闭失败或遗留行锁时，清理也必须有期限。
    let cleanup = tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::raw_sql(
            "DROP TRIGGER test_job_writeback_1106_reject ON jobs;
         DROP FUNCTION test_job_writeback_1106_reject();
         DROP SEQUENCE test_job_writeback_1106_rejections;
         DROP TABLE test_job_writeback_1106_gate;
         DELETE FROM jobs WHERE kind = 'test_job_writeback_1106';",
        )
        .execute(&control),
    )
    .await;
    let control_shutdown = tokio::time::timeout(Duration::from_secs(5), control.close()).await;
    result?;
    cleanup??;
    control_shutdown?;
    Ok(())
}
