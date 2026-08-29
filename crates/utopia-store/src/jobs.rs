//! 任务队列：Postgres `FOR UPDATE SKIP LOCKED` 消费。
//! worker 与 API 同进程（tokio task），失败按 30s * attempts² 退避重试。
//! 并发消费：调度循环按"运行中 < 目标数"续派，任务在独立 task 执行；
//! 目标数经 AtomicUsize 热读——系统设置里改并发即时生效，无需重启。

use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use utopia_core::AppResult;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

pub async fn enqueue(pool: &PgPool, kind: &str, payload: serde_json::Value) -> AppResult<i64> {
    let (id,): (i64,) =
        sqlx::query_as("INSERT INTO jobs (kind, payload) VALUES ($1, $2) RETURNING id")
            .bind(kind)
            .bind(payload)
            .fetch_one(pool)
            .await?;
    Ok(id)
}

/// 认领一个到期任务；没有则返回 None。
async fn claim_one(pool: &PgPool) -> AppResult<Option<Job>> {
    let job = sqlx::query_as(
        "UPDATE jobs SET status = 'running', locked_at = now(),
                attempts = attempts + 1, updated_at = now()
         WHERE id = (
             SELECT id FROM jobs
             WHERE status = 'queued' AND run_at <= now()
             ORDER BY run_at
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         RETURNING id, kind, payload, attempts, max_attempts",
    )
    .fetch_optional(pool)
    .await?;
    Ok(job)
}

async fn mark_done(pool: &PgPool, id: i64) -> AppResult<()> {
    // **成功要把上一次的错清掉。** 重试成功后 last_error 仍留着失败那次的原文，
    // 于是任务表里出现 status='done' 配着一条错误信息——查问题的人读到的是
    // 一个已经不成立的原因。实测就这么误导过一次：bootstrap 明明跑成了，
    // 表上还挂着 "column relation_type does not exist"。
    sqlx::query(
        "UPDATE jobs SET status = 'done', last_error = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn mark_failed(pool: &PgPool, job: &Job, err: &str) -> AppResult<()> {
    if job.attempts >= job.max_attempts {
        sqlx::query(
            "UPDATE jobs SET status = 'failed', last_error = $2, updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(err)
        .execute(pool)
        .await?;
    } else {
        let backoff_secs = 30i64 * i64::from(job.attempts) * i64::from(job.attempts);
        sqlx::query(
            "UPDATE jobs SET status = 'queued', last_error = $2,
                    run_at = now() + make_interval(secs => $3::float8),
                    updated_at = now()
             WHERE id = $1",
        )
        .bind(job.id)
        .bind(err)
        .bind(backoff_secs as f64)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// worker 调度循环：运行中任务数低于目标并发就继续认领（有活立即续派），
/// 空闲时 2s 轮询；每个任务在独立 tokio task 中执行，长抽取不再阻塞同步。
/// `concurrency` 每轮热读——系统设置里改并发数即时生效。
/// 任务分发逻辑由调用方以 handler 注入（store 不依赖上层 crate）。
pub async fn run_worker<F, Fut>(pool: PgPool, concurrency: Arc<AtomicUsize>, handler: F)
where
    F: Fn(Job) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let running = Arc::new(AtomicUsize::new(0));
    // 孤儿回收：进程被杀时 running 任务无人收尸，文档会永远停在 extracting。
    // 单进程部署下，启动时仍为 running 的必是孤儿——一律重排队（处理器均幂等）。
    match sqlx::query(
        "UPDATE jobs SET status = 'queued', locked_at = NULL, updated_at = now()
         WHERE status = 'running'",
    )
    .execute(&pool)
    .await
    {
        Ok(r) if r.rows_affected() > 0 => {
            tracing::warn!(
                count = r.rows_affected(),
                "回收孤儿任务（上次进程退出时正在运行）"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, "孤儿任务回收失败"),
    }
    tracing::info!(
        concurrency = concurrency.load(Ordering::Relaxed),
        "jobs worker 已启动"
    );
    loop {
        let cap = concurrency.load(Ordering::Relaxed).max(1);
        if running.load(Ordering::Relaxed) >= cap {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        match claim_one(&pool).await {
            Ok(Some(job)) => {
                running.fetch_add(1, Ordering::Relaxed);
                let pool = pool.clone();
                let handler = handler.clone();
                let running = running.clone();
                tokio::spawn(async move {
                    let result = handler(job.clone()).await;
                    let outcome = match result {
                        Ok(()) => mark_done(&pool, job.id).await,
                        Err(e) => {
                            tracing::warn!(job_id = job.id, kind = %job.kind, error = %e, "任务执行失败");
                            mark_failed(&pool, &job, &e.to_string()).await
                        }
                    };
                    if let Err(e) = outcome {
                        tracing::error!(job_id = job.id, error = %e, "任务状态写回失败");
                    }
                    running.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
            Err(e) => {
                tracing::error!(error = %e, "任务认领失败，5s 后重试");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}
