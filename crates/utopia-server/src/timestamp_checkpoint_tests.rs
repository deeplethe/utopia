//! ADR 0072: persisted progress describes complete source timestamp groups.
use super::{sync_github_issues_with_client, sync_source};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use utopia_store::sources;
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const FIRST: &str = "2026-01-02T12:34:20Z";
const NEXT: &str = "2026-01-02T12:34:40Z";

fn time(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

#[derive(Clone, Copy)]
enum Provider {
    Github,
    Jira,
}

impl Provider {
    fn kind(self) -> &'static str {
        match self {
            Self::Github => "github_issues",
            Self::Jira => "jira_issues",
        }
    }

    fn row(self, number: usize, updated: &str) -> Value {
        match self {
            Self::Github => json!({"number":number, "title":format!("Issue {number}"),
                "state":"open", "body":format!("Body {number}"),
                "created_at":"2026-01-01T00:00:00Z", "updated_at":updated}),
            Self::Jira => json!({"key":format!("TEST-{number}"), "fields":{
                "summary":format!("Issue {number}"), "description":format!("Body {number}"),
                "created":"2026-01-01T00:00:00Z", "updated":updated}}),
        }
    }
}

#[derive(Default)]
struct Feed {
    rows: Vec<Value>,
    fail_page: Option<usize>,
    oversized_discussion: Option<(&'static str, usize)>,
    fail_detail_page: Option<usize>,
}

struct Fixture {
    provider: Provider,
    state: crate::state::AppState,
    source_id: Uuid,
    org_id: Uuid,
    server: MockServer,
    feed: Arc<Mutex<Feed>>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    async fn new(provider: Provider) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        let (org_id, workspace_id, kb_id, source_id) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'timestamp-test')")
            .bind(org_id)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'timestamp-test')")
            .bind(workspace_id)
            .bind(org_id)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'timestamp-test')",
        )
        .bind(kb_id)
        .bind(workspace_id)
        .execute(&pool)
        .await?;
        let server = MockServer::start().await;
        let config = match provider {
            Provider::Github => json!({"repo":"test/repo"}),
            Provider::Jira => json!({"base_url":server.uri(), "project":"TEST"}),
        };
        sqlx::query(
            "INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,$3,'timestamp-test',$4)",
        )
        .bind(source_id)
        .bind(kb_id)
        .bind(provider.kind())
        .bind(config)
        .execute(&pool)
        .await?;
        let directory = tempfile::tempdir()?;
        let config = utopia_core::config::AppConfig {
            data_dir: directory.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &directory.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool, &config, search, "test-only".into());
        let feed = Arc::new(Mutex::new(Feed::default()));
        let current = feed.clone();
        Mock::given(method("GET")).respond_with(move |request: &Request| {
            let feed = current.lock().unwrap();
            let query_number = |key: &str, default: usize| request.url.query_pairs()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.parse::<usize>().unwrap()).unwrap_or(default);
            let path = request.url.path();
            if path.ends_with("/comments") || path.ends_with("/events") {
                if let Some((kind, count)) = feed.oversized_discussion {
                    if path.ends_with(kind) && path.contains("/issues/1/") {
                        let page = query_number("page", 1);
                        if feed.fail_detail_page == Some(page) {
                            return ResponseTemplate::new(503);
                        }
                        let rows: Vec<_> = (1..=count).skip((page - 1) * 100).take(100)
                            .map(|number| if kind == "comments" {
                                json!({"issue_url":"https://api.github.com/repos/test/repo/issues/1",
                                    "created_at":FIRST, "body":format!("Discussion item {number}")})
                            } else {
                                json!({"event":"closed", "created_at":FIRST,
                                    "actor":{"login":format!("Discussion item {number}")}})
                            }).collect();
                        return ResponseTemplate::new(200).set_body_json(rows);
                    }
                }
                return ResponseTemplate::new(200).set_body_json(json!([]));
            }
            let (offset, limit) = match provider {
                Provider::Github => ((query_number("page", 1) - 1) * 100, 100),
                Provider::Jira => (query_number("startAt", 0), query_number("maxResults", 50)),
            };
            if feed.fail_page == Some(offset / limit + 1) {
                return ResponseTemplate::new(503);
            }
            // Return the consumed prefix deliberately: provider bounds are inclusive/rounded.
            let rows: Vec<_> = feed.rows.iter().skip(offset).take(limit).cloned().collect();
            let body = match provider {
                Provider::Github => json!(rows),
                Provider::Jira => json!({"issues":rows,"total":feed.rows.len(),"startAt":offset,"maxResults":limit}),
            };
            ResponseTemplate::new(200).set_body_json(body)
        }).mount(&server).await;
        Ok(Some(Self {
            provider,
            state,
            source_id,
            org_id,
            server,
            feed,
            _directory: directory,
        }))
    }

    fn set_rows(&self, first_count: usize, next_count: usize) {
        self.feed.lock().unwrap().rows = (1..=first_count + next_count)
            .map(|number| {
                self.provider
                    .row(number, if number <= first_count { FIRST } else { NEXT })
            })
            .collect();
    }

    async fn sync(&self) -> anyhow::Result<()> {
        if matches!(self.provider, Provider::Jira) {
            sync_source(&self.state, self.source_id).await?;
            return Ok(());
        }
        let previous = sources::sync_checkpoint(&self.state.pool, self.source_id).await?;
        let source = sources::get(&self.state.pool, self.source_id).await?;
        let run = sources::start_run(&self.state.pool, self.source_id).await?;
        let http = reqwest::Client::builder().no_proxy().build()?;
        let stats = sync_github_issues_with_client(
            &self.state,
            &source,
            previous,
            &http,
            &self.server.uri(),
        )
        .await?;
        sources::finish_timestamp_sync(
            &self.state.pool,
            run,
            self.source_id,
            previous,
            stats.checkpoint,
            stats.created as i32,
            stats.updated as i32,
        )
        .await?;
        Ok(())
    }

    async fn checkpoint(&self) -> anyhow::Result<Option<DateTime<Utc>>> {
        Ok(sources::sync_checkpoint(&self.state.pool, self.source_id).await?)
    }

    async fn counts(&self) -> anyhow::Result<(i64, i64, i64)> {
        Ok(sqlx::query_as("SELECT
            (SELECT count(*) FROM documents WHERE source_id=$1),
            (SELECT count(*) FROM document_versions v JOIN documents d ON d.id=v.document_id WHERE d.source_id=$1),
            (SELECT count(*) FROM jobs j JOIN documents d ON d.id::text=j.payload->>'document_id' WHERE d.source_id=$1)")
            .bind(self.source_id).fetch_one(&self.state.pool).await?)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM jobs WHERE payload->>'document_id' IN (SELECT id::text FROM documents WHERE source_id=$1)")
            .bind(self.source_id).execute(&self.state.pool).await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org_id)
            .execute(&self.state.pool)
            .await?;
        Ok(())
    }
}

