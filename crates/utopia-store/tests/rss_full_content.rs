use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn failed_requeue_shares_rss_admission_capacity() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb, source, _) = seed_source(&pool).await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
    tx.commit().await?;
    utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
    let entries: Vec<_> = (0..40).map(|i| entry(format!("capacity-{i}"))).collect();
    utopia_store::rss_full_content::discover(&pool, source, 1, &entries).await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        25
    );
    sqlx::query("UPDATE jobs SET status = 'failed' WHERE payload->>'source_id' = $1")
        .bind(source.to_string())
        .execute(&pool)
        .await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        15
    );
    let scope = utopia_store::jobs::RequeueScope {
        kb_id: Some(kb),
        ..Default::default()
    };
    assert_eq!(
        utopia_store::jobs::requeue_failed(&pool, scope).await?,
        0,
        "generic retries must not bypass RSS admission"
    );
    let (a, b) = tokio::join!(
        utopia_store::rss_full_content::requeue_failed(&pool, scope),
        utopia_store::rss_full_content::requeue_failed(&pool, scope)
    );
    let requeued = a? + b?;
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'source_id' = $1 AND status IN ('queued', 'running')")
        .bind(source.to_string()).fetch_one(&pool).await?;
    cleanup(&pool, org).await?;
    assert_eq!(live, 25, "RSS requeue exceeded the admission cap");
    assert_eq!(requeued, 10, "requeue result must count only admitted jobs");
    Ok(())
}

#[tokio::test]
async fn pre_fetch_snapshot_rejects_changed_config_and_stale_generation() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, _, source, _) = seed_source(&pool).await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
    tx.commit().await?;
    let old_config =
        serde_json::json!({"feed_url":"https://example.com/feed", "content_mode":"full_new_items"});
    let (snapshot, _) =
        utopia_store::rss_full_content::begin_feed_observation(&pool, source, &old_config).await?;
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE sources SET config = jsonb_set(config, '{feed_url}', '\"https://example.com/new\"') WHERE id = $1")
        .bind(source).execute(&mut *tx).await?;
    utopia_store::rss_full_content::enable_source(&mut tx, source).await?;
    tx.commit().await?;
    assert!(
        utopia_store::rss_full_content::begin_feed_observation(&pool, source, &old_config)
            .await
            .is_err()
    );
    assert!(utopia_store::rss_full_content::record_baseline(
        &pool,
        source,
        snapshot.activation_generation,
        &[]
    )
    .await
    .is_err());
    let activation = utopia_store::rss_full_content::get_activation(&pool, source)
        .await?
        .unwrap();
    assert_eq!(activation.activation_state, "pending");
    cleanup(&pool, org).await?;
    Ok(())
}

#[tokio::test]
async fn delayed_observation_cannot_cross_purge_and_diagnostics_hide_job_errors(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb, source, _) = seed_source(&pool).await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
    tx.commit().await?;
    utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
    let config =
        serde_json::json!({"feed_url":"https://example.com/feed","content_mode":"full_new_items"});
    let (_, before_fetch) =
        utopia_store::rss_full_content::begin_feed_observation(&pool, source, &config).await?;
    let doc = utopia_store::documents::create_with_version_and_processing(
        &pool,
        kb,
        "race.md",
        "text/markdown",
        10,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        Some(source),
        None,
        Some("race-key"),
    )
    .await?;
    utopia_store::documents::delete(&pool, kb, doc.id, None).await?;
    utopia_store::documents::purge(&pool, kb, doc.id).await?;
    utopia_store::rss_full_content::discover_observed(
        &pool,
        source,
        1,
        &[entry("race-key")],
        before_fetch,
    )
    .await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        0
    );
    let rows = utopia_store::rss_full_content::list_current_entries(&pool, source, 100).await?;
    assert_eq!(rows[0].state, "deleted");
    utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")]).await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        1
    );
    let row = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
        .await?
        .remove(0);
    let secret = format!(
        "nested: https://user:password@example.com/?token=SECRET {}",
        "é".repeat(3000)
    );
    sqlx::query("UPDATE jobs SET status='failed',last_error=$2 WHERE id=$1")
        .bind(row.hydration_job_id)
        .bind(secret)
        .execute(&pool)
        .await?;
    let row = utopia_store::rss_full_content::get_entry(&pool, row.id)
        .await?
        .unwrap();
    assert_eq!(row.state, "terminal");
    assert_eq!(
        row.error_detail.as_deref(),
        Some("Full-content acquisition failed; retry is available")
    );
    let counts = utopia_store::rss_full_content::counts(&pool, source)
        .await?
        .unwrap();
    assert_eq!(counts.terminal_count, 1);
    let sources = utopia_store::sources::list(&pool, kb).await?;
    assert_eq!(sources[0].rss_full_content_terminal_count, 1);
    // A live document is the accepted-content authority even if an older job failed.
    utopia_store::documents::create_with_version_and_processing(
        &pool,
        kb,
        "race.md",
        "text/markdown",
        10,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        Some(source),
        None,
        Some("race-key"),
    )
    .await?;
    assert_eq!(
        utopia_store::rss_full_content::get_entry(&pool, row.id)
            .await?
            .unwrap()
            .state,
        "complete"
    );
    let counts = utopia_store::rss_full_content::counts(&pool, source)
        .await?
        .unwrap();
    assert_eq!(counts.complete_count, 1);
    assert_eq!(counts.terminal_count, 0);
    cleanup(&pool, org).await?;
    Ok(())
}

