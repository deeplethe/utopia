//! #216：失败的任务能回到队列。
//!
//! 此前 `failed` 就是终点——余额耗尽一批文档全失败，充值之后只能逐个点，或整源重抽
//! （已成功的也重跑一遍模型）。`jobs::requeue_failed` 按范围重排：库、种类、失败时间
//! 三个条件都可空。这里守三件事：
//!
//! 1. **按库圈得准。** 任务的 payload 只带 `document_id` / `source_id` / `kb_id` 三种
//!    之一，按库重排要解到库——别的库的一条都不碰，没有库的（系统任务）只有不限库时才动。
//! 2. **只动 failed 的**，`done` 与 `queued` 不动；重排后 `attempts` 归零、立即到期。
//! 3. **种类与时间窗**：`kind` 只排那一种；`failed_since` 只排那之后失败的——
//!    告警上的「再跑一遍」圈的正是那次故障窗口里的任务。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{Duration, Utc};
use sqlx::PgPool;
use utopia_store::jobs::{self, RequeueScope};
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb1: Uuid,
    kb2: Uuid,
    doc1: Uuid,
    src2: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb1, kb2) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (src1, doc1, src2) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'requeue-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'requeue-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for kb in [kb1, kb2] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'requeue-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    }
    sqlx::query("INSERT INTO sources (id, kb_id, kind, name) VALUES ($1, $2, 'folder', 'f')")
        .bind(src1)
        .bind(kb1)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, sha256, status)
         VALUES ($1, $2, $3, 'a.md', 'requeue', 'ready')",
    )
    .bind(doc1)
    .bind(kb1)
    .bind(src1)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO sources (id, kb_id, kind, name) VALUES ($1, $2, 'url', 'u')")
        .bind(src2)
        .bind(kb2)
        .execute(pool)
        .await?;
    Ok(Fx {
        org,
        kb1,
        kb2,
        doc1,
        src2,
    })
}

async fn job(
    pool: &PgPool,
    kind: &str,
    payload: serde_json::Value,
    status: &str,
    failed_ago: Duration,
) -> anyhow::Result<i64> {
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload, status, attempts, last_error, updated_at)
         VALUES ($1, $2, $3, 3, 'out of credit', $4) RETURNING id",
    )
    .bind(kind)
    .bind(payload)
    .bind(status)
    .bind(Utc::now() - failed_ago)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

async fn state(pool: &PgPool, id: i64) -> anyhow::Result<(String, i32)> {
    Ok(
        sqlx::query_as("SELECT status, attempts FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
async fn a_failed_job_finds_its_way_back() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let mut ids: Vec<i64> = Vec::new();

    let run = async {
        let m = Duration::minutes(1);
        // kb1：文档任务、库级任务、一条很早就失败的、一条已经成功的
        let d1 = job(
            &pool,
            "process_document",
            serde_json::json!({ "document_id": f.doc1 }),
            "failed",
            m,
        )
        .await?;
        let b1 = job(
            &pool,
            "bootstrap_ontology",
            serde_json::json!({ "kb_id": f.kb1 }),
            "failed",
            m,
        )
        .await?;
        let old1 = job(
            &pool,
            "process_document",
            serde_json::json!({ "document_id": f.doc1 }),
            "failed",
            Duration::days(3),
        )
        .await?;
        let done1 = job(
            &pool,
            "process_document",
            serde_json::json!({ "document_id": f.doc1 }),
            "done",
            m,
        )
        .await?;
        // kb2：来源任务
        let s2 = job(
            &pool,
            "sync_source",
            serde_json::json!({ "source_id": f.src2 }),
            "failed",
            m,
        )
        .await?;
        // 没有库的系统任务
        let sys = job(&pool, "noop", serde_json::json!({}), "failed", m).await?;
        ids.extend([d1, b1, old1, done1, s2, sys]);

        assert_eq!(jobs::failed_count(&pool, Some(f.kb1)).await?, 3);
        assert_eq!(jobs::failed_count(&pool, Some(f.kb2)).await?, 1);

        // 3. 时间窗：只排最近一小时里失败的 kb1 任务
        let n = jobs::requeue_failed(
            &pool,
            RequeueScope {
                kb_id: Some(f.kb1),
                kind: None,
                failed_since: Some(Utc::now() - Duration::hours(1)),
            },
        )
        .await?;
        assert_eq!(
            n, 2,
            "the document job and the base-level job, not the old one"
        );
        assert_eq!(state(&pool, d1).await?, ("queued".into(), 0));
        assert_eq!(state(&pool, b1).await?, ("queued".into(), 0));
        assert_eq!(state(&pool, old1).await?.0, "failed");
        assert_eq!(state(&pool, done1).await?.0, "done", "done stays done");
        assert_eq!(
            state(&pool, s2).await?.0,
            "failed",
            "another base is untouched"
        );
        assert_eq!(
            state(&pool, sys).await?.0,
            "failed",
            "a system job has no base"
        );

        // 3. 种类：kb1 里只排 process_document，老的那条这次回来
        let n = jobs::requeue_failed(
            &pool,
            RequeueScope {
                kb_id: Some(f.kb1),
                kind: Some("process_document"),
                failed_since: None,
            },
        )
        .await?;
        assert_eq!(n, 1);
        assert_eq!(state(&pool, old1).await?, ("queued".into(), 0));
        assert_eq!(jobs::failed_count(&pool, Some(f.kb1)).await?, 0);

        // 1. 按库：kb2 的来源任务
        let n = jobs::requeue_failed(
            &pool,
            RequeueScope {
                kb_id: Some(f.kb2),
                kind: None,
                failed_since: None,
            },
        )
        .await?;
        assert_eq!(n, 1);
        assert_eq!(state(&pool, s2).await?, ("queued".into(), 0));

        // 不限库才动系统任务
        let before = jobs::failed_count(&pool, None).await?;
        let n = jobs::requeue_failed(
            &pool,
            RequeueScope {
                kb_id: None,
                kind: Some("noop"),
                failed_since: Some(Utc::now() - Duration::hours(1)),
            },
        )
        .await?;
        assert!(n >= 1);
        assert_eq!(state(&pool, sys).await?.0, "queued");
        assert!(jobs::failed_count(&pool, None).await? < before);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 自拆：任务表不随组织级联，手动清
    let _ = sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
