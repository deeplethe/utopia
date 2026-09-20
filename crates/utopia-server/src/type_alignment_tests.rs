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

// Exercise the public route, including authentication and its success side effects.
mod review_locks {
    use super::*;
    use axum::http::StatusCode;
    use std::time::Duration;
    use tower::ServiceExt;

    async fn editor(f: &Fx) -> anyhow::Result<(Uuid, String)> {
        let user = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES($1,$2,$3,'unused','Lock test')")
            .bind(user).bind(f.org).bind(format!("{user}@example.test")).execute(&f.pool).await?;
        sqlx::query("INSERT INTO kb_members(kb_id,user_id,role) VALUES($1,$2,'editor')")
            .bind(f.kb)
            .bind(user)
            .execute(&f.pool)
            .await?;
        Ok((user, crate::auth::issue_token(&f.state, user)?))
    }

    async fn request(
        state: AppState,
        kb: Uuid,
        token: &str,
        class: Option<&str>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri(format!(
                "/api/v1/kbs/{kb}/review/alignment/kind-words/company"
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(axum::body::Body::from(json!({"class":class}).to_string()))?;
        let response = crate::api::router(state, &Default::default())
            .oneshot(request)
            .await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 4096).await?;
        Ok((status, serde_json::from_slice(&body)?))
    }

    async fn snapshot(f: &Fx) -> anyhow::Result<Value> {
        let binding: Value =
            sqlx::query_scalar("SELECT to_jsonb(b) FROM type_bindings b WHERE kb_id=$1")
                .bind(f.kb)
                .fetch_one(&f.pool)
                .await?;
        let entities: Value = sqlx::query_scalar(
            "SELECT jsonb_agg(to_jsonb(e) ORDER BY id) FROM entities e WHERE kb_id=$1",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(f.kb.to_string())
            .fetch_one(&f.pool)
            .await?;
        let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE kb_id=$1 AND action='alignment.kind_word_decided'")
            .bind(f.kb).fetch_one(&f.pool).await?;
        Ok(json!({"binding":binding,"entities":entities,"jobs":jobs,"audit":audit}))
    }

    async fn wait_for_lock(pool: &sqlx::PgPool, blocker: i32) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                    .bind(blocker).fetch_one(pool).await?;
                if waiting { return anyhow::Ok(()); }
                tokio::task::yield_now().await;
            }
        }).await?
    }

    // A single-connection request pool proves reuse, rather than accidentally
    // checking a different connection whose session settings were never changed.
    async fn request_pool() -> anyhow::Result<sqlx::PgPool> {
        Ok(sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|c, _| {
                Box::pin(async move {
                    sqlx::query("SET lock_timeout = '7s'").execute(c).await?;
                    Ok(())
                })
            })
            .connect(&utopia_store::test_db::url().expect("fixture has a database"))
            .await?)
    }

    async fn session(pool: &sqlx::PgPool) -> anyhow::Result<(i32, String)> {
        Ok(
            sqlx::query_as("SELECT pg_backend_pid(), current_setting('lock_timeout')")
                .fetch_one(pool)
                .await?,
        )
    }

    async fn contention(binding_lock: bool, unapply: bool) -> anyhow::Result<()> {
        let Some(f) = Fx::new(vec![]).await? else {
            return Ok(());
        };
        let result = async {
            f.seed_bound().await?;
            let human = Uuid::now_v7();
            let other = Uuid::now_v7();
            sqlx::query("INSERT INTO entity_types(id,kb_id,key,label) VALUES($1,$2,'other','Other')")
                .bind(other).bind(f.kb).execute(&f.pool).await?;
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,specific_type,type_id,type_source) VALUES($1,$2,'Human choice','company',$3,'human')")
                .bind(human).bind(f.kb).bind(f.class).execute(&f.pool).await?;
            let (_, token) = editor(&f).await?;
            let pool = request_pool().await?;
            let original_session = session(&pool).await?;
            let mut state = f.state.clone();
            state.pool = pool.clone();
            let mut events = state.events.subscribe();
            let before = snapshot(&f).await?;
            let mut gate = f.pool.begin().await?;
            let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *gate).await?;
            if binding_lock {
                sqlx::query("SELECT id FROM type_bindings WHERE kb_id=$1 FOR UPDATE")
                    .bind(f.kb).fetch_one(&mut *gate).await?;
            } else {
                sqlx::query("SELECT id FROM entities WHERE id=$1 FOR UPDATE")
                    .bind(f.entity).fetch_one(&mut *gate).await?;
            }
            let class = if unapply { None } else { Some("other") };
            let mut tasks = tokio::task::JoinSet::new();
            let kb = f.kb;
            let state_copy = state.clone();
            let token_copy = token.clone();
            tasks.spawn(async move { request(state_copy, kb, &token_copy, class).await });
            let outcome = async {
                wait_for_lock(&f.pool, blocker).await?;
                let (status, body) = tokio::time::timeout(Duration::from_secs(6), tasks.join_next())
                    .await?.expect("request task")??;
                anyhow::ensure!(status == StatusCode::CONFLICT, "expected 409, got {status}: {body}");
                anyhow::ensure!(body["code"] == "alignment_busy");
                anyhow::ensure!(snapshot(&f).await? == before, "timeout left a partial write");
                anyhow::ensure!(events.try_recv().is_err(), "failed request emitted success");
                anyhow::ensure!(session(&pool).await? == original_session, "session setting leaked");
                anyhow::Ok(())
            }.await;
            // Also run on assertion failure or outer timeout; JoinSet aborts any
            // remaining request when dropped, and the fixture is cleaned below.
            gate.rollback().await?;
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            outcome?;
            let (status, _) = request(state, f.kb, &token, class).await?;
            anyhow::ensure!(status == StatusCode::OK);
            anyhow::ensure!(session(&pool).await? == original_session);
            let after = snapshot(&f).await?;
            anyhow::ensure!(after["binding"]["decided_by"] == "person");
            anyhow::ensure!(after["binding"]["status"] == if unapply {"none"} else {"bound"});
            anyhow::ensure!(after["jobs"] == 1 && after["audit"] == 1);
            let projected: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
                .bind(f.entity).fetch_one(&f.pool).await?;
            anyhow::ensure!(projected == if unapply { None } else { Some(other) });
            let preserved: Uuid = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
                .bind(human).fetch_one(&f.pool).await?;
            anyhow::ensure!(preserved == f.class);
            anyhow::ensure!(events.try_recv()?.kind == "review");
            anyhow::ensure!(events.try_recv()?.kind == "graph");
            anyhow::ensure!(!type_bindings::decide_and_apply(&pool, f.kb, "company", &[], Some(f.class), "bound", &json!({}), "agent").await?);
            anyhow::ensure!(snapshot(&f).await? == after, "older agent overwrote human");
            pool.close().await;
            anyhow::Ok(())
        }.await;
        f.cleanup().await?;
        result
    }

    #[tokio::test]
    async fn binding_lock_returns_conflict_then_retries() -> anyhow::Result<()> {
        contention(true, false).await
    }
    #[tokio::test]
    async fn projection_lock_rolls_back_the_binding() -> anyhow::Result<()> {
        contention(false, false).await
    }
    #[tokio::test]
    async fn unapply_lock_rolls_back_the_binding() -> anyhow::Result<()> {
        contention(false, true).await
    }

    #[tokio::test]
    async fn unrelated_errors_and_permissions_keep_their_meaning() -> anyhow::Result<()> {
        let Some(f) = Fx::new(vec![]).await? else {
            return Ok(());
        };
        let result = async {
            f.seed_bound().await?;
            let (user, token) = editor(&f).await?;
            let before = snapshot(&f).await?;
            anyhow::ensure!(request(f.state.clone(), f.kb, "invalid", None).await?.0 == StatusCode::UNAUTHORIZED);
            anyhow::ensure!(request(f.state.clone(), f.kb, &token, Some("missing")).await?.0 == StatusCode::UNPROCESSABLE_ENTITY);
            anyhow::ensure!(request(f.state.clone(), Uuid::now_v7(), &token, None).await?.0 == StatusCode::NOT_FOUND);
            let other_kb = Uuid::now_v7();
            sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name,visibility) SELECT $1,workspace_id,'Other base','restricted' FROM knowledge_bases WHERE id=$2")
                .bind(other_kb).bind(f.kb).execute(&f.pool).await?;
            anyhow::ensure!(request(f.state.clone(), other_kb, &token, None).await?.0 == StatusCode::NOT_FOUND);
            sqlx::query("UPDATE kb_members SET role='viewer' WHERE user_id=$1").bind(user).execute(&f.pool).await?;
            anyhow::ensure!(request(f.state.clone(), f.kb, &token, None).await?.0 == StatusCode::FORBIDDEN);
            let pool = request_pool().await?;
            let original = session(&pool).await?;
            let error = type_bindings::decide_and_apply_human(&pool, f.kb, "company", Some(Uuid::now_v7()), &json!({})).await.unwrap_err();
            anyhow::ensure!(matches!(error, utopia_core::AppError::Db(sqlx::Error::Database(e)) if e.code().as_deref()==Some("23503")));
            anyhow::ensure!(session(&pool).await? == original);
            anyhow::ensure!(snapshot(&f).await? == before);
            pool.close().await;
            anyhow::Ok(())
        }.await;
        f.cleanup().await?;
        result
    }

    #[tokio::test]
    async fn agent_keeps_its_session_wait_policy() -> anyhow::Result<()> {
        let Some(f) = Fx::new(vec![]).await? else {
            return Ok(());
        };
        let result = async {
            let pool = request_pool().await?;
            let original = session(&pool).await?;
            let mut gate = f.pool.begin().await?;
            let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *gate)
                .await?;
            sqlx::query("SELECT id FROM entities WHERE id=$1 FOR UPDATE")
                .bind(f.entity)
                .fetch_one(&mut *gate)
                .await?;
            let mut tasks = tokio::task::JoinSet::new();
            let (kb, class, request_pool) = (f.kb, f.class, pool.clone());
            tasks.spawn(async move {
                type_bindings::decide_and_apply(
                    &request_pool,
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
            let outcome = async {
                wait_for_lock(&f.pool, blocker).await?;
                anyhow::ensure!(
                    tokio::time::timeout(Duration::from_millis(2300), tasks.join_next())
                        .await
                        .is_err(),
                    "agent received the human timeout"
                );
                anyhow::Ok(())
            }
            .await;
            gate.rollback().await?;
            if outcome.is_err() {
                tasks.abort_all();
            }
            let completed =
                tokio::time::timeout(Duration::from_secs(10), tasks.join_next()).await?;
            outcome?;
            anyhow::ensure!(completed.expect("agent result")??);
            anyhow::ensure!(session(&pool).await? == original);
            pool.close().await;
            anyhow::Ok(())
        }
        .await;
        f.cleanup().await?;
        result
    }
}