#[tokio::test]
async fn poorer_observation_preserves_candidate_without_refreshing_purge_permission(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb, source, _) = seed_source(&pool).await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
    tx.commit().await?;
    utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
    let mut rich = entry("retained");
    rich.article_url = None;
    rich.embedded_html = Some("<p>Retained substantive article</p>".into());
    utopia_store::rss_full_content::discover(&pool, source, 1, &[rich.clone()]).await?;
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT observed_at FROM rss_full_content_entries WHERE source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await?;
    let mut poor = rich.clone();
    poor.embedded_html = None;
    poor.has_usable_source = false;
    poor.summary = "Summary only".into();
    utopia_store::rss_full_content::discover(&pool, source, 1, &[poor.clone()]).await?;
    let row = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
        .await?
        .remove(0);
    assert_eq!(
        row.embedded_html, rich.embedded_html,
        "poorer feed erased retained acquisition content"
    );
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT observed_at FROM rss_full_content_entries WHERE source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await?;
    assert_eq!(
        before, after,
        "old content must not acquire a fresh timestamp"
    );
    let doc = utopia_store::documents::create_with_version_and_processing(
        &pool,
        kb,
        "retained.md",
        "text/markdown",
        10,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        Some(source),
        None,
        Some("retained"),
    )
    .await?;
    utopia_store::documents::delete(&pool, kb, doc.id, None).await?;
    utopia_store::documents::purge(&pool, kb, doc.id).await?;
    utopia_store::rss_full_content::discover(&pool, source, 1, &[poor]).await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        0
    );
    utopia_store::rss_full_content::discover(&pool, source, 1, &[rich]).await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        1
    );
    cleanup(&pool, org).await?;
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

/// A document may predate full-content activation. Its first later observation
/// must not become permission to recreate it after a subsequent purge while
/// admission was paused. No post-purge feed observation occurs in this test.
#[tokio::test]
async fn pre_purge_observation_without_a_job_cannot_publish_after_purge() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org_id, kb_id, source_id, _) = seed_source(&pool).await?;
    let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let original = utopia_store::documents::create_with_version_and_processing(
        &pool,
        kb_id,
        "existing.md",
        "text/markdown",
        10,
        sha,
        Some(source_id),
        None,
        Some("preexisting-key"),
    )
    .await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source_id).await?;
    tx.commit().await?;
    utopia_store::rss_full_content::record_baseline(&pool, source_id, 1, &[]).await?;
    utopia_store::rss_full_content::discover(&pool, source_id, 1, &[entry("preexisting-key")])
        .await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source_id, 1, 0, 5,)
            .await?,
        0
    );
    utopia_store::documents::delete(&pool, kb_id, original.id, None).await?;
    utopia_store::documents::purge(&pool, kb_id, original.id).await?;
    let released: bool = sqlx::query_scalar(
        "SELECT external_key IS NULL AND purged_at IS NOT NULL FROM documents WHERE id = $1",
    )
    .bind(original.id)
    .fetch_one(&pool)
    .await?;
    assert!(released, "exercise real purge, including identity release");

    // A restart/backlog tick is NOT another publisher observation.
    utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source_id, 1, 25, 5).await?;
    let rows = utopia_store::rss_full_content::list_current_entries(&pool, source_id, 100).await?;
    let observation = rows
        .iter()
        .find(|e| e.external_key == "preexisting-key")
        .unwrap();
    let replacement = if let Some(job_id) = observation.hydration_job_id {
        sqlx::query("UPDATE jobs SET status = 'running', attempts = 1 WHERE id = $1 AND status IN ('queued', 'running')")
            .bind(job_id)
            .execute(&pool)
            .await?;
        utopia_store::rss_full_content::current_attempt(&pool, observation.id, job_id).await?;
        utopia_store::rss_full_content::complete_hydration(
            &pool,
            observation.id,
            job_id,
            source_id,
            1,
            kb_id,
            "preexisting-key",
            "existing.md",
            "text/markdown",
            10,
            sha,
            None,
            "feed",
            None,
        )
        .await?
    } else {
        None
    };
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE source_id = $1 AND deleted_at IS NULL",
    )
    .bind(source_id)
    .fetch_one(&pool)
    .await?;
    cleanup(&pool, org_id).await?;
    assert!(
        replacement.is_none(),
        "pre-purge retained payload recreated a purged document"
    );
    assert_eq!(live, 0);
    Ok(())
}

