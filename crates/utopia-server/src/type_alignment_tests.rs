//! A disputed kind-word binding no longer projects its old class onto entities.
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Model {
    replies: Arc<Vec<Value>>,
    requests: Arc<Mutex<Vec<Value>>>,
    hold: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    let n = {
        let mut seen = m.requests.lock().unwrap();
        seen.push(body);
        seen.len() - 1
    };
    if n == 0 && m.hold.load(std::sync::atomic::Ordering::SeqCst) {
        m.entered.notify_one();
        m.release.notified().await;
    }
    let text = m
        .replies
        .get(n)
        .expect("unexpected model request")
        .to_string();
    let frame = json!({"choices":[{"delta":{"content":text}}]});
    (
        [("content-type", "text/event-stream")],
        format!("data: {frame}\n\ndata: [DONE]\n\n"),
    )
}
struct Fx {
    pool: sqlx::PgPool,
    state: AppState,
    org: Uuid,
    kb: Uuid,
    class: Uuid,
    entity: Uuid,
    model: Model,
    server: tokio::task::JoinHandle<()>,
    dir: tempfile::TempDir,
}
impl Fx {
    async fn new(replies: Vec<Value>) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, class, entity) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'alignment-audit')")
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'alignment-audit')")
            .bind(ws)
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'alignment-audit')",
        )
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO entity_types(id,kb_id,key,label,description) VALUES($1,$2,'organization','Organization','OLD definition')").bind(class).bind(kb).execute(&pool).await?;
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,specific_type) VALUES($1,$2,'Acme','company')").bind(entity).bind(kb).execute(&pool).await?;
        let model = Model {
            replies: Arc::new(replies),
            requests: Arc::new(Mutex::new(Vec::new())),
            hold: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            entered: Arc::new(tokio::sync::Notify::new()),
            release: Arc::new(tokio::sync::Notify::new()),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(model.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        utopia_store::settings::upsert(
            &pool,
            ws,
            Some(&endpoint),
            None,
            Some("scripted"),
            None,
            None,
            None,
            None,
        )
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        Ok(Some(Self {
            pool,
            state,
            org,
            kb,
            class,
            entity,
            model,
            server,
            dir,
        }))
    }
    async fn run(&self) -> anyhow::Result<()> {
        align_types(&self.state, self.kb).await
    }
    fn requests(&self) -> Vec<Value> {
        self.model.requests.lock().unwrap().clone()
    }
    async fn cleanup(self) -> anyhow::Result<()> {
        self.server.abort();
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        drop(self.state);
        self.dir.close()?;
        Ok(())
    }
    async fn seed_bound(&self) -> anyhow::Result<()> {
        type_bindings::decide(
            &self.pool,
            self.kb,
            "company",
            &[],
            Some(self.class),
            "bound",
            &json!({}),
            "agent",
        )
        .await?;
        type_bindings::apply(&self.pool, self.kb, "company", self.class).await?;
        sqlx::query("UPDATE type_bindings SET decided_at='2000-01-01' WHERE kb_id=$1")
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
fn vote(class: Option<&str>) -> Value {
    json!({"b":[[0,class]]})
}

#[tokio::test]
async fn disagreement_retracts_previous_aligned_type() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![vote(Some("organization")), vote(None)]).await? else {
        return Ok(());
    };
    f.seed_bound().await?;
    let human = Uuid::now_v7();
    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,specific_type,type_id,type_source) VALUES($1,$2,'Human choice','company',$3,'human')")
        .bind(human).bind(f.kb).bind(f.class).execute(&f.pool).await?;
    // Downstream phrase signatures derive their subject type from the entity projection.
    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_value,layer,phrase) VALUES($1,$2,$3,'{\"value\":\"UK\"}','open','based in')")
        .bind(Uuid::now_v7()).bind(f.kb).bind(f.entity).execute(&f.pool).await?;
    f.run().await?;
    let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
    let projected: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(f.entity)
        .fetch_one(&f.pool)
        .await?;
    let downstream = utopia_store::phrase_bindings::signatures(&f.pool, f.kb)
        .await?
        .remove(0)
        .subject_type_id;
    let human_type: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(human)
        .fetch_one(&f.pool)
        .await?;
    assert_eq!(
        human_type,
        Some(f.class),
        "explicit human entity classification is preserved"
    );
    let count = f.requests().len();
    f.cleanup().await?;
    assert_eq!(count, 2);
    assert_eq!(binding.status, "undecided");
    assert_eq!(binding.type_id, None);
    assert_eq!(
        projected, None,
        "an undecided binding must not leave an aligned class on its entities"
    );
    assert_eq!(
        downstream, None,
        "phrase alignment must not consume the revoked class"
    );
    Ok(())
}

