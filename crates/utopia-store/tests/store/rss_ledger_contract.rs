//! Required-DB acceptance for the unreleased RSS discovery contract.
use sqlx::PgPool;

#[tokio::test]
async fn rss_has_one_observation_table_and_typed_source_activation() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    utopia_store::db::migrate(&pool).await?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name FROM information_schema.tables
         WHERE table_schema = 'public' AND table_name LIKE 'rss_full_content_%'
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(tables, vec!["rss_full_content_entries"]);
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'sources'
           AND column_name IN ('rss_generation', 'rss_baselined_at')
         ORDER BY column_name",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(columns, vec!["rss_baselined_at", "rss_generation"]);
    let redundant: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_name='rss_full_content_entries' AND column_name IN ('state','attempt_count','error_code','error_detail','content_source','content_sha256','completed_at','document_id')").fetch_one(&pool).await?;
    assert_eq!(
        redundant, 0,
        "execution and document state must not be persisted twice"
    );
    let kind: String = sqlx::query_scalar("SELECT column_name FROM information_schema.columns WHERE table_name='rss_full_content_entries' AND column_name='entry_kind'").fetch_one(&pool).await?;
    assert_eq!(kind, "entry_kind");
    Ok(())
}