async fn large_group_progress(provider: Provider, group_size: usize) -> anyhow::Result<()> {
    let Some(fixture) = Fixture::new(provider).await? else {
        return Ok(());
    };
    fixture.set_rows(group_size, 1);
    fixture.sync().await?;
    assert_eq!(fixture.counts().await?.0, group_size as i64);
    assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
    fixture.sync().await?;
    let counts = fixture.counts().await?;
    assert_eq!(
        counts,
        (
            (group_size + 1) as i64,
            (group_size + 1) as i64,
            (group_size + 1) as i64
        )
    );
    assert_eq!(fixture.checkpoint().await?, Some(time(NEXT)));
    fixture.sync().await?;
    assert_eq!(fixture.counts().await?, counts);
    assert_eq!(fixture.checkpoint().await?, Some(time(NEXT)));
    let requests = fixture.server.received_requests().await.unwrap();
    let incremental = requests.iter().any(|request| match provider {
        Provider::Github => request.url.query_pairs().any(|(key, value)| {
            key == "since"
                && (time(&value) == time(FIRST)
                    || time(&value) == time(FIRST) + chrono::Duration::seconds(1))
        }),
        Provider::Jira => request
            .url
            .query_pairs()
            .any(|(key, value)| key == "jql" && value.contains("2026-01-02 12:34")),
    });
    assert!(
        incremental,
        "the next run must send the persisted source bound"
    );
    fixture.cleanup().await
}

