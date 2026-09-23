//! A person clicking Review should never wait indefinitely behind a background
//! materialization. The human entry point (`materialize_human`) sets a 2-second
//! `lock_timeout` and maps 55P03 to `AppError::CodedConflict { code: "alignment_busy" }`.
//!
//! The worker entry point (`materialize`) keeps its existing unbounded wait —
//! workers don't have a spinner to look at. This test pins the *human* half only.
//!
//! `#798` survey follow-up; same pattern as `#828`'s `decide_and_apply_human`.

use sqlx::{postgres::PgPoolOptions, PgPool};
use utopia_store::{materialize, phrase_bindings};
use uuid::Uuid;

async fn wait_for_blocked_descendants(
    pool: &PgPool,
    blocker: i32,
    count: i64,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "WITH RECURSIVE blocked(pid) AS (
                    SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))
                    UNION SELECT p.pid FROM pg_stat_activity p JOIN blocked b ON b.pid=ANY(pg_blocking_pids(p.pid))
                ) SELECT count(*) FROM blocked",
            )
            .bind(blocker)
            .fetch_one(pool)
            .await?;
            if blocked >= count {
                return anyhow::Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    Ok(())
}

/// A human materialization that hits a worker-held advisory lock returns
/// CodedConflict("alignment_busy") within a couple of seconds, not after the
/// worker releases naturally.
#[tokio::test]
async fn human_materialization_against_a_held_worker_lock_returns_alignment_busy(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let control = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&control).await?;
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
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'human-materialize-budget')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'human-materialize-budget')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'human-materialize-budget')")
        .bind(kb).bind(ws).execute(&pool).await?;
    sqlx::query(
        "INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,'Acme'),($3,$2,'London')",
    )
    .bind(subject)
    .bind(kb)
    .bind(object)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES($1,$2,'based_in','based in','state')")
        .bind(property).bind(kb).execute(&pool).await?;
    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES($1,$2,$3,$4,'open','based in')")
        .bind(statement).bind(kb).bind(subject).bind(object).execute(&pool).await?;
    let signature = phrase_bindings::signatures(&pool, kb).await?.remove(0);
    phrase_bindings::decide(
        &pool,
        kb,
        &signature,
        phrase_bindings::Decision {
            relation_type_id: Some(property),
            direction: Some("forward"),
            status: "bound",
            votes: &serde_json::json!({}),
            decided_by: "agent",
        },
    )
    .await?;

    // Hold the typed_materialize advisory lock on a separate connection so the
    // human materialization has to wait for it. We DO NOT commit until the test
    // asserts the human request has already returned 409 with alignment_busy.
    let mut gate = pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))")
        .bind(kb.to_string())
        .execute(&mut *gate)
        .await?;
    let human_pool = pool.clone();
    let human_kb = kb;
    let human = tokio::spawn(async move {
        // Allow up to 10s wall-clock. Without the fix this would still be
        // running at 10s and the spawned task would not have returned.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            materialize::materialize_human(&human_pool, human_kb),
        )
        .await
    });
    // The human materialization is on a different connection but the same
    // pool — pool size is 2 so it should queue on `gate` and reach the lock
    // wait within a few hundred ms.
    wait_for_blocked_descendants(&control, blocker, 1).await?;
    let result = human.await??;
    let err = result.expect_err("the human materialization must have hit lock_timeout");
    let utopia_core::AppError::CodedConflict { code, message } = err else {
        panic!("expected CodedConflict, got: {err:?}");
    };
    assert_eq!(code, "alignment_busy");
    assert!(message.contains("try again"), "message: {message}");

    // Now release the gate and confirm a *worker* materialize (no budget)
    // completes normally on this base.
    gate.commit().await?;
    let outcome = materialize::materialize(&pool, kb).await?;
    assert_eq!(
        outcome.added, 1,
        "the bound statement should produce one typed fact"
    );
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    control.close().await;
    Ok(())
}

/// When the typed_materialize lock is free, materialize_human completes
/// normally (no regression on the worker side).
#[tokio::test]
async fn human_materialization_with_no_contention_matches_succeeds() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb, subject, object, property, statement) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'human-materialize-no-contention')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'human-materialize-no-contention')",
    )
    .bind(ws)
    .bind(org)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'human-materialize-no-contention')")
        .bind(kb).bind(ws).execute(&pool).await?;
    sqlx::query(
        "INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,'Acme'),($3,$2,'London')",
    )
    .bind(subject)
    .bind(kb)
    .bind(object)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES($1,$2,'based_in','based in','state')")
        .bind(property).bind(kb).execute(&pool).await?;
    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES($1,$2,$3,$4,'open','based in')")
        .bind(statement).bind(kb).bind(subject).bind(object).execute(&pool).await?;
    let signature = phrase_bindings::signatures(&pool, kb).await?.remove(0);
    phrase_bindings::decide(
        &pool,
        kb,
        &signature,
        phrase_bindings::Decision {
            relation_type_id: Some(property),
            direction: Some("forward"),
            status: "bound",
            votes: &serde_json::json!({}),
            decided_by: "agent",
        },
    )
    .await?;

    let outcome = materialize::materialize_human(&pool, kb).await?;
    assert_eq!(outcome.added, 1, "one typed fact should be added");

    // The SET LOCAL inside materialize_human must not leak to the session —
    // otherwise the next caller on this connection inherits our 2-second
    // budget, which would be wrong for a worker. Capture the value before and
    // after, and assert they're equal. Postgres' session default is `0`
    // (wait forever) on a clean connection; we don't depend on that.
    let before: Option<String> = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_optional(&pool)
        .await?;
    // Run materialize_human once more — if SET LOCAL had leaked, this second
    // call would still see `2s` (which we never set at session level).
    let _ = materialize::materialize_human(&pool, kb).await?;
    let after: Option<String> = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_optional(&pool)
        .await?;
    assert_eq!(
        before, after,
        "lock_timeout should not leak out of materialize_human's transaction; before={before:?} after={after:?}"
    );

    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    Ok(())
}
