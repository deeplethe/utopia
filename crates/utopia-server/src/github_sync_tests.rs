//! A replacement issue document must contain its entire discussion, even when only
//! its title changed since the last sync. These fixtures exercise real persistence.
use super::sync_github_issues_with_client;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use utopia_core::models::{Document, Source};
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Initial,
    TitleOnly,
    DiscussionUpdated,
    CommentPageFailure,
    Repeated,
    CommentsDeleted,
}

fn time(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

fn issue(number: i64, stage: Stage) -> Value {
    json!({
        "number": number,
        "title": if stage == Stage::Initial { "Original issue" } else { "Updated issue" },
        "state": "open",
        "body": "A synthetic discussion for the sync regression.",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": if stage == Stage::Initial { "2026-01-02T00:00:00Z" } else { "2026-01-04T00:00:00Z" },
        "pull_request": if number == 20 { json!({"url":"https://api.github.com/repos/test/repo/pulls/20"}) } else { Value::Null },
    })
}

fn comments(stage: Stage) -> Vec<Value> {
    if stage == Stage::CommentsDeleted {
        return vec![];
    }
    (0..if matches!(stage, Stage::Initial | Stage::TitleOnly) { 100 } else { 101 })
        .map(|index| {
            let created = time("2026-01-01T01:00:00Z") + chrono::Duration::seconds(index);
            json!({
                "issue_url": "https://api.github.com/repos/test/repo/issues/18",
                "user": {"login":"fixture-author"},
                "created_at": if index == 100 { time("2026-01-04T00:00:00Z") } else { created },
                "body": match index {
                    0 => "The original conclusion must survive every refresh.".to_owned(),
                    1 if matches!(stage, Stage::Initial | Stage::TitleOnly) => "An explanation before correction.".to_owned(),
                    1 => "The corrected explanation.".to_owned(),
                    100 => "A new follow-up on the second page.".to_owned(),
                    _ => format!("Earlier discussion comment {index}."),
                }
            })
        })
        .collect()
}

async fn document(
    state: &crate::state::AppState,
    source: &Source,
) -> anyhow::Result<(Document, String)> {
    let doc = utopia_store::documents::find_by_external_key(
        &state.pool,
        source.id,
        "github:test/repo#18",
    )
    .await?
    .expect("the issue should have been persisted");
    let body = String::from_utf8(state.blob.get(&doc.sha256).await?)?;
    Ok((doc, body))
}

async fn versions(state: &crate::state::AppState, document_id: Uuid) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM document_versions WHERE document_id=$1")
            .bind(document_id)
            .fetch_one(&state.pool)
            .await?,
    )
}

