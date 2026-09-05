//! Exercises the production RSS HTTP/parser/synchronization path against a
//! private synthetic database and a loopback fixture (operator-owned feed).
use super::sync_rss;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

fn feed(ids: impl IntoIterator<Item = usize>) -> String {
    let body=ids.into_iter().map(|id|format!("<item><guid>id-{id}</guid><title>Entry {id}</title><link>https://example.com/{id}</link><description>Summary</description></item>")).collect::<String>();
    format!("<rss version=\"2.0\"><channel><title>Fixture</title><link>https://example.com/</link><description>Fixture</description>{body}</channel></rss>")
}

#[tokio::test]
async fn rss_sync_large_empty_failed_and_stale_responses() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let org = Uuid::now_v7();
    let ws = Uuid::now_v7();
    let kb = Uuid::now_v7();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'rss-sync-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'rss-sync-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'rss-sync-test')")
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
    let server = MockServer::start().await;
    sqlx::query("INSERT INTO sources(id,kb_id,kind,name,config,rss_generation) VALUES($1,$2,'rss','fixture',$3,1)")
        .bind(id).bind(kb).bind(serde_json::json!({"feed_url":server.uri(),"content_mode":"full_new_items"})).execute(&pool).await?;
    let dir = std::env::temp_dir().join(format!("utopia-rss-sync-{id}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let source = utopia_store::sources::get(&pool, id).await?;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed(0..201)))
        .mount(&server)
        .await;
    sync_rss(&state, &source).await?;
    let a = utopia_store::rss_full_content::get_activation(&pool, id)
        .await?
        .unwrap();
    assert_eq!(a.baseline_count, 201);
    let bodies:i64=sqlx::query_scalar("SELECT count(*) FROM rss_full_content_entries WHERE source_id=$1 AND (summary<>'' OR embedded_html IS NOT NULL)").bind(id).fetch_one(&pool).await?;
    assert_eq!(bodies, 0);
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed((0..202).rev())))
        .mount(&server)
        .await;
    let stats = sync_rss(&state, &source).await?;
    assert_eq!(stats.queued_for_content, 1);
    let c = utopia_store::rss_full_content::counts(&pool, id)
        .await?
        .unwrap();
    assert_eq!(c.baseline_count, 201);
    assert_eq!(c.queued_count, 1);
    // A failed response cannot activate; an empty successful response must.
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::enable_source(&mut tx, id).await?;
    tx.commit().await?;
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not a feed"))
        .mount(&server)
        .await;
    assert!(sync_rss(&state, &source).await.is_err());
    assert_eq!(
        utopia_store::rss_full_content::get_activation(&pool, id)
            .await?
            .unwrap()
            .activation_state,
        "pending"
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed([])))
        .mount(&server)
        .await;
    sync_rss(&state, &source).await?;
    assert_eq!(
        utopia_store::rss_full_content::get_activation(&pool, id)
            .await?
            .unwrap()
            .activation_state,
        "active"
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed([999])))
        .mount(&server)
        .await;
    assert_eq!(sync_rss(&state, &source).await?.queued_for_content, 1);
    // Wait for an actual request, not an assumed sleep, before changing generation.
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(feed([1000]))
                .set_delay(std::time::Duration::from_millis(500)),
        )
        .mount(&server)
        .await;
    let worker_state = state.clone();
    let old_source = source.clone();
    let task = tokio::spawn(async move { sync_rss(&worker_state, &old_source).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if server
                .received_requests()
                .await
                .is_some_and(|r| !r.is_empty())
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await?;
    let mut tx = pool.begin().await?;
    utopia_store::rss_full_content::enable_source(&mut tx, id).await?;
    tx.commit().await?;
    assert!(task.await?.is_err());
    let a = utopia_store::rss_full_content::get_activation(&pool, id)
        .await?
        .unwrap();
    assert_eq!(a.activation_state, "pending");
    assert_eq!(a.baseline_count, 0);
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed([])))
        .mount(&server)
        .await;
    sync_rss(&state, &source).await?;
    server.reset().await;
    let content="Detailed reporting examines implementation choices, testing evidence, operational risks, historical context, and specific consequences for engineers maintaining reliable software systems. ".repeat(30);
    let full=format!("<rss version=\"2.0\" xmlns:content=\"http://purl.org/rss/1.0/modules/content/\"><channel><title>Fixture</title><link>https://example.com/</link><description>Fixture</description><item><guid>native-full</guid><title>Native article</title><description>Summary</description><content:encoded><![CDATA[<article><h1>Native article</h1><p>{content}</p></article>]]></content:encoded></item></channel></rss>");
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(full))
        .mount(&server)
        .await;
    assert_eq!(sync_rss(&state, &source).await?.queued_for_content, 1);
    let entry = utopia_store::rss_full_content::list_current_entries(&pool, id, 100)
        .await?
        .remove(0);
    let job = entry.hydration_job_id.unwrap();
    sqlx::query("UPDATE jobs SET status='running',attempts=1 WHERE id=$1 AND status='queued'")
        .bind(job)
        .execute(&pool)
        .await?;
    crate::rss_full_content::hydrate_entry(&state, job, id, entry.id).await?;
    let completed = utopia_store::rss_full_content::get_entry(&pool, entry.id)
        .await?
        .unwrap();
    assert_eq!(completed.state, "complete");
    assert!(completed.embedded_html.is_none());
    let document = completed.document_id.unwrap();
    let (mime, bytes): (String, i64) =
        sqlx::query_as("SELECT mime,size_bytes FROM documents WHERE id=$1")
            .bind(document)
            .fetch_one(&pool)
            .await?;
    assert_eq!(mime, "text/markdown");
    assert!(bytes > 1000);
    let processing: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='process_document' AND payload->>'document_id'=$1",
    )
    .bind(document.to_string())
    .fetch_one(&pool)
    .await?;
    assert_eq!(processing, 1);
    // Shared non-full-content source ingestion must preserve upstream restoration.
    for (key, changed) in [("ordinary-same", false), ("ordinary-changed", true)] {
        let original = format!("Original content for {key}");
        super::ingest_item(
            &state,
            kb,
            id,
            key,
            "ordinary.md",
            "text/markdown",
            original.as_bytes(),
            None,
        )
        .await?;
        let doc = utopia_store::documents::find_by_external_key(&pool, id, key)
            .await?
            .unwrap();
        utopia_store::documents::delete(&pool, kb, doc.id, None).await?;
        let fresh = if changed {
            format!("Changed content for {key}")
        } else {
            original
        };
        let action = super::ingest_item(
            &state,
            kb,
            id,
            key,
            "ordinary.md",
            "text/markdown",
            fresh.as_bytes(),
            None,
        )
        .await?;
        assert_eq!(action, super::IngestAction::Updated);
        let restored = utopia_store::documents::find_by_external_key(&pool, id, key)
            .await?
            .unwrap();
        assert_eq!(restored.id, doc.id);
        assert!(restored.deleted_at.is_none());
        assert_eq!(restored.sha256, super::sha_hex(fresh.as_bytes()));
    }
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
