//! Opt-in: requires a dedicated, otherwise idle migrated database.
//! UTOPIA_TEST_REQUIRE_DB=1 cargo test -p utopia-store --lib materialize::delivery_tests::busy_defers_without_retaining_connections -- --ignored --test-threads=1 --nocapture
use super::{materialize_in_tx, Outcome};
use crate::{jobs, materialize, phrase_bindings};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, Instant};
use utopia_core::AppResult;
use uuid::Uuid;

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
// Test-only adapter: use the real body after acquiring the production lock.
async fn try_materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<Option<Outcome>> {
    let mut tx = pool.begin().await?;
    let acquired: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))",
    )
    .bind(kb_id.to_string())
    .fetch_one(&mut *tx)
    .await?;
    if !acquired {
        tx.rollback().await?;
        return Ok(None);
    }
    let outcome = materialize_in_tx(&mut tx, kb_id).await?;
    tx.commit().await?;
    Ok(Some(outcome))
}

async fn accept(
    pool: &PgPool,
    kb: Uuid,
    sig: &phrase_bindings::PhraseSignature,
    property: Option<Uuid>,
    budget: i32,
) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await?;
    anyhow::ensure!(
        phrase_bindings::decide_on(
            &mut tx,
            kb,
            sig,
            phrase_bindings::Decision {
                relation_type_id: property,
                direction: property.map(|_| "forward"),
                status: if property.is_some() { "bound" } else { "none" },
                votes: &json!({}),
                decided_by: "person",
                basis: None,
            }
        )
        .await?
    );
    let id = jobs::enqueue_with_max_attempts_tx(
        &mut tx,
        "test_human_phrase_materialize",
        json!({"kb_id":kb}),
        budget,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}
async fn claim(pool: &PgPool, id: i64) -> anyhow::Result<jobs::Job> {
    // Restrict the production claim SQL to this test's job, never steal work.
    Ok(sqlx::query_as("UPDATE jobs SET status='running', attempts=attempts+1, locked_at=now() WHERE id=$1 AND status='queued' RETURNING id,kind,payload,attempts,max_attempts")
        .bind(id).fetch_one(pool).await?)
}
async fn handle(pool: &PgPool, kb: Uuid, job: &jobs::Job) -> anyhow::Result<()> {
    if try_materialize(pool, kb).await?.is_some() {
        sqlx::query("UPDATE jobs SET status='done',last_error=NULL WHERE id=$1")
            .bind(job.id)
            .execute(pool)
            .await?;
    } else {
        let e = anyhow::anyhow!("typed projection busy")
            .context(utopia_core::Deferred::new(Duration::from_secs(1)));
        jobs::mark_failed(pool, job, &e).await?;
    }
    Ok(())
}
async fn status(pool: &PgPool, id: i64) -> anyhow::Result<String> {
    Ok(sqlx::query_scalar("SELECT status FROM jobs WHERE id=$1")
        .bind(id)
        .fetch_one(pool)
        .await?)
}

#[tokio::test]
#[ignore = "requires a dedicated idle database; observes a blocked production call"]
async fn busy_defers_without_retaining_connections() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let control = PgPool::connect(&url).await?;
    crate::db::migrate(&control).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    let (org, ws, kb, subject, object, property, statement) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'materialize-race')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'materialize-race')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'materialize-race')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    for (id, name) in [(subject, "Acme"), (object, "London")] {
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(&pool)
            .await?;
    }
    sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES($1,$2,'based_in','based in','state')").bind(property).bind(kb).execute(&pool).await?;
    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES($1,$2,$3,$4,'open','based in')").bind(statement).bind(kb).bind(subject).bind(object).execute(&pool).await?;
    let signature = phrase_bindings::signatures(&pool, kb).await?.remove(0);
    let run=async {
        let mut blocker=control.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('typed_materialize'),hashtext($1))").bind(kb.to_string()).execute(&mut *blocker).await?;
        let start=Instant::now();
        let id=tokio::time::timeout(Duration::from_secs(2),accept(&pool,kb,&signature,Some(property),3)).await??;
        let job=claim(&pool,id).await?;
        tokio::time::timeout(Duration::from_secs(2),handle(&pool,kb,&job)).await??;
        anyhow::ensure!(status(&pool,id).await?=="queued");
        for _ in 0..10 { anyhow::ensure!(tokio::time::timeout(Duration::from_secs(1),try_materialize(&pool,kb)).await??.is_none()); }
        anyhow::ensure!(tokio::time::timeout(Duration::from_secs(1),pool.acquire()).await?.is_ok());
        println!("B-T04/T07/T21 PASS busy deferred same job; two-connection pool available; elapsed_ms={}",start.elapsed().as_millis());
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *blocker).await?;
        let production_pool = pool.clone();
        let production = tokio::spawn(async move { super::materialize(&production_pool, kb).await });
        let mut production = AbortOnDrop(production);
        let observed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query LIKE '%pg_advisory_xact_lock%' AND wait_event_type='Lock')")
                    .bind(blocker_pid).fetch_one(&control).await?;
                if waiting { break anyhow::Ok(()); }
                tokio::task::yield_now().await;
            }
        }).await;
        if !matches!(observed, Ok(Ok(()))) {
            blocker.rollback().await?;
            production.0.abort(); let _ = (&mut production.0).await;
            anyhow::bail!("production materialize did not wait on the test lock: {observed:?}");
        }
        println!("production materialize observed blocked by pg_blocking_pids");
        blocker.rollback().await?;
        tokio::time::timeout(Duration::from_secs(5), &mut production.0).await???;

        handle(&pool,kb,&claim(&pool,id).await?).await?;
        anyhow::ensure!(materialize::count(&pool,kb).await?==1);

        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    control.close().await;
    run
}
