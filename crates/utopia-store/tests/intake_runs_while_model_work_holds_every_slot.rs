//! #1300、#1351：常规名额全被调模型的任务占着时，同步和人判定后的物化照样有名额跑。
//!
//! 起真实的 `run_worker`，它会认领库里任何排队任务，所以自成一个二进制（见
//! `tests/store/main.rs`）。处理器只认带本测试标记的任务，别人的任务一概报错，不替人干活。

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;
use utopia_store::jobs;

#[tokio::test]
async fn intake_runs_while_model_work_holds_every_slot() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let mark = uuid::Uuid::now_v7().to_string();
    let (started, mut extracting) = tokio::sync::mpsc::unbounded_channel();
    let (finished, mut ran) = tokio::sync::mpsc::unbounded_channel();
    let ours = mark.clone();
    let worker = tokio::spawn(jobs::run_worker(
        pool.clone(),
        Arc::new(AtomicUsize::new(1)),
        move |job| {
            let (ours, started, finished) = (ours.clone(), started.clone(), finished.clone());
            async move {
                anyhow::ensure!(
                    job.payload["lane_test"] == ours.as_str(),
                    "not this test's job"
                );
                if job.kind == "extract_document" {
                    started.send(())?;
                    // 一直占着唯一的常规名额，像一次在等模型许可的抽取
                    std::future::pending::<()>().await;
                }
                finished.send(job.kind)?;
                Ok(())
            }
        },
    ));
    let queue = |kind: &'static str| {
        // 放在一百年前，排在同优先级里库里别的一切前面
        sqlx::query("INSERT INTO jobs (kind, payload, run_at) VALUES ($1, $2, now() - interval '100 years')")
            .bind(kind)
            .bind(serde_json::json!({ "lane_test": mark }))
            .execute(&pool)
    };
    let result = async {
        queue("extract_document").await?;
        tokio::time::timeout(Duration::from_secs(10), extracting.recv()).await?;
        for kind in [
            "sync_source",
            utopia_store::phrase_bindings::MATERIALIZE_KIND,
        ] {
            queue(kind).await?;
            let claimed = tokio::time::timeout(Duration::from_secs(10), ran.recv()).await;
            anyhow::ensure!(
                matches!(&claimed, Ok(Some(claimed_kind)) if claimed_kind == kind),
                "{kind} waited behind the extraction: {claimed:?}"
            );
        }
        anyhow::Ok(())
    }
    .await;
    worker.abort();
    let _ = worker.await;
    sqlx::query("DELETE FROM jobs WHERE payload->>'lane_test' = $1")
        .bind(&mark)
        .execute(&pool)
        .await?;
    result
}
