use super::sync_github_issues_with_client;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

#[tokio::test]
async fn incremental_github_snapshots_keep_existing_comments() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, source) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'github-history')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'github-history')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'github-history')")
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;

    sqlx::query(
        "INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,'github_issues','fixture',$3)",
    )
    .bind(source)
    .bind(kb)
    .bind(serde_json::json!({"repo":"acme/project"}))
    .execute(&pool)
    .await?;
    let mut source = utopia_store::sources::get(&pool, source).await?;
    let dir = std::env::temp_dir().join(format!("utopia-github-history-{}", source.id));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let server = MockServer::start().await;
    let http = reqwest::Client::new();
    let mut snapshots = Vec::new();
    for round in 0..3 {
        server.reset().await;
        let old = serde_json::json!({"issue_url":"https://api.github.com/repos/acme/project/issues/1","user":{"login":"alice"},"created_at":"2026-01-01T01:00:00Z","body":"Original decision"});
        let new = serde_json::json!({"issue_url":"https://api.github.com/repos/acme/project/issues/1","user":{"login":"bob"},"created_at":"2026-01-03T01:00:00Z","body":"Follow-up discussion"});
        let all = if round == 0 {
            vec![old.clone()]
        } else {
            vec![old.clone(), new.clone()]
        };
        let incremental = match round {
            0 => vec![old],
            1 => vec![new],
            _ => vec![],
        };
        for (endpoint, body) in [
            (
                "issues",
                serde_json::json!([{"number":1,"title":format!("Issue revision {round}"),"state":"open","body":"Description","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-05T00:00:00Z"}]),
            ),
            ("issues/comments", serde_json::json!(incremental)),
            ("issues/1/comments", serde_json::json!(all)),
            ("issues/1/events", serde_json::json!([])),
        ] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/acme/project/{endpoint}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        let stats = sync_github_issues_with_client(&state, &source, &http, &server.uri()).await?;
        assert_eq!(
            (stats.created, stats.updated),
            if round == 0 { (1, 0) } else { (0, 1) }
        );
        let (id, sha): (Uuid, String) =
            sqlx::query_as("SELECT id,sha256 FROM documents WHERE source_id=$1")
                .bind(source.id)
                .fetch_one(&pool)
                .await?;
        snapshots.push((id, String::from_utf8(state.blob.get(&sha).await?)?));
        source.last_sync_at = Some(if round == 0 {
            "2026-01-02T00:00:00Z".parse()?
        } else {
            "2026-01-04T00:00:00Z".parse()?
        });
    }
    sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    let _ = std::fs::remove_dir_all(dir);
    for (round, (id, body)) in snapshots.iter().enumerate() {
        assert_eq!(*id, snapshots[0].0);
        assert!(
            body.contains("Original decision"),
            "round {round} lost old comment: {body}"
        );
        if round > 0 {
            assert!(
                body.contains("Follow-up discussion"),
                "round {round}: {body}"
            );
        }
    }
    Ok(())
}