#[tokio::test]
async fn human_decision_during_disagreement_survives() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![vote(Some("organization")), vote(None)]).await? else {
        return Ok(());
    };
    f.model
        .hold
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let state = f.state.clone();
    let kb = f.kb;
    let worker = tokio::spawn(async move { align_types(&state, kb).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        f.model.entered.notified(),
    )
    .await?;
    type_bindings::decide(
        &f.pool,
        f.kb,
        "company",
        &[],
        Some(f.class),
        "bound",
        &json!({}),
        "person",
    )
    .await?;
    type_bindings::apply(&f.pool, f.kb, "company", f.class).await?;
    f.model.release.notify_one();
    worker.await??;
    let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
    let projected: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(f.entity)
        .fetch_one(&f.pool)
        .await?;
    let class = f.class;
    f.cleanup().await?;
    assert_eq!(binding.decided_by, "person");
    assert_eq!(binding.type_id, Some(class));
    assert_eq!(projected, Some(class));
    Ok(())
}

#[tokio::test]
async fn agreed_votes_keep_their_existing_behavior() -> anyhow::Result<()> {
    for key in [Some("organization"), None] {
        let Some(f) = Fx::new(vec![vote(key), vote(key)]).await? else {
            return Ok(());
        };
        f.seed_bound().await?;
        f.run().await?;
        let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
        let projected: Option<Uuid> =
            sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
                .bind(f.entity)
                .fetch_one(&f.pool)
                .await?;
        let expected = key.map(|_| f.class);
        f.cleanup().await?;
        assert_eq!(binding.type_id, expected);
        assert_eq!(projected, expected);
        assert_eq!(binding.status, if key.is_some() { "bound" } else { "none" });
    }
    Ok(())
}

// Pause the agent exactly at its entity write, using a PostgreSQL row lock.
// No wall-clock delay is used to choose which decision wins.
#[tokio::test]
async fn human_none_wins_when_agent_is_already_writing_its_projection() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![]).await? else {
        return Ok(());
    };
    let mut gate = f.pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await?;
    sqlx::query("SELECT id FROM entities WHERE id=$1 FOR UPDATE")
        .bind(f.entity)
        .fetch_one(&mut *gate)
        .await?;
    let pool = f.pool.clone();
    let kb = f.kb;
    let class = f.class;
    let agent = tokio::spawn(async move {
        type_bindings::decide_and_apply(
            &pool,
            kb,
            "company",
            &[],
            Some(class),
            "bound",
            &json!({}),
            "agent",
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                .bind(blocker).fetch_one(&f.pool).await?;
            if waiting { break; }
            tokio::task::yield_now().await;
        }
        anyhow::Ok(())
    }).await??;
    let pool = f.pool.clone();
    let person = tokio::spawn(async move {
        type_bindings::decide_and_apply(
            &pool,
            kb,
            "company",
            &[],
            None,
            "none",
            &json!({}),
            "person",
        )
        .await
    });
    // Before the fix the person can commit because no binding transaction holds
    // the row. With the fix the person waits for the agent's binding lock.
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            if person.is_finished() { break; }
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity p WHERE EXISTS (SELECT 1 FROM unnest(pg_blocking_pids(p.pid)) b(pid) WHERE $1=ANY(pg_blocking_pids(b.pid))))")
                .bind(blocker).fetch_one(&f.pool).await?;
            if waiting { break; }
            tokio::task::yield_now().await;
        }
        anyhow::Ok(())
    }).await??;
    gate.commit().await?;
    assert!(agent.await??);
    assert!(person.await??);
    let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
    let projected: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(f.entity)
        .fetch_one(&f.pool)
        .await?;
    f.cleanup().await?;
    assert_eq!(binding.decided_by, "person");
    assert_eq!(binding.status, "none");
    assert_eq!(
        projected, None,
        "the older agent must not undo the person's none decision"
    );
    Ok(())
}
