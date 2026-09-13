//! 提示词里附的「文件开头」是序号最小的现存分块，不是还没抽的第一块。
//!
//! 抽取只取 `extracted_at IS NULL` 的分块。从前开头取的是那张清单的第一条：改过的文件
//! 重抽第 3、7 块时，第 7 块把第 3 块当成开头；失败重试时，后面的块把失败的那块当成开头。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn the_opening_is_the_first_live_chunk_whether_or_not_it_was_extracted() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, doc) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'opening-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'opening-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'opening-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1, $2, 'a.html', 'a')",
        )
        .bind(doc)
        .bind(kb)
        .execute(&pool)
        .await?;
        let superseded = Uuid::now_v7();
        let first = Uuid::now_v7();
        // 第 0 块是上一版留下、已被取代的；第 1 块抽过了；第 3、7 块等着抽
        for (id, seq, extracted, gone) in [
            (superseded, 0, true, true),
            (first, 1, true, false),
            (Uuid::now_v7(), 3, false, false),
            (Uuid::now_v7(), 7, false, false),
        ] {
            sqlx::query(
                "INSERT INTO chunks (id, kb_id, document_id, seq, text, extracted_at, superseded_at)
                 VALUES ($1, $2, $3, $4, $5,
                         CASE WHEN $6 THEN now() END, CASE WHEN $7 THEN now() END)",
            )
            .bind(id)
            .bind(kb)
            .bind(doc)
            .bind(seq)
            .bind(format!("chunk {seq}"))
            .bind(extracted)
            .bind(gone)
            .execute(&pool)
            .await?;
        }
        let pending = utopia_store::documents::chunks_for_extraction(&pool, doc).await?;
        assert_eq!(
            pending.first().map(|c| c.seq),
            Some(3),
            "等着抽的第一块是第 3 块"
        );
        let opening = utopia_store::documents::opening_chunk(&pool, doc).await?;
        assert_eq!(
            opening.map(|(id, _)| id),
            Some(first),
            "开头是现存的第 1 块，抽没抽过都一样"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(ws)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
