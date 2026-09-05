//! Database regression for the orchestration shared by KB and admin requeue routes.
use super::requeue_failed;
use sqlx::PgPool;
use utopia_store::jobs::RequeueScope;
use uuid::Uuid;

#[tokio::test]
async fn requeue_api_combines_policies_and_preserves_scope_filters() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb, source, ws) = seed_source(&pool).await?;
    let other_kb = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'other-requeue-kb')",
    )
    .bind(other_kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
    tx.commit().await?;
    utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
    utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("retry")]).await?;
    utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?;
    let rss: i64 = sqlx::query_scalar(
        "SELECT current_job_id FROM rss_full_content_entries WHERE source_id=$1",
    )
    .bind(source)
    .fetch_one(&pool)
    .await?;
    let kind = format!("requeue-test-{org}");
    let generic =
        utopia_store::jobs::enqueue(&pool, &kind, serde_json::json!({"kb_id":kb})).await?;
    let other =
        utopia_store::jobs::enqueue(&pool, &kind, serde_json::json!({"kb_id":other_kb})).await?;
    let ids = [rss, generic, other];
    // A future window isolates the global route policy from other shared-DB tests.
    let since = chrono::DateTime::parse_from_rfc3339("2100-01-01T00:00:00Z")?.to_utc();
    for (scope_kb, scope_kind, window, expected_ids) in [
        (Some(kb), None, since, vec![rss, generic]),
        (None, None, since, vec![rss, generic, other]),
        (Some(kb), Some(kind.as_str()), since, vec![generic]),
        (None, Some(kind.as_str()), since, vec![generic, other]),
        (Some(kb), Some("hydrate_rss_entry"), since, vec![rss]),
        (None, Some("hydrate_rss_entry"), since, vec![rss]),
        (Some(kb), None, since + chrono::Duration::seconds(1), vec![]),
        (None, None, since + chrono::Duration::seconds(1), vec![]),
    ] {
        sqlx::query("UPDATE jobs SET status='failed', attempts=3, updated_at=$2 WHERE id=ANY($1)")
            .bind(ids.as_slice())
            .bind(since)
            .execute(&pool)
            .await?;
        let count = requeue_failed(
            &pool,
            RequeueScope {
                kb_id: scope_kb,
                kind: scope_kind,
                failed_since: Some(window),
            },
        )
        .await?;
        assert_eq!(count, expected_ids.len() as u64);
        let mut queued: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE id=ANY($1) AND status='queued' AND attempts=0 ORDER BY id",
        )
        .bind(ids.as_slice())
        .fetch_all(&pool)
        .await?;
        let mut expected_ids = expected_ids;
        queued.sort_unstable();
        expected_ids.sort_unstable();
        assert_eq!(queued, expected_ids);
    }
    sqlx::query("DELETE FROM jobs WHERE id=ANY($1)")
        .bind(ids.as_slice())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}

async fn seed_source(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid, Uuid)> {
    let org_id = Uuid::now_v7();
    let workspace_id = Uuid::now_v7();
    let kb_id = Uuid::now_v7();
    let source_id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org_id)
        .bind(format!("rss-full-content-test-{org_id}"))
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(org_id)
        .bind("rss-full-content-test")
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb_id)
        .bind(workspace_id)
        .bind("rss-full-content-test")
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO sources (id, kb_id, kind, name, config)
         VALUES ($1, $2, 'rss', $3, '{\"feed_url\":\"https://example.com/feed\",\"content_mode\":\"full_new_items\"}'::jsonb)",
    )
    .bind(source_id)
    .bind(kb_id)
    .bind("rss-full-content-test")
    .execute(pool)
    .await?;
    Ok((org_id, kb_id, source_id, workspace_id))
}

fn entry(key: impl Into<String>) -> utopia_store::rss_full_content::NewEntry {
    utopia_store::rss_full_content::NewEntry {
        external_key: key.into(),
        title: "Test entry".into(),
        article_url: Some("https://example.com/article".into()),
        summary: "A useful summary".into(),
        embedded_html: None,
        doc_time: None,
        has_usable_source: true,
    }
}