async fn finish_entry(
    pool: &PgPool,
    kb: Uuid,
    source: Uuid,
    entry_id: Uuid,
    job: i64,
) -> anyhow::Result<Option<Uuid>> {
    sqlx::query("UPDATE jobs SET status = 'running', attempts = 1 WHERE id = $1 AND status IN ('queued', 'running')")
        .bind(job)
        .execute(pool)
        .await?;
    utopia_store::rss_full_content::current_attempt(pool, entry_id, job).await?;
    Ok(utopia_store::rss_full_content::complete_hydration(
        pool,
        entry_id,
        job,
        source,
        1,
        kb,
        "race-key",
        "race.md",
        "text/markdown",
        10,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        None,
        "feed",
        None,
    )
    .await?)
}

#[tokio::test]
async fn late_worker_and_null_observation_require_fresh_post_purge_evidence() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // Both absent-at-observation and existing-at-observation identities, with and
    // without a pre-purge job. Include restore/delete and observation-before-purge.
    for (created_later, admitted, restore, observe_before_purge) in [
        (true, false, false, false),
        (true, true, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, true, true, true),
    ] {
        let (org, kb, source, _) = seed_source(&pool).await?;
        let mut tx = pool.begin().await?;
        utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
        tx.commit().await?;
        utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
        if created_later {
            utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")])
                .await?;
        }
        let original = utopia_store::documents::create_with_version_and_processing(
            &pool,
            kb,
            "race.md",
            "text/markdown",
            10,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some(source),
            None,
            Some("race-key"),
        )
        .await?;
        if !created_later {
            utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")])
                .await?;
        }
        utopia_store::rss_full_content::claim_pending_and_enqueue(
            &pool,
            source,
            1,
            if admitted { 25 } else { 0 },
            5,
        )
        .await?;
        let old = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
            .await?
            .remove(0);
        utopia_store::documents::delete(&pool, kb, original.id, None).await?;
        assert!(
            utopia_store::documents::delete(&pool, kb, original.id, None)
                .await
                .is_err()
        );
        if restore {
            utopia_store::documents::restore(&pool, kb, original.id).await?;
            if let Some(job) = old.hydration_job_id {
                assert!(
                    finish_entry(&pool, kb, source, old.id, job)
                        .await?
                        .is_none(),
                    "manual restore must not refresh old worker"
                );
            }
            utopia_store::documents::delete(&pool, kb, original.id, None).await?;
        }
        if observe_before_purge {
            utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")])
                .await?;
        }
        utopia_store::documents::purge(&pool, kb, original.id).await?;
        assert!(utopia_store::documents::purge(&pool, kb, original.id)
            .await
            .is_err());
        assert_eq!(
            utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5)
                .await?,
            0
        );
        if let Some(job) = old.hydration_job_id {
            assert!(
                finish_entry(&pool, kb, source, old.id, job)
                    .await?
                    .is_none(),
                "late worker recreated purged content"
            );
        }
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE source_id = $1 AND deleted_at IS NULL",
        )
        .bind(source)
        .fetch_one(&pool)
        .await?;
        assert_eq!(live, 0);
        utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")]).await?;
        assert_eq!(
            utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5)
                .await?,
            1,
            "actual later observation permits replacement"
        );
        let new = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
            .await?
            .remove(0);
        if let Some(job) = old.hydration_job_id {
            assert_ne!(new.hydration_job_id, Some(job));
            assert!(
                finish_entry(&pool, kb, source, old.id, job)
                    .await?
                    .is_none(),
                "superseded worker inherited fresh permission"
            );
        }
        let replacement = finish_entry(&pool, kb, source, new.id, new.hydration_job_id.unwrap())
            .await?
            .unwrap();
        assert_ne!(replacement, original.id);
        assert!(
            finish_entry(&pool, kb, source, new.id, new.hydration_job_id.unwrap())
                .await?
                .is_none()
        );
        cleanup(&pool, org).await?;
    }
    Ok(())
}

