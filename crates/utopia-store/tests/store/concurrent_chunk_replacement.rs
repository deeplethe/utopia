use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;
use utopia_ingest::{ChunkPiece, Provenance};
use uuid::Uuid;

fn pieces() -> Vec<ChunkPiece> {
    vec![ChunkPiece {
        seq: 0,
        text: "Revenue was 100.".into(),
        char_start: 0,
        char_end: 16,
        heading: None,
        provenance: Provenance::stated(),
    }]
}

#[tokio::test]
async fn overlapping_reprocessing_keeps_one_live_copy_of_each_chunk() -> anyhow::Result<()> {
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
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'chunk-race')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'chunk-race')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'chunk-race')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO documents (id, kb_id, filename, sha256, status) VALUES ($1, $2, 'report.txt', $1::text, 'pending')").bind(doc).bind(kb).execute(&pool).await?;

    let result = async {
        let mut gate = pool.begin().await?;
        sqlx::query("SELECT id FROM documents WHERE id = $1 FOR UPDATE")
            .bind(doc).execute(&mut *gate).await?;
        // Separate one-connection pools let us observe both writers at a real DB
        // lock, rather than assuming that a sleep has produced the interleaving.
        let mut tasks = Vec::new();
        let mut pids = Vec::new();
        for _ in 0..2 {
            let writer = PgPoolOptions::new().max_connections(1).connect(&url).await?;
            pids.push(sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()").fetch_one(&writer).await?);
            tasks.push(tokio::spawn(async move {
                utopia_store::documents::replace_chunks(&writer, kb, doc, &pieces()).await
            }));
        }
        let waiting = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let count: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE pid = ANY($1) AND wait_event_type = 'Lock'")
                    .bind(&pids).fetch_one(&pool).await?;
                if count == 2 { break Ok::<_, sqlx::Error>(()); }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        // Before the fix both have already read an empty chunk set and are
        // blocked by the insert's FK check. With the fix they wait before reading.
        gate.commit().await?;
        let mut returned = Vec::new();
        for task in tasks { returned.push(task.await??); }
        waiting??;
        let live: i64 = sqlx::query_scalar("SELECT count(*) FROM chunks WHERE document_id = $1 AND superseded_at IS NULL")
            .bind(doc).fetch_one(&pool).await?;
        Ok::<_, anyhow::Error>((live, returned))
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    let (live, returned) = result?;
    assert_eq!(
        live, 1,
        "overlapping processing must not duplicate the document's live text"
    );
    assert_eq!(
        returned[0], returned[1],
        "both runs must return the adopted chunk id for indexing"
    );
    Ok(())
}