#[tokio::test]
async fn github_imports_1001_equal_time_items_then_skips_the_old_prefix() -> anyhow::Result<()> {
    large_group_progress(Provider::Github, 1001).await
}

#[tokio::test]
async fn jira_imports_501_equal_time_items_then_skips_the_rounded_prefix() -> anyhow::Result<()> {
    large_group_progress(Provider::Jira, 501).await
}

#[tokio::test]
async fn processing_budget_finishes_item_240_and_leaves_the_next_group() -> anyhow::Result<()> {
    for provider in [Provider::Github, Provider::Jira] {
        large_group_progress(provider, 240).await?;
    }
    Ok(())
}

#[tokio::test]
async fn failed_listing_does_not_advance_and_retry_deduplicates() -> anyhow::Result<()> {
    for provider in [Provider::Github, Provider::Jira] {
        let Some(fixture) = Fixture::new(provider).await? else {
            return Ok(());
        };
        fixture.set_rows(240, 0);
        fixture.feed.lock().unwrap().fail_page = Some(2);
        assert!(fixture.sync().await.is_err());
        assert_eq!(fixture.checkpoint().await?, None);
        fixture.feed.lock().unwrap().fail_page = None;
        fixture.sync().await?;
        assert_eq!(fixture.counts().await?, (240, 240, 240));
        assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
        fixture.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn github_failed_discussion_tail_preserves_the_document_and_recovers() -> anyhow::Result<()> {
    for (endpoint, count) in [("comments", 1001), ("events", 1100)] {
        let Some(fixture) = Fixture::new(Provider::Github).await? else {
            return Ok(());
        };
        fixture.set_rows(1, 0);
        fixture.sync().await?;
        let original: String =
            sqlx::query_scalar("SELECT sha256 FROM documents WHERE source_id=$1")
                .bind(fixture.source_id)
                .fetch_one(&fixture.state.pool)
                .await?;
        {
            let mut feed = fixture.feed.lock().unwrap();
            feed.rows = vec![Provider::Github.row(1, NEXT), Provider::Github.row(2, NEXT)];
            feed.oversized_discussion = Some((endpoint, count));
            feed.fail_detail_page = Some(11);
        }
        assert!(
            fixture.sync().await.is_err(),
            "failed {endpoint} tail must not replace a document"
        );
        assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
        let current: String = sqlx::query_scalar("SELECT sha256 FROM documents WHERE source_id=$1")
            .bind(fixture.source_id)
            .fetch_one(&fixture.state.pool)
            .await?;
        assert_eq!(current, original);
        assert_eq!(fixture.counts().await?, (1, 1, 1));
        fixture.feed.lock().unwrap().fail_detail_page = None;
        fixture.sync().await?;
        assert_eq!(fixture.checkpoint().await?, Some(time(NEXT)));
        assert_eq!(fixture.counts().await?, (2, 3, 3));
        let updated: String = sqlx::query_scalar(
            "SELECT sha256 FROM documents WHERE source_id=$1 AND external_key='github:test/repo#1'",
        )
        .bind(fixture.source_id)
        .fetch_one(&fixture.state.pool)
        .await?;
        let body = String::from_utf8(fixture.state.blob.get(&updated).await?)?;
        assert!(body.contains(&format!("Discussion item {count}")));
        fixture.sync().await?;
        assert_eq!(fixture.counts().await?, (2, 3, 3));
        fixture.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn recovery_ignores_old_success_and_retains_history_after_audit_pruning() -> anyhow::Result<()>
{
    for provider in [Provider::Github, Provider::Jira] {
        let Some(fixture) = Fixture::new(provider).await? else {
            return Ok(());
        };
        fixture.set_rows(1, 0);
        fixture.sync().await?;
        let before = fixture.counts().await?;
        sqlx::query("UPDATE sources SET sync_checkpoint=NULL WHERE id=$1")
            .bind(fixture.source_id)
            .execute(&fixture.state.pool)
            .await?;
        assert_eq!(fixture.checkpoint().await?, None);
        fixture.sync().await?;
        assert_eq!(
            fixture.counts().await?,
            before,
            "recovery must retain versions and jobs"
        );
        assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
        for _ in 0..51 {
            let run = sources::start_run(&fixture.state.pool, fixture.source_id).await?;
            sources::finish_run(
                &fixture.state.pool,
                run,
                fixture.source_id,
                Some("fixture failure"),
                0,
                0,
            )
            .await?;
        }
        assert_eq!(
            sources::list_runs(&fixture.state.pool, fixture.source_id, 100)
                .await?
                .len(),
            50
        );
        assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
        fixture.feed.lock().unwrap().rows.clear();
        fixture.sync().await?;
        assert_eq!(
            fixture.checkpoint().await?,
            Some(time(FIRST)),
            "empty reads must not advance to wall clock"
        );
        assert_eq!(fixture.counts().await?, before);
        fixture.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn checkpoint_and_run_success_commit_atomically() -> anyhow::Result<()> {
    let Some(fixture) = Fixture::new(Provider::Github).await? else {
        return Ok(());
    };
    let pool = &fixture.state.pool;
    assert!(sources::finish_timestamp_sync(
        pool,
        Uuid::now_v7(),
        fixture.source_id,
        None,
        Some(time(FIRST)),
        1,
        0
    )
    .await
    .is_err());
    assert_eq!(
        fixture.checkpoint().await?,
        None,
        "missing run must roll back source checkpoint"
    );
    let run = sources::start_run(pool, fixture.source_id).await?;
    sources::finish_timestamp_sync(pool, run, fixture.source_id, None, Some(time(FIRST)), 1, 0)
        .await?;
    let stale_run = sources::start_run(pool, fixture.source_id).await?;
    assert!(sources::finish_timestamp_sync(
        pool,
        stale_run,
        fixture.source_id,
        None,
        Some(time(NEXT)),
        1,
        0
    )
    .await
    .is_err());
    assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
    let status: String = sqlx::query_scalar("SELECT status FROM source_sync_runs WHERE id=$1")
        .bind(stale_run)
        .fetch_one(pool)
        .await?;
    assert_eq!(
        status, "running",
        "checkpoint conflict must not mark the stale run successful"
    );
    fixture.cleanup().await
}

#[tokio::test]
async fn a_document_write_failure_keeps_the_checkpoint_and_retry_deduplicates() -> anyhow::Result<()>
{
    for provider in [Provider::Github, Provider::Jira] {
        let Some(fixture) = Fixture::new(provider).await? else {
            return Ok(());
        };
        fixture.set_rows(3, 0);
        // The constraint affects only this fixture's second document, including on shared test databases.
        let constraint = format!("checkpoint_write_failure_{}", fixture.source_id.simple());
        let second_key = match provider {
            Provider::Github => "github:test/repo#2".to_owned(),
            Provider::Jira => "jira:127.0.0.1/TEST-2".to_owned(),
        };
        sqlx::query(&format!("ALTER TABLE documents ADD CONSTRAINT {constraint} CHECK \
            (source_id IS DISTINCT FROM '{}'::uuid OR external_key IS DISTINCT FROM '{}') NOT VALID",
            fixture.source_id, second_key))
            .execute(&fixture.state.pool).await?;
        let failed = fixture.sync().await;
        sqlx::query(&format!(
            "ALTER TABLE documents DROP CONSTRAINT {constraint}"
        ))
        .execute(&fixture.state.pool)
        .await?;
        assert!(failed.is_err(), "the second document write must fail");
        assert_eq!(fixture.checkpoint().await?, None);
        assert_eq!(
            fixture.counts().await?,
            (1, 1, 1),
            "the first item was committed before the failure"
        );
        fixture.sync().await?;
        assert_eq!(fixture.counts().await?, (3, 3, 3));
        assert_eq!(fixture.checkpoint().await?, Some(time(FIRST)));
        fixture.cleanup().await?;
    }
    Ok(())
}