#[tokio::test]
async fn later_observation_restores_soft_deleted_identity() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb, source, _) = seed_source(&pool).await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
    tx.commit().await?;
    utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
    utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")]).await?;
    utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?;
    let old = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
        .await?
        .remove(0);
    let original = finish_entry(&pool, kb, source, old.id, old.hydration_job_id.unwrap())
        .await?
        .unwrap();
    utopia_store::documents::delete(&pool, kb, original, None).await?;
    assert!(
        finish_entry(&pool, kb, source, old.id, old.hydration_job_id.unwrap())
            .await?
            .is_none()
    );
    utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")]).await?;
    assert_eq!(
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?,
        1
    );
    let new = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
        .await?
        .remove(0);
    let restored = finish_entry(&pool, kb, source, new.id, new.hydration_job_id.unwrap()).await?;
    assert_eq!(restored, Some(original));
    cleanup(&pool, org).await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_deletion_and_publication_leave_no_live_document() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    for _ in 0..8 {
        let (org, kb, source, _) = seed_source(&pool).await?;
        let mut tx = pool.begin().await?;
        utopia_store::rss_full_content::initialize_source(&mut tx, source).await?;
        tx.commit().await?;
        utopia_store::rss_full_content::record_baseline(&pool, source, 1, &[]).await?;
        let original = utopia_store::documents::create_with_version_and_processing(
            &pool,
            kb,
            "race.md",
            "text/markdown",
            10,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some(source),
            None,
            Some("race-key"),
        )
        .await?;
        utopia_store::rss_full_content::discover(&pool, source, 1, &[entry("race-key")]).await?;
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5).await?;
        let observation = utopia_store::rss_full_content::list_current_entries(&pool, source, 100)
            .await?
            .remove(0);
        let job = observation.hydration_job_id.unwrap();
        let (completion, deletion) =
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                tokio::join!(
                    finish_entry(&pool, kb, source, observation.id, job),
                    utopia_store::documents::delete(&pool, kb, original.id, None),
                )
            })
            .await?;
        if let Some(id) = completion? {
            assert_eq!(id, original.id);
        }
        deletion?;
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE source_id = $1 AND deleted_at IS NULL",
        )
        .bind(source)
        .fetch_one(&pool)
        .await?;
        assert_eq!(live, 0);
        utopia_store::documents::purge(&pool, kb, original.id).await?;
        // Simulate restart plus a generic failed-job requeue. Neither is feed evidence.
        sqlx::query("UPDATE jobs SET status = 'failed' WHERE id = $1")
            .bind(job)
            .execute(&pool)
            .await?;
        utopia_store::rss_full_content::requeue_failed(
            &pool,
            utopia_store::jobs::RequeueScope {
                kb_id: Some(kb),
                kind: Some("hydrate_rss_entry"),
                failed_since: None,
            },
        )
        .await?;
        assert!(finish_entry(&pool, kb, source, observation.id, job)
            .await?
            .is_none());
        assert_eq!(
            utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source, 1, 25, 5)
                .await?,
            0
        );
        cleanup(&pool, org).await?;
    }
    Ok(())
}

