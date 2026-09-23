//! 文库的抽取进度：每篇文档只数一次（#715）。
//!
//! 进度条从前拿 `ready`（摄入完成）当分子、`ready + extracting` 当分母。一篇「摄入完了、
//! 图谱还在抽」的文档在两个维度里各算一次：库里只有一篇正在抽的文档，也显示成 1 / 2。
//! 现在分子分母都只看 `graph_status`：`done` 与 `extracting`。删掉的文档不算，
//! 抽取失败的单独数。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::documents;
use uuid::Uuid;

async fn document(
    pool: &PgPool,
    kb: Uuid,
    name: &str,
    graph_status: &str,
    deleted: bool,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status, graph_status, deleted_at)
         VALUES ($1, $2, $3, $3, 'ready', $4, CASE WHEN $5 THEN now() END)",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(name)
    .bind(graph_status)
    .bind(deleted)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_document_still_extracting_is_not_counted_as_done() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'progress-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'progress-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'progress-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        document(&pool, kb, "done.md", "done", false).await?;
        document(&pool, kb, "extracting.md", "extracting", false).await?;
        document(&pool, kb, "queued.md", "queued", false).await?;
        document(&pool, kb, "failed.md", "failed", false).await?;
        // 删掉的文档哪一格都不算，包括它删之前正在抽的
        document(&pool, kb, "deleted.md", "extracting", true).await?;
        let page = documents::page(&pool, kb, None, None, None, false, 50, 0).await?;
        // 摄入完成的四篇都是 ready——重抽的作用范围不变
        assert_eq!(page.ready, 4);
        assert_eq!(page.done, 1);
        assert_eq!(page.extracting, 2, "queued 与 extracting 都算在抽");
        assert_eq!(page.failed, 1);
        // 进度条因此是 done / (done + extracting) = 1 / 3。旧的写法 ready / (ready + extracting)
        // 会是 4 / 6：两篇还没抽完的、一篇抽失败的都被算成了完成
        anyhow::Ok(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await;
    run
}
