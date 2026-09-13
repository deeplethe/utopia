//! `#526` 的 `Deferred` 标记走通的端到端验证。
//!
//! 仅测两个边界：
//!
//! 1. 同一个等待条件两次走 `Deferred`，`attempts` **不会**爬到 `max_attempts`——
//!    它每次被退回去，**预算不烧**。这是「让它等本体向量补齐」与「让它重试
//!    失败调用」之间的区别：前者是同一个时钟；后者应该退避递增。
//! 2. `run_at` 真的被推到未来——而不是「立刻再试一次」，那会跟正在跑的
//!    `embed_ontology` 任务抢同一把库级锁。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败（见 `utopia_store::test_db`）。

use std::time::Duration;

use anyhow::anyhow;
use sqlx::PgPool;
use utopia_core::Deferred;
use utopia_store::jobs;

#[tokio::test]
async fn a_deferred_job_does_not_spend_its_budget() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;

    // 直接插一行「正在跑」的任务——attempts 已经由 `claim_one` 加过一次，
    // 这一关的语义是 `mark_failed(Deferred)` 写回时把它退回去。
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload, status, attempts, max_attempts)
         VALUES ('extract_document', '{\"document_id\":\"00000000-0000-0000-0000-000000000000\"}',
                 'running', 2, 3)
         RETURNING id",
    )
    .fetch_one(&pool)
    .await?;

    // 第一次等待：本体向量补齐，30s。
    let err = anyhow!("waiting on ontology index").context(Deferred::new(Duration::from_secs(30)));
    let job = jobs::Job {
        id,
        kind: "extract_document".into(),
        payload: serde_json::json!({}),
        attempts: 2,
        max_attempts: 3,
    };
    jobs::mark_failed(&pool, &job, &err).await?;

    let (status, attempts, last_error): (String, i32, String) =
        sqlx::query_as("SELECT status, attempts, last_error FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status, "queued", "Deferred 应该让任务挂回 queued");
    assert_eq!(attempts, 1, "attempts 应该被退回去：原 2 减 1 等于 1");
    assert!(
        last_error.contains("waiting on ontology index"),
        "last_error 应当是让运维一眼知道在等什么的措辞；实际是：{last_error:?}"
    );

    // `run_at` 落在未来——不是「立刻又来抢锁」。
    let run_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT run_at FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    let now = chrono::Utc::now();
    let ahead = (run_at - now).num_seconds();
    assert!(
        (25..=35).contains(&ahead),
        "run_at 应该比 now 晚约 30s；实际 {ahead}s"
    );

    // 第二次等待：同一个任务、同一段等待。**预算不烧**——
    // 这是与「限流重试」的关键区别。
    let err2 = anyhow!("waiting on ontology index").context(Deferred::new(Duration::from_secs(30)));
    let job2 = jobs::Job {
        attempts: 1, // 第一次之后的状态
        ..job.clone()
    };
    jobs::mark_failed(&pool, &job2, &err2).await?;
    let (status2, attempts2, _): (String, i32, String) =
        sqlx::query_as("SELECT status, attempts, last_error FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status2, "queued");
    assert_eq!(attempts2, 0, "两次 Deferred 之后 attempts 应该退到 0");

    // 收尾：避免污染下一次跑（其它测试共享同一个库）
    sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    Ok(())
}

/// `Terminal` 优先于 `Deferred`：挂两个 marker 时，结果由 `Terminal` 决定。
/// 这是 `mark_failed` 里 `is_terminal` 写在前面、`is_deferred` 写在后面的语义保证。
#[tokio::test]
async fn terminal_wins_over_deferred_when_both_attached() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;

    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload, status, attempts, max_attempts)
         VALUES ('extract_document', '{\"document_id\":\"00000000-0000-0000-0000-000000000000\"}',
                 'running', 1, 3)
         RETURNING id",
    )
    .fetch_one(&pool)
    .await?;

    // 先挂 Deferred，再挂 Terminal。Terminal 是「最后一次改主意」的那一个。
    let err = anyhow!("balance gone after waiting").context(Deferred::new(Duration::from_secs(30)));
    let err = err.context(utopia_core::Terminal);
    let job = jobs::Job {
        id,
        kind: "extract_document".into(),
        payload: serde_json::json!({}),
        attempts: 1,
        max_attempts: 3,
    };
    jobs::mark_failed(&pool, &job, &err).await?;

    let (status, attempts): (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status, "failed", "Terminal 应该让任务直接进 failed");
    // attempts 不会被退回去——`failed` 走的是另一条 SQL 路径。
    assert_eq!(attempts, 1);

    sqlx::query("DELETE FROM jobs WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    Ok(())
}

/// 等也有期限：从第一次挂回去算起超过 `DEFER_WINDOW_SECS` 还在等，就按普通失败
/// 退避、烧预算。等的那件事自己一直失败时，任务不该每 30 秒醒一次、永远排着
#[tokio::test]
async fn a_wait_past_its_window_is_a_failure() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;

    // 第一次挂回去时记下开始等的时刻
    let (fresh,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload, status, attempts, max_attempts)
         VALUES ('extract_document', '{}', 'running', 1, 3) RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    let err = anyhow!("waiting on ontology index").context(Deferred::new(Duration::from_secs(30)));
    let job = jobs::Job {
        id: fresh,
        kind: "extract_document".into(),
        payload: serde_json::json!({}),
        attempts: 1,
        max_attempts: 3,
    };
    jobs::mark_failed(&pool, &job, &err).await?;
    let since: Option<String> =
        sqlx::query_scalar("SELECT payload->>'deferred_since' FROM jobs WHERE id = $1")
            .bind(fresh)
            .fetch_one(&pool)
            .await?;
    assert!(since.is_some(), "第一次等待应当记下 deferred_since");

    // 已经等过了期限：不再退回 attempts，走普通退避
    let (stale,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload, status, attempts, max_attempts)
         VALUES ('extract_document',
                 jsonb_build_object('deferred_since', (now() - make_interval(secs => $1::float8))::text),
                 'running', 1, 3)
         RETURNING id",
    )
    .bind((jobs::DEFER_WINDOW_SECS + 60) as f64)
    .fetch_one(&pool)
    .await?;
    let job = jobs::Job { id: stale, ..job };
    jobs::mark_failed(&pool, &job, &err).await?;
    let (status, attempts): (String, i32) =
        sqlx::query_as("SELECT status, attempts FROM jobs WHERE id = $1")
            .bind(stale)
            .fetch_one(&pool)
            .await?;
    assert_eq!(status, "queued", "还有预算，按退避重排");
    assert_eq!(attempts, 1, "过了期限的等待要烧预算，attempts 不再退回");

    sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
        .bind(vec![fresh, stale])
        .execute(&pool)
        .await?;
    Ok(())
}
