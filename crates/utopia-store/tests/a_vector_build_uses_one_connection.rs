//! A build already owns a connection; checking its index must not need another.
use std::time::Duration;
use utopia_store::vector_index::{self, Target};

#[tokio::test]
async fn a_build_finishes_with_only_one_connection_available() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    // Two is the application's supported minimum. The timeout only bounds a
    // broken nested acquire; no assertion depends on index-building latency.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(2))
        .connect(&url)
        .await?;
    utopia_store::db::migrate(&pool).await?;
    for (target, dims) in [(Target::Chunks, 17), (Target::EntityProfiles, 18)] {
        vector_index::drop(&pool, target, dims).await?;
        // Stand in for another request holding one connection. This makes the
        // pool pressure deterministic, without racing two build tasks.
        let busy = pool.acquire().await?;
        let run = async {
            let built = vector_index::build(&pool, target, dims).await?;
            assert!(built.created);
            assert_eq!(vector_index::status(&pool, target, dims).await?, Some(true));
            let again = vector_index::build(&pool, target, dims).await?;
            assert!(!again.created, "an existing valid index is reused");
            Ok::<_, anyhow::Error>(())
        }
        .await;
        drop(busy);
        vector_index::drop(&pool, target, dims).await?;
        run?;
    }
    pool.close().await;
    Ok(())
}