async fn cleanup(pool: &PgPool, org_id: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn baseline_activation_creates_no_documents_or_hydration_jobs() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org_id, _kb_id, source_id, _workspace_id) = seed_source(&pool).await?;
    sqlx::query("UPDATE sources SET rss_generation=1, rss_baselined_at=NULL WHERE id=$1")
        .bind(source_id)
        .execute(&pool)
        .await?;

    let entries = vec![entry("baseline-1"), entry("baseline-2")];
    let inserted =
        utopia_store::rss_full_content::record_baseline(&pool, source_id, 1, &entries).await?;
    assert_eq!(inserted, 2);

    let activation = utopia_store::rss_full_content::get_activation(&pool, source_id)
        .await?
        .unwrap();
    assert_eq!(activation.activation_state, "active");
    assert_eq!(activation.baseline_count, 2);
    let (documents,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM documents WHERE source_id = $1")
            .bind(source_id)
            .fetch_one(&pool)
            .await?;
    let (jobs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM jobs WHERE kind = 'hydrate_rss_entry' AND payload->>'source_id' = $1",
    )
    .bind(source_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(documents, 0);
    assert_eq!(jobs, 0);
    cleanup(&pool, org_id).await?;
    Ok(())
}

#[tokio::test]
async fn repeated_discovery_is_idempotent_and_claiming_is_capped_at_twenty_five(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org_id, _kb_id, source_id, _workspace_id) = seed_source(&pool).await?;
    sqlx::query("UPDATE sources SET rss_generation=1, rss_baselined_at=now() WHERE id=$1")
        .bind(source_id)
        .execute(&pool)
        .await?;

    let entries: Vec<_> = (0..30)
        .map(|index| entry(format!("entry-{index}")))
        .collect();
    let first = utopia_store::rss_full_content::discover(&pool, source_id, 1, &entries).await?;
    assert_eq!(first.discovered, 30);
    assert_eq!(first.terminal, 0);
    let second = utopia_store::rss_full_content::discover(&pool, source_id, 1, &entries).await?;
    assert_eq!(second.discovered, 0);

    let queued =
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source_id, 1, 25, 5)
            .await?;
    assert_eq!(queued, 25);
    let repeated_claim =
        utopia_store::rss_full_content::claim_pending_and_enqueue(&pool, source_id, 1, 25, 5)
            .await?;
    assert_eq!(repeated_claim, 0);

    let counts = utopia_store::rss_full_content::counts(&pool, source_id)
        .await?
        .unwrap();
    let pending = counts.pending_count;
    let queued_rows = counts.queued_count;
    let (jobs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM jobs
         WHERE kind = 'hydrate_rss_entry' AND payload->>'source_id' = $1",
    )
    .bind(source_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(pending, 5);
    assert_eq!(queued_rows, 25);
    assert_eq!(jobs, 25);
    cleanup(&pool, org_id).await?;
    Ok(())
}

#[tokio::test]
async fn content_replacement_preserves_document_identity_and_queues_processing_atomically(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org_id, kb_id, source_id, _workspace_id) = seed_source(&pool).await?;
    let document_id = Uuid::now_v7();
    let old_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let new_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    sqlx::query(
        "INSERT INTO documents
            (id, kb_id, source_id, filename, mime, size_bytes, sha256, external_key)
         VALUES ($1, $2, $3, 'entry.md', 'text/markdown', 3, $4, 'rss-key')",
    )
    .bind(document_id)
    .bind(kb_id)
    .bind(source_id)
    .bind(old_sha)
    .execute(&pool)
    .await?;

    utopia_store::documents::replace_content_and_enqueue_processing(
        &pool,
        document_id,
        "entry.md",
        "text/markdown",
        11,
        new_sha,
        None,
    )
    .await?;

    let (actual_id, actual_sha): (Uuid, String) =
        sqlx::query_as("SELECT id, sha256 FROM documents WHERE id = $1")
            .bind(document_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(actual_id, document_id);
    assert_eq!(actual_sha, new_sha);

    let (versions,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM document_versions WHERE document_id = $1 AND sha256 = $2",
    )
    .bind(document_id)
    .bind(new_sha)
    .fetch_one(&pool)
    .await?;
    assert_eq!(versions, 1);

    let (jobs,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM jobs
         WHERE kind = 'process_document'
           AND payload->>'document_id' = $1",
    )
    .bind(document_id.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(jobs, 1);
    cleanup(&pool, org_id).await?;
    Ok(())
}
