//! 任务队列：Postgres `FOR UPDATE SKIP LOCKED` 消费。
//! worker 与 API 同进程（tokio task），失败按 30s * attempts² 退避重试——
//! 除非处理器把它标成了 `utopia_core::Terminal`，那种一次就到此为止（见 [`retry_delay`]）。
//! 并发消费：调度循环按"运行中 < 目标数"续派，任务在独立 task 执行；
//! 目标数经 AtomicUsize 热读——系统设置里改并发即时生效，无需重启。

use chrono::{DateTime, Utc};
use sqlx::{postgres::PgListener, PgPool, Postgres, Transaction};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use utopia_core::AppResult;
use uuid::Uuid;

pub const JOB_CHANNEL: &str = "utopia_jobs";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub max_attempts: i32,
}

/// 同种任务、同样载荷已经排着就不再排：一批文档各自抽完都想触发同一个库级任务
/// （类型消解、对账），排十次跑十次是浪费，而且后九次看到的是第一次跑完的库。
/// 返回 None = 已有一条在排队
pub async fn enqueue_unless_queued(
    pool: &PgPool,
    kind: &str,
    payload: serde_json::Value,
) -> AppResult<Option<i64>> {
    let mut tx = pool.begin().await?;
    let row: Option<(i64,)> = sqlx::query_as(
        "INSERT INTO jobs (kind, payload)
         SELECT $1, $2
          WHERE NOT EXISTS (SELECT 1 FROM jobs WHERE kind = $1 AND payload = $2 AND status = 'queued')
         RETURNING id",
    )
    .bind(kind)
    .bind(payload)
    .fetch_optional(&mut *tx)
    .await?;
    if row.is_some() {
        notify_worker_tx(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(row.map(|(id,)| id))
}

pub async fn enqueue(pool: &PgPool, kind: &str, payload: serde_json::Value) -> AppResult<i64> {
    enqueue_with_max_attempts(pool, kind, payload, 3).await
}

/// 入队并显式指定有限的重试预算。调用方不能把一个外部系统的任务变成无限重试。
pub async fn enqueue_with_max_attempts(
    pool: &PgPool,
    kind: &str,
    payload: serde_json::Value,
    max_attempts: i32,
) -> AppResult<i64> {
    if !(1..=32).contains(&max_attempts) {
        return Err(utopia_core::AppError::Validation(
            "max_attempts must be between 1 and 32".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let id = enqueue_with_max_attempts_tx(&mut tx, kind, payload, max_attempts).await?;
    tx.commit().await?;
    Ok(id)
}

/// 与 ledger 变更共用事务；不能先写 job、再写 entry，否则进程退出窗口会留下
/// 一个无法从 entry 找到的 hydration 任务。
pub async fn enqueue_with_max_attempts_tx(
    tx: &mut Transaction<'_, Postgres>,
    kind: &str,
    payload: serde_json::Value,
    max_attempts: i32,
) -> AppResult<i64> {
    if !(1..=32).contains(&max_attempts) {
        return Err(utopia_core::AppError::Validation(
            "max_attempts must be between 1 and 32".into(),
        ));
    }
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload, max_attempts) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(kind)
    .bind(payload)
    .bind(max_attempts)
    .fetch_one(&mut **tx)
    .await?;
    notify_worker_tx(tx).await?;
    Ok(id)
}

/// Wake idle workers only after the transaction containing the job is committed.
/// PostgreSQL delivers `NOTIFY` at commit, so a worker can never wake up before
/// the row it needs to claim is visible.
pub(crate) async fn notify_worker_tx(tx: &mut Transaction<'_, Postgres>) -> AppResult<()> {
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(JOB_CHANNEL)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 空闲轮询间隔。有通知时用不到它；它兜的是 `run_at` 在未来的重试、
/// 重连期间丢掉的通知，以及几个 worker 被同一条通知叫醒后没抢到的那些（#517）。
pub const IDLE_POLL: Duration = Duration::from_secs(2);

/// 一次空闲等待是怎么结束的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// 有人入队，通知到了
    Notified,
    /// 等满一个轮询间隔，什么也没来
    Polled,
}

/// 建立唤醒监听。`None` = 建不起来，worker 只靠轮询，行为与没有通知时一样。
///
/// 这条连接从池里取出后**一直握着**：LISTEN 是连接级状态，还回去就没了。
/// 池上限（`db.rs`）从此少一条给别人用；jobs 只起一个 worker，所以只少这一条。
pub async fn listen_for_jobs(pool: &PgPool) -> Option<PgListener> {
    let mut listener = match PgListener::connect_with(pool).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!(error = %e, "jobs worker 建立通知监听失败，退回轮询");
            return None;
        }
    };
    if let Err(e) = listener.listen(JOB_CHANNEL).await {
        tracing::error!(error = %e, "jobs worker 监听通知失败，退回轮询");
        return None;
    }
    Some(listener)
}

/// 空闲等待：通知先到就先醒，否则等满 `poll`。
///
/// 监听出错不能立刻再认领：库能认领而监听连接重连不上（池满就是一种）的话，
/// 循环会变成「认领一次、警告一行」的紧循环。出错就按轮询的节奏睡一觉，
/// 下一轮 `recv` 自己会重连。
pub async fn wait_for_work(listener: Option<&mut PgListener>, poll: Duration) -> Wake {
    let Some(listener) = listener else {
        tokio::time::sleep(poll).await;
        return Wake::Polled;
    };
    match tokio::time::timeout(poll, listener.recv()).await {
        Ok(Ok(_)) => Wake::Notified,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "jobs worker 通知监听失败，继续轮询");
            tokio::time::sleep(poll).await;
            Wake::Polled
        }
        Err(_) => Wake::Polled,
    }
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
/// RSS hydration is excluded: its source admission policy owns retries.
/// 其他处理器都是幂等的（启动时回收孤儿就靠这一点）；
/// 此前 `failed` 是终点，余额耗尽一批文档全失败，充值之后只能逐个点或整源重抽
pub async fn requeue_failed(pool: &PgPool, scope: RequeueScope<'_>) -> AppResult<u64> {
    let mut tx = pool.begin().await?;
    let sql = format!(
        "UPDATE jobs j
            SET status = 'queued', attempts = 0, run_at = now(), updated_at = now()
          WHERE j.status = 'failed' AND j.kind <> 'hydrate_rss_entry'
            AND ($1::text IS NULL OR j.kind = $1)
            AND ($2::timestamptz IS NULL OR j.updated_at >= $2)
            AND ($3::uuid IS NULL OR {})",
        KB_SCOPE.replace("$KB", "$3")
    );
    let res = sqlx::query(&sql)
        .bind(scope.kind)
        .bind(scope.failed_since)
        .bind(scope.kb_id)
        .execute(&mut *tx)
        .await?;
    let count = res.rows_affected();
    if count > 0 {
        notify_worker_tx(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(count)
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

/// 处理器挂上 `Deferred { retry_in }` 时下一次重试的等待秒数。**与失败次数无关**——
/// 这是 `Deferred` 跟默认退避的关键区别：默认的 `30s × attempts²` 是「再试一次
/// 也许能好」的递增，而 `Deferred` 是「现在条件不满足，`retry_in` 之后再试」，
/// 与第几次没有关系。同一个等待条件挂回队列两次，两次都得到同一个 `run_at`
/// 偏移，没有 60s、120s 的递增（#526）。
///
/// 一条任务最多连续等多久（见 `mark_failed`）。一小时够一个大本体在远端嵌入模型上
/// 补齐；过了还没好，多半是补齐任务自己在失败，该让这条抽取按失败处理、被人看见
pub const DEFER_WINDOW_SECS: i64 = 60 * 60;

fn deferred_retry_secs(retry_in: std::time::Duration) -> i64 {
    // 截断到秒：底层 `run_at` 是 timestamptz，亚秒精度存不住，而几十毫秒也不值得
    // 一行浮点换算。向上取整——少等一秒比早跑一秒好，前者无害，后者会把还在跑的
    // embedding job 撞回锁上。
    let secs = retry_in.as_secs_f64().ceil();
    if !secs.is_finite() || secs < 1.0 {
        1
    } else {
        secs as i64
    }
}

/// `pub` 给集成测试用——主流程仍然由 `run_worker` 内的私有 caller 调用，
/// 不会从这里出。`#[doc(hidden)]` 是因为它属于内部契约，不进公开 API。
#[doc(hidden)]
pub async fn mark_failed(pool: &PgPool, job: &Job, err: &anyhow::Error) -> AppResult<()> {
    let text = format!("{err:#}");
    // `Terminal` 优先：处理器最后改主意说「这次不算了」就该走 `failed` 路径，
    // 不该被 `Deferred` 覆盖。两个都挂时由调用方决定——`is_terminal` 写在前面。
    if utopia_core::is_terminal(err) {
        let res = sqlx::query(
            "UPDATE jobs SET status = 'failed', last_error = $2, updated_at = now() WHERE id = $1",
        )
        .bind(job.id)
        .bind(&text)
        .execute(pool)
        .await?;
        let _ = res.rows_affected();
        return Ok(());
    }
    // `Deferred`（#526）：把任务挂回 `queued`，把 `attempts` 退回去，不烧预算。
    // 第一次走到这里时 `claim_one` 已经把 `attempts` 加 1，写回时要 -1，
    // 否则同一次等待会让 `attempts` 慢慢爬到 `max_attempts`，最后那条
    // `failed` 是我们最不想看见的——ontology 还差一秒就绪，文档却先死了。
    //
    // **等也有期限。** 从第一次挂回去算起（记在 payload 的 `deferred_since`，不用
    // `created_at`：一批上传排队几小时是常态，那不算在等）超过 [`DEFER_WINDOW_SECS`]
    // 还在等，就不再挂回去，落到下面的普通退避、烧预算。等的那件事（比如
    // `embed_ontology`）自己一直失败时，不设期限这条任务会每 30 秒醒一次、永远排着，
    // 却没有一次被记成失败
    if let Some(retry_in) = utopia_core::is_deferred(err) {
        let secs = deferred_retry_secs(retry_in);
        let res = sqlx::query(
            "UPDATE jobs SET status = 'queued', last_error = $2,
                    attempts = GREATEST(0, attempts - 1),
                    run_at = now() + make_interval(secs => $3::float8),
                    payload = payload || jsonb_build_object('deferred_since',
                        COALESCE(payload->>'deferred_since', now()::text)),
                    updated_at = now()
             WHERE id = $1
               AND COALESCE((payload->>'deferred_since')::timestamptz, now())
                   > now() - make_interval(secs => $4::float8)",
        )
        .bind(job.id)
        .bind(&text)
        .bind(secs as f64)
        .bind(DEFER_WINDOW_SECS as f64)
        .execute(pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(());
        }
    }
    let Some(backoff_secs) = retry_delay(job.attempts, job.max_attempts, false) else {
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
/// 空闲时等入队通知、2s 轮询兜底（#517）；每个任务在独立 tokio task 中执行，长抽取不再阻塞同步。
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
    let mut listener = listen_for_jobs(&pool).await;
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
                    // **处理器 panic 也要收尸。** 直接 `handler(job).await` 的话，
                    // panic 会把这个 spawn 出来的 future 一起掀掉：`mark_failed`
                    // 不会跑（任务行永远停在 running，无错误无重试），`running`
                    // 也不会减（每 panic 一次就永久少一个并发名额，攒够 cap 之后
                    // 整个队列不再认领任何任务）。套一层 spawn，panic 变成
                    // JoinError 拿回来，两件事就都还在。
                    let inner = {
                        let (handler, job) = (handler.clone(), job.clone());
                        tokio::spawn(async move { handler(job).await })
                    };
                    let result = match inner.await {
                        Ok(r) => r,
                        Err(join) => {
                            Err(anyhow::anyhow!("任务处理器 panic（详情见 stderr）：{join}"))
                        }
                    };
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
            Ok(None) => {
                wait_for_work(listener.as_mut(), IDLE_POLL).await;
            }
            Err(e) => {
                tracing::error!(error = %e, "任务认领失败，5s 后重试");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{deferred_retry_secs, retry_delay};
    use std::time::Duration;

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

    /// `Deferred` 不随失败次数递增——同一等待条件两次排队得到的 `run_at`
    /// 偏移相同，不会出现 60s、120s 的递增（#526）。
    #[test]
    fn deferred_retry_is_independent_of_attempts() {
        assert_eq!(
            deferred_retry_secs(Duration::from_secs(30)),
            deferred_retry_secs(Duration::from_secs(30))
        );
        // 截断到秒，向上取整：29.5s → 30s
        assert_eq!(deferred_retry_secs(Duration::from_millis(29_500)), 30);
        // 0/负数/NaN 都给 1s 下界——至少等一秒，比立刻重试的轮询间隔还短就是浪费
        assert_eq!(deferred_retry_secs(Duration::ZERO), 1);
        assert_eq!(deferred_retry_secs(Duration::from_nanos(500)), 1);
    }
}
