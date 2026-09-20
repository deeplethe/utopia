//! Isolated protocol experiment; no production job kind or HTTP route is registered.
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, Instant};
use utopia_store::{jobs, materialize, phrase_bindings};
use uuid::Uuid;

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
            }
        )
        .await?
    );
    let id = jobs::enqueue_with_max_attempts_tx(
        &mut tx,
        "prototype_materialize_typed",
        json!({"kb_id":kb}),
        budget,
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}
async fn claim(pool: &PgPool, id: i64) -> anyhow::Result<jobs::Job> {
    // Restrict the production claim SQL to this experiment's job, never steal work.
    Ok(sqlx::query_as("UPDATE jobs SET status='running', attempts=attempts+1, locked_at=now() WHERE id=$1 AND status='queued' RETURNING id,kind,payload,attempts,max_attempts")
        .bind(id).fetch_one(pool).await?)
}
async fn handle(pool: &PgPool, kb: Uuid, job: &jobs::Job) -> anyhow::Result<()> {
    if materialize::try_materialize(pool, kb).await?.is_some() {
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
async fn delivery_protocol_rollback_busy_late_arrivals_recovery_and_cost() -> anyhow::Result<()> {
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
        // Invalid enqueue budget is a real helper failure after the decision write.
        anyhow::ensure!(accept(&pool,kb,&signature,Some(property),0).await.is_err());
        anyhow::ensure!(phrase_bindings::bindings(&pool,kb).await?.is_empty());
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1").bind(kb.to_string()).fetch_one(&pool).await?;
        anyhow::ensure!(count==0);
        println!("B-T01/T02 PASS same-transaction enqueue failure rolls back decision");

        let mut blocker=control.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('typed_materialize'),hashtext($1))").bind(kb.to_string()).execute(&mut *blocker).await?;
        let start=Instant::now();
        let id=tokio::time::timeout(Duration::from_secs(2),accept(&pool,kb,&signature,Some(property),3)).await??;
        let job=claim(&pool,id).await?;
        tokio::time::timeout(Duration::from_secs(2),handle(&pool,kb,&job)).await??;
        anyhow::ensure!(status(&pool,id).await?=="queued");
        for _ in 0..10 { anyhow::ensure!(tokio::time::timeout(Duration::from_secs(1),materialize::try_materialize(&pool,kb)).await??.is_none()); }
        anyhow::ensure!(tokio::time::timeout(Duration::from_secs(1),pool.acquire()).await?.is_ok());
        println!("B-T04/T07/T21 PASS busy deferred same job; two-connection pool available; elapsed_ms={}",start.elapsed().as_millis());
        blocker.rollback().await?;
        handle(&pool,kb,&claim(&pool,id).await?).await?;
        anyhow::ensure!(materialize::count(&pool,kb).await?==1);

        // A decision after the older materializer's last read has its own job.
        // Pause before ack by not acking the first completed projection yet.
        let old=accept(&pool,kb,&signature,Some(property),3).await?;
        let old_job=claim(&pool,old).await?;
        anyhow::ensure!(materialize::try_materialize(&pool,kb).await?.is_some());
        let newer=accept(&pool,kb,&signature,None,3).await?;
        anyhow::ensure!(status(&pool,newer).await?=="queued");
        handle(&pool,kb,&claim(&pool,newer).await?).await?;
        handle(&pool,kb,&old_job).await?;
        anyhow::ensure!(materialize::count(&pool,kb).await?==0);
        anyhow::ensure!(phrase_bindings::bindings(&pool,kb).await?[0].status=="none");
        let open:i64=sqlx::query_scalar("SELECT count(*) FROM facts WHERE id=$1 AND invalidated_at IS NULL").bind(statement).fetch_one(&pool).await?;
        anyhow::ensure!(open==1);
        println!("B-T05/T06/T09/T11/T14/T15 PASS late decision, reverse order and duplicate processing converge without replay");

        // Actual queue recovery (task restart, not an OS process crash).
        let recovery=accept(&pool,kb,&signature,Some(property),3).await?;
        let _unacked=claim(&pool,recovery).await?;
        materialize::try_materialize(&pool,kb).await?;
        let (sent,mut received)=tokio::sync::mpsc::unbounded_channel();
        let worker_pool=pool.clone();
        let run_pool=pool.clone();
        let worker=tokio::spawn(jobs::run_worker(worker_pool,std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(1)),move |job| {
            let pool=run_pool.clone(); let sent=sent.clone();
            async move {
                anyhow::ensure!(job.kind=="prototype_materialize_typed");
                anyhow::ensure!(materialize::try_materialize(&pool,kb).await?.is_some());
                sent.send(job.id)?;
                Ok(())
            }
        }));
        let result=tokio::time::timeout(Duration::from_secs(10),received.recv()).await;
        let acknowledged=if matches!(result,Ok(Some(x)) if x==recovery) {
            tokio::time::timeout(Duration::from_secs(5),async {
                while status(&pool,recovery).await? != "done" {tokio::task::yield_now().await;}
                anyhow::Ok(())
            }).await?
        } else { Err(anyhow::anyhow!("worker did not recover expected job: {result:?}")) };
        worker.abort(); let _=worker.await; acknowledged?;
        anyhow::ensure!(materialize::count(&pool,kb).await?==1);
        println!("B-T09/T12/T20 PASS actual run_worker startup recovery and idempotent recompute; no model path linked to handler; task-level restart only");

        // Deferred is bounded, then the normal finite failure budget takes over.
        let failure=accept(&pool,kb,&signature,None,1).await?;
        sqlx::query("UPDATE jobs SET payload=payload || jsonb_build_object('deferred_since',(now()-interval '1 day')::text) WHERE id=$1").bind(failure).execute(&pool).await?;
        let job=claim(&pool,failure).await?;
        jobs::mark_failed(&pool,&job,&anyhow::anyhow!("still busy").context(utopia_core::Deferred::new(Duration::from_secs(1)))).await?;
        anyhow::ensure!(status(&pool,failure).await?=="failed");
        anyhow::ensure!(jobs::requeue_failed(&pool,jobs::RequeueScope{kb_id:Some(kb),kind:Some("prototype_materialize_typed"),failed_since:None}).await?==1);
        handle(&pool,kb,&claim(&pool,failure).await?).await?;
        anyhow::ensure!(status(&pool,failure).await?=="done");
        println!("B-T08 PASS finite deferral exhaustion stays visible and can be explicitly requeued (not a process-crash test)");

        // Cost on a 100-statement graph. No listener or model is needed to find durable rows.
        for i in 1..100 {
            let subject=Uuid::now_v7();
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)").bind(subject).bind(kb).bind(format!("Entity {i}")).execute(&pool).await?;
            sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES($1,$2,$3,$4,'open','based in')").bind(Uuid::now_v7()).bind(kb).bind(subject).bind(object).execute(&pool).await?;
        }
        for n in [1,10,100] {
            let start=Instant::now(); let mut ids=Vec::new(); let mut max_ms=0;
            for _ in 0..n { let tick=Instant::now(); ids.push(accept(&pool,kb,&signature,Some(property),3).await?); max_ms=max_ms.max(tick.elapsed().as_micros()); }
            let accept_ms=start.elapsed().as_millis();
            for id in ids { handle(&pool,kb,&claim(&pool,id).await?).await?; }
            anyhow::ensure!(materialize::count(&pool,kb).await?==100);
            println!("B-T22 decisions={n} statements=100 jobs={n} recomputations={n} accept_total_ms={accept_ms} max_accept_us={max_ms} convergence_ms={}",start.elapsed().as_millis());
        }
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

