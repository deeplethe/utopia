//! 认领顺序（#1300、#1351）：不调对话模型的任务排在模型任务前面，哪怕它来得晚。
//!
//! `claim_one` 认领的是全库的任务，所以这两条放在一百年前，排在库里别的一切前面。

use super::claim_one;
use sqlx::{Connection, PgConnection, PgPool};

#[tokio::test]
async fn a_later_intake_job_is_claimed_before_an_earlier_extraction() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    for kind in ["sync_source", crate::phrase_bindings::MATERIALIZE_KIND] {
        let ids: Vec<i64> = sqlx::query_scalar(
            r#"INSERT INTO jobs (kind, payload, run_at) VALUES
               ('extract_document', '{"test": "claim-order"}', now() - interval '101 years'),
               ($1,                 '{"test": "claim-order"}', now() - interval '100 years')
           RETURNING id"#,
        )
        .bind(kind)
        .fetch_all(&pool)
        .await?;

        let claimed = claim_one(&pool, false).await?.map(|job| job.kind);
        sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&pool)
            .await?;
        assert_eq!(claimed.as_deref(), Some(kind));
    }
    Ok(())
}

#[tokio::test]
async fn an_upgrade_reindexes_already_queued_materialization() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let schema = format!("claim_upgrade_{}", uuid::Uuid::now_v7().simple());
    let result: anyhow::Result<Vec<String>> = async {
        let mut tx = pool.begin().await?;
        // 旧函数和旧索引只装进这个 schema，不改并发测试正在用的队列。
        sqlx::raw_sql(&format!(
            "CREATE SCHEMA {schema};
             SET LOCAL search_path TO {schema};
             CREATE TABLE jobs (kind text, run_at timestamptz, status text DEFAULT 'queued');"
        ))
        .execute(&mut *tx)
        .await?;
        sqlx::raw_sql(include_str!(
            "../../../migrations/0116_intake_jobs_are_claimed_before_model_work.sql"
        ))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO jobs (kind, run_at) VALUES
               ('extract_document', now() - interval '1 day'),
               ($1, now())",
        )
        .bind(crate::phrase_bindings::MATERIALIZE_KIND)
        .execute(&mut *tx)
        .await?;
        // 旧数据在升级前已经提交；新进程迁移时没有旧函数的内联计划缓存。
        tx.commit().await?;
        let mut upgrade = PgConnection::connect(&url).await?;
        sqlx::raw_sql(&format!(
            "SET search_path TO {schema}; {}",
            include_str!("../../../migrations/0123_human_phrase_materialization_is_intake.sql")
        ))
        .execute(&mut upgrade)
        .await?;
        upgrade.close().await?;

        // 认领由升级后的新 worker 连接执行，直接验证已有队列的索引键。
        let mut fresh = PgConnection::connect(&url).await?;
        sqlx::raw_sql(&format!(
            "SET search_path TO {schema};
             SET enable_seqscan = off; SET enable_bitmapscan = off;"
        ))
        .execute(&mut fresh)
        .await?;
        let eligible = sqlx::query_scalar(
            "SELECT kind FROM jobs WHERE status = 'queued' AND job_priority(kind) = 0
             ORDER BY job_priority(kind), run_at",
        )
        .fetch_all(&mut fresh)
        .await?;
        fresh.close().await?;
        Ok(eligible)
    }
    .await;

    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&pool)
        .await?;
    assert_eq!(result?, vec![crate::phrase_bindings::MATERIALIZE_KIND]);
    Ok(())
}
