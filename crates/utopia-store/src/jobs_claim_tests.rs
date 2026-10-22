//! 认领顺序（#1300）：不调对话模型的任务排在模型任务前面，哪怕它来得晚。
//!
//! `claim_one` 认领的是全库的任务，所以这两条放在一百年前，排在库里别的一切前面。

use super::claim_one;
use sqlx::PgPool;

#[tokio::test]
async fn a_later_sync_is_claimed_before_an_earlier_extraction() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"INSERT INTO jobs (kind, payload, run_at) VALUES
               ('extract_document', '{"test": "claim-order"}', now() - interval '101 years'),
               ('sync_source',      '{"test": "claim-order"}', now() - interval '100 years')
           RETURNING id"#,
    )
    .fetch_all(&pool)
    .await?;

    let claimed = claim_one(&pool, false).await?.map(|job| job.kind);
    sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&pool)
        .await?;
    assert_eq!(claimed.as_deref(), Some("sync_source"));
    Ok(())
}
