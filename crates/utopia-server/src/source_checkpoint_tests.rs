//! 增量游标代表已成功覆盖的窗口，失败的尝试不能把尚未读到的条目跳过去。
use super::sync_source;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

async fn retry_reads_the_uncovered_window(
    prior_success: bool,
    fail_before_retry: bool,
) -> anyhow::Result<()> {
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
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'checkpoint-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'checkpoint-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'checkpoint-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    let server = MockServer::start().await;
    sqlx::query(
        "INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,'custom','fixture',$3)",
    )
    .bind(source)
    .bind(kb)
    .bind(serde_json::json!({"endpoint":server.uri()}))
    .execute(&pool)
    .await?;
    let started = chrono::Utc::now() - chrono::Duration::hours(2);
    let changed = started + chrono::Duration::minutes(30);
    if prior_success {
        let run = utopia_store::sources::start_run(&pool, source).await?;
        utopia_store::sources::finish_run(&pool, run, source, None, 0, 0).await?;
        sqlx::query("UPDATE source_sync_runs SET started_at=$2,finished_at=$3 WHERE id=$1")
            .bind(run)
            .bind(started)
            .bind(started + chrono::Duration::hours(1))
            .execute(&pool)
            .await?;
        utopia_store::sources::touch_sync_time(&pool, source, started + chrono::Duration::hours(1))
            .await?;
    }
    let dir = std::env::temp_dir().join(format!("utopia-checkpoint-{source}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let result = async {
        if fail_before_retry {
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
            assert!(sync_source(&state, source).await.is_err());
            let failed = utopia_store::sources::get(&pool, source).await?;
            assert_eq!(failed.last_sync_status, "failed");
            assert!(failed.last_sync_at.is_some(), "attempt time still drives scheduling and diagnostics");
            server.reset().await;
        }
        Mock::given(method("GET")).respond_with(move |request: &Request| {
            let since = request.url.query_pairs().find(|(k,_)| k == "since")
                .map(|(_,v)| chrono::DateTime::parse_from_rfc3339(&v).unwrap().with_timezone(&chrono::Utc));
            let items = if since.is_none_or(|s| s <= changed) {
                serde_json::json!([{"id":"missed", "title":"Missed update", "content":"An update from the uncovered window"}])
            } else { serde_json::json!([]) };
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":items}))
        }).mount(&server).await;
        sync_source(&state, source).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE source_id=$1")
            .bind(source).fetch_one(&pool).await?;
        anyhow::ensure!(count == 1, "incremental cursor skipped an unimported update: {count} documents");
        let ok = utopia_store::sources::get(&pool, source).await?;
        assert_eq!(ok.last_sync_status, "ok");
        Ok::<_, anyhow::Error>(())
    }.await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    drop(state);
    let _ = std::fs::remove_dir_all(dir);
    result
}

#[tokio::test]
async fn a_failed_first_sync_does_not_skip_the_existing_items() -> anyhow::Result<()> {
    retry_reads_the_uncovered_window(false, true).await
}

#[tokio::test]
async fn retry_keeps_updates_from_the_previous_successful_run_window() -> anyhow::Result<()> {
    retry_reads_the_uncovered_window(true, true).await
}

#[tokio::test]
async fn updates_during_a_successful_sync_remain_in_the_next_window() -> anyhow::Result<()> {
    retry_reads_the_uncovered_window(true, false).await
}
