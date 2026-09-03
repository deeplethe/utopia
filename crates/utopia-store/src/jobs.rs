//! 任务队列：Postgres `FOR UPDATE SKIP LOCKED` 消费。
//! worker 与 API 同进程（tokio task），失败按 30s * attempts² 退避重试——
//! 除非处理器把它标成了 `utopia_core::Terminal`，那种一次就到此为止（见 [`retry_delay`]）。
//! 并发消费：调度循环按"运行中 < 目标数"续派，任务在独立 task 执行；
//! 目标数经 AtomicUsize 热读——系统设置里改并发即时生效，无需重启。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use utopia_core::AppResult;
use uuid::Uuid;

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

/// 重排失败任务的范围（#216）。三个条件都可空，空 = 不限。
///
/// **按库圈要解 payload**：任务表没有 kb 列，payload 只带 `document_id` /
/// `source_id` / `kb_id` 三种之一，各自解到库。没有库的系统任务只在不限库时才动
#[derive(Debug, Default, Clone, Copy)]
pub struct RequeueScope<'a> {
    pub kb_id: Option<Uuid>,
    pub kind: Option<&'a str>,
    /// 只排这个时刻之后失败的——告警上的「再跑一遍」圈的正是那次故障窗口
    pub failed_since: Option<DateTime<Utc>>,
}

/// 库范围的 SQL 谓词，`$N` 是库 id；`requeue_failed` 与 `failed_count` 共用
const KB_SCOPE: &str = "(
       (j.payload ? 'kb_id' AND j.payload->>'kb_id' = $KB::text)
    OR (j.payload ? 'document_id' AND EXISTS (
            SELECT 1 FROM documents d
             WHERE d.id::text = j.payload->>'document_id' AND d.kb_id = $KB))
    OR (j.payload ? 'source_id' AND EXISTS (
            SELECT 1 FROM sources s
             WHERE s.id::text = j.payload->>'source_id' AND s.kb_id = $KB)))";

/// 把范围内的 failed 任务放回队列：`attempts` 归零、立即到期。
///
/// 处理器都是幂等的（启动时回收孤儿就靠这一点），所以重排永远安全；
/// 此前 `failed` 是终点，余额耗尽一批文档全失败，充值之后只能逐个点或整源重抽
pub async fn requeue_failed(pool: &PgPool, scope: RequeueScope<'_>) -> AppResult<u64> {
    let sql = format!(
        "UPDATE jobs j
            SET status = 'queued', attempts = 0, run_at = now(), updated_at = now()
          WHERE j.status = 'failed'
            AND ($1::text IS NULL OR j.kind = $1)
            AND ($2::timestamptz IS NULL OR j.updated_at >= $2)
            AND ($3::uuid IS NULL OR {})",
        KB_SCOPE.replace("$KB", "$3")
    );
    let res = sqlx::query(&sql)
        .bind(scope.kind)
        .bind(scope.failed_since)
        .bind(scope.kb_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// 范围内 failed 的条数——设置页那一行「N 个失败任务」
pub async fn failed_count(pool: &PgPool, kb_id: Option<Uuid>) -> AppResult<i64> {
    let sql = format!(
        "SELECT count(*) FROM jobs j
          WHERE j.status = 'failed' AND ($1::uuid IS NULL OR {})",
        KB_SCOPE.replace("$KB", "$1")
    );
    Ok(sqlx::query_scalar(&sql).bind(kb_id).fetch_one(pool).await?)
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

/// 下一次重试等多久；`None` = 到此为止。
///
/// 两个理由到此为止：**次数用完了**，或者**处理器说了这次不会因为重试而变好**
/// （`utopia_core::Terminal`，见 issue #195）。后者以前不存在，于是余额耗尽的
/// 任务照样把三次退避走完——七分钟里余额不会自己长回来，那三次只是把同一句
/// 错误重说三遍，还把运维该看见的「失败」推迟了七分钟。
///
/// 限流相反，它正是这套退避的服务对象：配额会自己恢复。#176 把两类分开就是
/// 为了让它们各走各的路，而重试策略当时没跟上。
///
/// 抽成纯函数是为了能测——决定在这里，写库只是把决定落下去。
fn retry_delay(attempts: i32, max_attempts: i32, terminal: bool) -> Option<i64> {
    if terminal || attempts >= max_attempts {
        return None;
    }
    Some(30i64 * i64::from(attempts) * i64::from(attempts))
}

async fn mark_failed(pool: &PgPool, job: &Job, err: &anyhow::Error) -> AppResult<()> {
    let text = format!("{err:#}");
    let Some(backoff_secs) = retry_delay(
        job.attempts,
        job.max_attempts,
        utopia_core::is_terminal(err),
    ) else {
        sqlx::query(
            "UPDATE jobs SET status = 'failed', last_error = $2, updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(&text)
        .execute(pool)
        .await?;
        return Ok(());
    };
    sqlx::query(
        "UPDATE jobs SET status = 'queued', last_error = $2,
                run_at = now() + make_interval(secs => $3::float8),
                updated_at = now()
         WHERE id = $1",
    )
    .bind(job.id)
    .bind(&text)
    .bind(backoff_secs as f64)
    .execute(pool)
    .await?;
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
                            mark_failed(&pool, &job, &e).await
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

#[cfg(test)]
mod tests {
    use super::retry_delay;

    /// 退避照旧：30s、120s、270s，第三次之后放弃。
    #[test]
    fn an_ordinary_failure_backs_off_and_then_gives_up() {
        assert_eq!(retry_delay(1, 3, false), Some(30));
        assert_eq!(retry_delay(2, 3, false), Some(120));
        assert_eq!(retry_delay(3, 3, false), None);
    }

    /// 标成没救的**第一次就到此为止**——那三次退避加起来是七分钟，
    /// 而余额不会在七分钟里自己长回来（#195）。
    #[test]
    fn a_terminal_failure_does_not_spend_the_budget() {
        assert_eq!(retry_delay(1, 3, true), None);
    }
}
