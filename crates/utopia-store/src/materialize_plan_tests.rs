//! The recompute's whole-base statements are planned for the base they are given (#1298): a cached
//! statement switches to a generic plan after a few runs, and the generic plan of the source
//! cleanup ran for hours on a production base where the plan for the base's own id takes seconds.
use super::materialize;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn a_recompute_leaves_none_of_its_whole_base_statements_cached_on_its_connection(
) -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let base = Uuid::now_v7();

    materialize(&pool, base).await?;
    materialize(&pool, base).await?;
    let cached: Vec<String> = sqlx::query_scalar(
        "SELECT statement FROM pg_prepared_statements
          WHERE statement LIKE '%kb_id = $1%' AND statement NOT LIKE '%pg_prepared_statements%'",
    )
    .fetch_all(&pool)
    .await?;

    assert_eq!(cached, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn the_source_cleanup_plans_the_sources_that_hold_once_as_a_cte_the_delete_scans(
) -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;

    let plan_lines: Vec<String> =
        sqlx::query_scalar(&format!("EXPLAIN {}", super::source_cleanup_sql()))
            .persistent(false)
            .bind(Uuid::now_v7())
            .fetch_all(&pool)
            .await?;
    let plan = plan_lines.join("\n");

    assert!(
        plan.contains("CTE holding") && plan.contains("CTE Scan on holding"),
        "the delete must anti-join a CTE built once, not rescan the binding match per source, got:\n{plan}"
    );
    Ok(())
}