#[tokio::test]
async fn incremental_github_sync_preserves_the_complete_discussion() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, source_id) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'github-sync-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'github-sync-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'github-sync-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,'github_issues','fixture',$3)",
    )
    .bind(source_id)
    .bind(kb)
    .bind(json!({"repo":"test/repo", "auth_header":"Bearer fixture-token"}))
    .execute(&pool)
    .await?;
    let source = utopia_store::sources::get(&pool, source_id).await?;
    let dir = tempfile::tempdir()?;
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(
        &dir.path().join("search"),
    )?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let server = MockServer::start().await;
    let stage = Arc::new(Mutex::new(Stage::Initial));
    let current = stage.clone();
    Mock::given(method("GET"))
        .respond_with(move |request: &Request| {
            let stage = *current.lock().unwrap();
            let page = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "page")
                .map(|(_, value)| value.parse::<usize>().unwrap())
                .unwrap_or(1);
            let since = request.url.query_pairs().any(|(key, _)| key == "since");
            let path = request.url.path();
            let rows = match path {
                "/repos/test/repo/issues" => {
                    if since {
                        vec![issue(18, stage), issue(20, stage)]
                    } else {
                        vec![issue(18, stage), issue(19, stage), issue(20, stage)]
                    }
                }
                // Keep the old endpoint realistic, so the regression fails on lost
                // stored history rather than on an unexpected request path.
                "/repos/test/repo/issues/comments" | "/repos/test/repo/issues/18/comments" => {
                    if stage == Stage::CommentPageFailure && page == 2 {
                        return ResponseTemplate::new(503);
                    }
                    comments(stage)
                        .into_iter()
                        .enumerate()
                        .filter(|(index, _)| {
                            !since
                                || (!matches!(stage, Stage::Initial | Stage::TitleOnly)
                                    && (*index == 1 || *index == 100))
                        })
                        .map(|(_, value)| value)
                        .collect()
                }
                "/repos/test/repo/issues/19/comments" => vec![],
                "/repos/test/repo/issues/18/events" | "/repos/test/repo/issues/19/events" => vec![],
                _ => return ResponseTemplate::new(404),
            };
            let rows: Vec<_> = rows.into_iter().skip((page - 1) * 100).take(100).collect();
            ResponseTemplate::new(200).set_body_json(rows)
        })
        .mount(&server)
        .await;
    let http = reqwest::Client::builder().no_proxy().build()?;
    let since = Some(time("2026-01-03T00:00:00Z"));
    let result = async {
        let first =
            sync_github_issues_with_client(&state, &source, None, &http, &server.uri()).await?;
        anyhow::ensure!(
            first.created == 2,
            "both issues, including the one without comments, should be created"
        );
        let (original, original_body) = document(&state, &source).await?;
        anyhow::ensure!(original_body.contains("The original conclusion"));

        *stage.lock().unwrap() = Stage::TitleOnly;
        let title_only =
            sync_github_issues_with_client(&state, &source, since, &http, &server.uri()).await?;
        anyhow::ensure!(title_only.updated == 1);
        let (_, title_body) = document(&state, &source).await?;
        anyhow::ensure!(
            title_body.contains("The original conclusion")
                && title_body.contains("An explanation before correction."),
            "updating only an issue title erased its stored discussion"
        );

        *stage.lock().unwrap() = Stage::DiscussionUpdated;
        let changed =
            sync_github_issues_with_client(&state, &source, since, &http, &server.uri()).await?;
        anyhow::ensure!(changed.updated == 1 && changed.created == 0);
        let (updated, body) = document(&state, &source).await?;
        anyhow::ensure!(
            body.contains("The original conclusion"),
            "incremental sync erased an unchanged comment from the stored document"
        );
        anyhow::ensure!(body.contains("The corrected explanation."));
        anyhow::ensure!(!body.contains("An explanation before correction."));
        anyhow::ensure!(body.contains("A new follow-up on the second page."));
        anyhow::ensure!(updated.id == original.id && updated.sha256 != original.sha256);
        anyhow::ensure!(updated.doc_time == Some(time("2026-01-04T00:00:00Z")));
        anyhow::ensure!(versions(&state, updated.id).await? == 3);
        anyhow::ensure!(
            String::from_utf8(state.blob.get(&original.sha256).await?)? == original_body
        );

        *stage.lock().unwrap() = Stage::CommentPageFailure;
        let failed =
            sync_github_issues_with_client(&state, &source, since, &http, &server.uri()).await;
        anyhow::ensure!(failed.is_err(), "a failed comment page must fail the sync");
        let (after_failure, after_failure_body) = document(&state, &source).await?;
        anyhow::ensure!(after_failure.sha256 == updated.sha256 && after_failure_body == body);
        anyhow::ensure!(versions(&state, updated.id).await? == 3);

        *stage.lock().unwrap() = Stage::Repeated;
        let repeated =
            sync_github_issues_with_client(&state, &source, since, &http, &server.uri()).await?;
        anyhow::ensure!(repeated.created == 0 && repeated.updated == 0);
        anyhow::ensure!(versions(&state, updated.id).await? == 3);

        // An actually empty current discussion must remove deleted comments;
        // concatenating the previous document would preserve stale content.
        *stage.lock().unwrap() = Stage::CommentsDeleted;
        let cleared =
            sync_github_issues_with_client(&state, &source, since, &http, &server.uri()).await?;
        anyhow::ensure!(cleared.updated == 1);
        let (after_clear, clear_body) = document(&state, &source).await?;
        anyhow::ensure!(after_clear.id == original.id && clear_body.contains("Updated issue"));
        anyhow::ensure!(
            !clear_body.contains("The original conclusion")
                && !clear_body.contains("The corrected explanation")
        );
        anyhow::ensure!(versions(&state, updated.id).await? == 4);

        let requests = server.received_requests().await.unwrap();
        anyhow::ensure!(requests.iter().all(|request| request
            .headers
            .get("authorization")
            .is_some_and(|value| value == "Bearer fixture-token")));
        anyhow::ensure!(
            !requests
                .iter()
                .any(|request| request.url.path().contains("/issues/20/")),
            "excluded pull requests must not consume comment or event requests"
        );
        let issue_requests: Vec<_> = requests
            .iter()
            .filter(|request| request.url.path() == "/repos/test/repo/issues")
            .collect();
        anyhow::ensure!(issue_requests.len() == 6);
        for (index, request) in issue_requests.iter().enumerate() {
            let lower_bound = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "since")
                .map(|(_, value)| time(&value));
            anyhow::ensure!(lower_bound == if index == 0 { None } else { since });
        }
        anyhow::ensure!(
            requests
                .iter()
                .filter(|request| request.url.path().ends_with("/comments"))
                .all(|request| !request.url.query_pairs().any(|(key, _)| key == "since")),
            "comments must be complete even while the issue list remains incremental"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
