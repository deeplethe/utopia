//! #1304：到期的库在它的推理任务还排着时只排一份。
//!
//! `queue_due_inferences` 扫的是全库到期的库，同时跑的测试刚建的库也会被它排上任务，所以
//! 这个测试自成一个二进制（为什么见 `store/main.rs`），收尾删掉它排下的**每一份**
//! `materialize_inferences`，不只是自己那个库的。

use sqlx::PgPool;
use utopia_store::reasoning::queue_due_inferences;
use uuid::Uuid;

#[tokio::test]
async fn a_due_base_is_queued_once_while_its_job_waits() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','inference-schedule');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','inference-schedule');
         INSERT INTO knowledge_bases(id,workspace_id,name,materialize_inferences)
              VALUES ('{kb}','{ws}','inference-schedule',true);"
    ))
    .execute(&pool)
    .await?;
    let before: i64 = sqlx::query_scalar("SELECT coalesce(max(id), 0) FROM jobs")
        .fetch_one(&pool)
        .await?;

    // 两拍之间没人认领：从没推过的库两拍都算到期
    queue_due_inferences(&pool).await?;
    queue_due_inferences(&pool).await?;
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs
          WHERE kind = 'materialize_inferences' AND status = 'queued' AND payload->>'kb_id' = $1",
    )
    .bind(kb.to_string())
    .fetch_one(&pool)
    .await?;

    sqlx::query("DELETE FROM jobs WHERE kind = 'materialize_inferences' AND id > $1")
        .bind(before)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    assert_eq!(queued, 1);
    Ok(())
}