// The integration-test executable is also a subprocess probe. This ignored entry
// runs only with explicit per-fixture environment from the parent, never in CI.
#[test]
#[ignore = "spawned only by the isolated crash-window experiment"]
fn crash_child() {
    let phase = std::env::var("UTOPIA_PROBE_PHASE").expect("explicit probe phase");
    let kb: Uuid = std::env::var("UTOPIA_PROBE_KB").unwrap().parse().unwrap();
    let property: Uuid = std::env::var("UTOPIA_PROBE_PROPERTY")
        .unwrap()
        .parse()
        .unwrap();
    let ready = std::env::var("UTOPIA_PROBE_READY").unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let pool = PgPool::connect(&utopia_store::test_db::url().unwrap())
            .await
            .unwrap();
        let signature = phrase_bindings::signatures(&pool, kb)
            .await
            .unwrap()
            .remove(0);
        let mut tx = pool.begin().await.unwrap();
        phrase_bindings::decide_on(
            &mut tx,
            kb,
            &signature,
            phrase_bindings::Decision {
                relation_type_id: Some(property),
                direction: Some("forward"),
                status: "bound",
                votes: &json!({}),
                decided_by: "person",
            },
        )
        .await
        .unwrap();
        let id = jobs::enqueue_with_max_attempts_tx(
            &mut tx,
            "prototype_materialize_typed",
            json!({"kb_id":kb}),
            3,
        )
        .await
        .unwrap();
        if phase == "uncommitted" {
            std::fs::write(&ready, id.to_string()).unwrap();
            std::future::pending::<()>().await;
        }
        tx.commit().await.unwrap();
        if phase == "unacked" {
            claim(&pool, id).await.unwrap();
            materialize::try_materialize(&pool, kb)
                .await
                .unwrap()
                .unwrap();
        }
        std::fs::write(&ready, id.to_string()).unwrap();
        std::future::pending::<()>().await;
    });
}

struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn process_exit_preserves_the_committed_delivery_boundary() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb, subject, object, property) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!("INSERT INTO organizations(id,name) VALUES('{org}','crash-probe');
      INSERT INTO workspaces(id,org_id,name) VALUES('{ws}','{org}','crash-probe');
      INSERT INTO knowledge_bases(id,workspace_id,name) VALUES('{kb}','{ws}','crash-probe');
      INSERT INTO entities(id,kb_id,canonical_name) VALUES('{subject}','{kb}','S'),('{object}','{kb}','O');
      INSERT INTO relation_types(id,kb_id,key,label) VALUES('{property}','{kb}','rel','Rel');
      INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES('{}','{kb}','{subject}','{object}','open','rel');",Uuid::now_v7())).execute(&pool).await?;
    let run=async {
        for phase in ["uncommitted","accepted","unacked"] {
            let ready=std::env::temp_dir().join(format!("utopia-crash-{}",Uuid::now_v7()));
            let mut child=ChildGuard(std::process::Command::new(std::env::current_exe()?)
                .args(["--exact","crash_child","--ignored","--nocapture"])
                .env("UTOPIA_PROBE_PHASE",phase).env("UTOPIA_PROBE_KB",kb.to_string())
                .env("UTOPIA_PROBE_PROPERTY",property.to_string()).env("UTOPIA_PROBE_READY",&ready)
                .spawn()?);
            let id=tokio::time::timeout(Duration::from_secs(15),async {
                loop {
                    if let Ok(value)=std::fs::read_to_string(&ready) {break value.parse::<i64>();}
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await??;
            child.0.kill()?;child.0.wait()?;
            let _=std::fs::remove_file(&ready);
            if phase=="uncommitted" {
                anyhow::ensure!(phrase_bindings::bindings(&pool,kb).await?.is_empty());
                let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE id=$1)").bind(id).fetch_one(&pool).await?;
                anyhow::ensure!(!exists);
            } else {
                anyhow::ensure!(phrase_bindings::bindings(&pool,kb).await?[0].status=="bound");
                anyhow::ensure!(status(&pool,id).await?==if phase=="accepted" {"queued"} else {"running"});
                // Exercise the same single-instance recovery update, scoped to
                // our job. Actual run_worker startup is tested separately above.
                sqlx::query("UPDATE jobs SET status='queued',locked_at=NULL WHERE id=$1 AND status='running'").bind(id).execute(&pool).await?;
                handle(&pool,kb,&claim(&pool,id).await?).await?;
                anyhow::ensure!(materialize::count(&pool,kb).await?==1);
                anyhow::ensure!(materialize::try_materialize(&pool,kb).await?==Some(materialize::Outcome::default()));
            }
            println!("OS process kill phase={phase}: PASS");
        }
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
    run
}
