//! Two materializers must not create two typed facts from the same statement.
use sqlx::{postgres::PgPoolOptions, PgPool};
use utopia_store::{materialize, phrase_bindings};
use uuid::Uuid;

async fn blocked_descendants(pool: &PgPool, blocker: i32) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar("WITH RECURSIVE blocked(pid) AS (
        SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))
        UNION SELECT p.pid FROM pg_stat_activity p JOIN blocked b ON b.pid=ANY(pg_blocking_pids(p.pid))
    ) SELECT count(*) FROM blocked")
        .bind(blocker).fetch_one(pool).await?)
}
async fn wait_for(pool: &PgPool, blocker: i32, count: i64) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while blocked_descendants(pool, blocker).await? < count {
            tokio::task::yield_now().await;
        }
        anyhow::Ok(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn overlapping_materializations_create_one_typed_fact() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let control = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&control).await?;
    // Exactly two connections for two competing materializers. The gate/observer
    // uses a separate pool so it does not affect the workers' connection budget.
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
    let name = format!("materialize_gate_{}", kb.simple());
    sqlx::query(&format!("CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
        PERFORM pg_advisory_xact_lock(hashtext('materialize-test'),hashtext(TG_ARGV[0])); RETURN NEW;
        END $$")).execute(&control).await?;
    sqlx::query(&format!(
        "CREATE TRIGGER {name} BEFORE INSERT ON facts FOR EACH ROW
        WHEN (NEW.kb_id='{kb}'::uuid AND NEW.layer='typed') EXECUTE FUNCTION {name}('{kb}')"
    ))
    .execute(&control)
    .await?;
    let run=async {
        let mut gate=control.begin().await?;
        let blocker:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *gate).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('materialize-test'),hashtext($1))")
            .bind(kb.to_string()).execute(&mut *gate).await?;
        let a_pool=pool.clone();
        let a=tokio::spawn(async move { materialize::materialize(&a_pool,kb).await });
        wait_for(&control,blocker,1).await?;
        let b_pool=pool.clone();
        let b=tokio::spawn(async move { materialize::materialize(&b_pool,kb).await });
        // Old code: both insert triggers wait on the gate. Fixed code: the second
        // materializer waits on the first, which waits on the gate.
        wait_for(&control,blocker,2).await?;
        gate.commit().await?;
        a.await??; b.await??;
        let live=materialize::count(&pool,kb).await?;
        let sources:i64=sqlx::query_scalar("SELECT count(*) FROM typed_fact_sources WHERE statement_id=$1").bind(statement).fetch_one(&pool).await?;
        anyhow::ensure!(live == 1,"overlapping runs produced {live} live typed facts; expected one");
        anyhow::ensure!(sources == 1,"the statement has {sources} typed projections; expected one");
        // One connection is enough for a subsequent materialization, including
        // the nested graph/temporal operations.
        let one=PgPoolOptions::new().max_connections(1).connect(&url).await?;
        anyhow::ensure!(tokio::time::timeout(std::time::Duration::from_secs(10),materialize::materialize(&one,kb)).await?? == materialize::Outcome::default());
        // A later statement closes the existing fact. This exercises a nested
        // temporal transaction on that same single connection (a savepoint).
        sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase,valid_to,valid_to_precision) VALUES($1,$2,$3,$4,'open','based in','2020-01-01','day')")
            .bind(Uuid::now_v7()).bind(kb).bind(subject).bind(object).execute(&one).await?;
        let closed=tokio::time::timeout(std::time::Duration::from_secs(10),materialize::materialize(&one,kb)).await??;
        anyhow::ensure!(closed.added == 1,"the closing statement should create one corrected fact");
        anyhow::ensure!(materialize::count(&one,kb).await? == 1);
        let ended:bool=sqlx::query_scalar("SELECT valid_to='2020-01-01'::timestamptz FROM facts WHERE kb_id=$1 AND layer='typed' AND invalidated_at IS NULL")
            .bind(kb).fetch_one(&one).await?;
        anyhow::ensure!(ended,"the temporal rewrite must commit with the projection");
        anyhow::ensure!(materialize::materialize(&one,kb).await? == materialize::Outcome::default());
        one.close().await;
        anyhow::Ok(())
    }.await;
    sqlx::query(&format!("DROP TRIGGER {name} ON facts"))
        .execute(&control)
        .await?;
    sqlx::query(&format!("DROP FUNCTION {name}()"))
        .execute(&control)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&control)
        .await?;
    pool.close().await;
    control.close().await;
    run
}
