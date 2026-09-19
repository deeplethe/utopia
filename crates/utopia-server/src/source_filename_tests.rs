//! 文件名截断不能让正常的多字节标题掀掉整次来源同步。
use super::{filename_from_url, slugify, sync_source};
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

#[test]
fn source_filenames_end_on_character_boundaries() {
    assert_eq!(
        slugify(&format!("A{}", "中".repeat(30))),
        format!("A{}", "中".repeat(19))
    );
    let name = filename_from_url(
        &format!("https://example.com/a{}", "中".repeat(50)),
        "text/html",
    );
    assert_eq!(name, format!("example.com-a{}.html", "中".repeat(35)));
    assert_eq!(slugify(&"a".repeat(80)), "a".repeat(60));
}

#[tokio::test]
async fn a_long_unicode_title_does_not_stop_an_rss_sync() -> anyhow::Result<()> {
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
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'filename-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'filename-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'filename-test')")
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
    let server = MockServer::start().await;
    let title = format!("A{}", "中".repeat(30));
    let feed = format!(
        r#"<rss version="2.0"><channel><title>Fixture</title><link>https://example.com/</link><description>Fixture</description><item><guid>unicode</guid><title>{title}</title><description>First article</description></item><item><guid>after</guid><title>Next article</title><description>Second article</description></item></channel></rss>"#
    );
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(feed))
        .mount(&server)
        .await;
    sqlx::query("INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,'rss','fixture',$3)")
        .bind(source)
        .bind(kb)
        .bind(serde_json::json!({"feed_url":server.uri()}))
        .execute(&pool)
        .await?;
    let dir = std::env::temp_dir().join(format!("utopia-source-filename-{source}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let result = tokio::spawn(async move { sync_source(&state, source).await }).await;
    let docs: Vec<String> =
        sqlx::query_scalar("SELECT filename FROM documents WHERE source_id=$1 ORDER BY filename")
            .bind(source)
            .fetch_all(&pool)
            .await?;
    let status: String = sqlx::query_scalar("SELECT last_sync_status FROM sources WHERE id=$1")
        .bind(source)
        .fetch_one(&pool)
        .await?;
    sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    let _ = std::fs::remove_dir_all(dir);
    result??;
    assert_eq!(
        docs,
        vec![
            format!("A{}.html", "中".repeat(19)),
            "Next-article.html".into()
        ]
    );
    assert_eq!(status, "ok");
    Ok(())
}
