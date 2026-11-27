//! Queue depth for telemetry: one grouped query says how many jobs sit in which base, lane
//! and status, each row dated by its oldest `run_at`. Every test builds and removes its own
//! base, and is skipped without `UTOPIA_DATABASE_URL`.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use utopia_store::jobs;
use uuid::Uuid;

type Job = (&'static str, Value, DateTime<Utc>);

fn at(date: &str) -> DateTime<Utc> {
    format!("{date}T00:00:00Z").parse().unwrap()
}

/// Queue the jobs `make` builds from the base's (kb, document, source) ids; the base's rows,
/// as (lane, status, count, oldest), must equal `want`.
async fn check<const N: usize>(
    make: impl FnOnce(Uuid, Uuid, Uuid) -> Vec<Job>,
    want: [(i16, &str, i64, DateTime<Utc>); N],
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let [org, ws, kb, doc, source] = [(); 5].map(|_| Uuid::now_v7());
    sqlx::query(
        "WITH o AS (INSERT INTO organizations (id, name) VALUES ($1, 'queue-test')),
              w AS (INSERT INTO workspaces (id, org_id, name) VALUES ($2, $1, 'queue-test')),
              k AS (INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($3, $2, 'queue-test')),
              s AS (INSERT INTO sources (id, kb_id, name) VALUES ($4, $3, 'queue-test'))
         INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($5, $3, 'q.txt', 'q')",
    )
    .bind(org)
    .bind(ws)
    .bind(kb)
    .bind(source)
    .bind(doc)
    .execute(&pool)
    .await?;
    let run = async {
        for (status, payload, run_at) in make(kb, doc, source) {
            sqlx::query(
                "INSERT INTO jobs (kind, payload, status, run_at) VALUES ('noop', $1, $2, $3)",
            )
            .bind(payload)
            .bind(status)
            .bind(run_at)
            .execute(&pool)
            .await?;
        }
        let mut rows = jobs::queue_depth_by_base(&pool).await?;
        rows.retain(|row| row.kb_id == Some(kb));
        rows.sort_by(|a, b| (a.lane, &a.status).cmp(&(b.lane, &b.status)));
        Ok::<_, anyhow::Error>(rows)
    }
    .await;
    let _ = sqlx::query(
        "DELETE FROM jobs WHERE payload->>'kb_id' IN ($1::text, 'not-a-uuid')
            OR payload->>'document_id' = $2::text OR payload->>'source_id' = $3::text",
    )
    .bind(kb)
    .bind(doc)
    .bind(source)
    .execute(&pool)
    .await;
    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await;
    let rows = run?;
    let got: Vec<_> = rows
        .iter()
        .map(|row| (row.lane, row.status.as_str(), row.count, row.oldest))
        .collect();
    assert_eq!(got, want);
    Ok(())
}

#[tokio::test]
async fn two_queued_jobs_in_one_base_count_as_two() -> anyhow::Result<()> {
    let make = |kb, _, _| vec![("queued", json!({ "kb_id": kb }), at("2020-01-01")); 2];
    check(make, [(0, "queued", 2, at("2020-01-01"))]).await
}

#[tokio::test]
async fn the_oldest_of_two_queued_jobs_dates_the_row() -> anyhow::Result<()> {
    let make = |kb, _, _| {
        ["2021-06-01", "2020-01-01", "2999-01-01"]
            .map(|date| ("queued", json!({ "kb_id": kb }), at(date)))
            .to_vec()
    };
    check(make, [(0, "queued", 2, at("2020-01-01"))]).await
}

#[tokio::test]
async fn a_queued_job_finds_its_base_by_document_or_source_and_done_is_left_out(
) -> anyhow::Result<()> {
    let make = |kb, doc, source| {
        vec![
            ("running", json!({ "kb_id": kb }), at("2020-01-01")),
            ("done", json!({ "kb_id": kb }), at("2020-01-01")),
            ("queued", json!({ "document_id": doc }), at("2020-01-01")),
            ("queued", json!({ "source_id": source }), at("2020-01-01")),
        ]
    };
    let day = at("2020-01-01");
    check(make, [(0, "queued", 2, day), (0, "running", 1, day)]).await
}

#[tokio::test]
async fn a_job_whose_kb_id_is_not_a_uuid_does_not_break_the_count() -> anyhow::Result<()> {
    let make = |kb, _, _| {
        ["queued", "queued"]
            .into_iter()
            .zip([json!({ "kb_id": kb }), json!({ "kb_id": "not-a-uuid" })])
            .map(|(status, payload)| (status, payload, at("2020-01-01")))
            .collect()
    };
    check(make, [(0, "queued", 1, at("2020-01-01"))]).await
}
