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
/// 嵌入端点一直坏着：只有配了嵌入模型的测试会走到这里
async fn embeddings_down() -> impl IntoResponse {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "embedding backend down",
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
            .route("/embeddings", post(embeddings_down))
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
        align_types_reasking(&self.state, self.kb, 0).await
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

/// 本体向量还在补时类别词对齐不动手：编辑器建了类先排 `embed_ontology` 再排这个任务，
/// 候选类按向量检索，没向量的类检索不到。它给自己排一份半分钟后的，补齐收尾后照常判
#[tokio::test]
async fn kind_word_alignment_waits_while_the_ontology_index_is_refreshing() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![vote(Some("organization")), vote(Some("organization"))]).await?
    else {
        return Ok(());
    };
    let run = async {
        sqlx::query("INSERT INTO jobs (kind, payload) VALUES ('embed_ontology', $1)")
            .bind(json!({ "kb_id": f.kb }))
            .execute(&f.pool)
            .await?;
        f.run().await?;
        assert_eq!(
            f.requests().len(),
            0,
            "nothing is asked while the ontology index is still being refreshed"
        );
        let deferred: Vec<(String, bool)> = sqlx::query_as(
            "SELECT status, run_at > now() FROM jobs
             WHERE kind='align_types' AND payload->>'kb_id'=$1",
        )
        .bind(f.kb.to_string())
        .fetch_all(&f.pool)
        .await?;
        assert_eq!(
            deferred,
            vec![("queued".to_string(), true)],
            "one kind-word round is queued for later"
        );
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(f.kb.to_string())
            .execute(&f.pool)
            .await?;
        f.run().await?;
        assert_eq!(
            f.requests().len(),
            2,
            "the refresh gone, the two votes are asked"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
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
    let worker = tokio::spawn(async move { align_types_reasking(&state, kb, 0).await });
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
    let requeued = align_types_jobs(&f).await?;
    let class = f.class;
    f.cleanup().await?;
    assert_eq!(binding.decided_by, "person");
    assert_eq!(binding.type_id, Some(class));
    assert_eq!(projected, Some(class));
    assert!(
        requeued.is_empty(),
        "a person's decision carries no basis and is never asked about again"
    );
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

/// bench 里「12 篇文档 type_id 全空」的回复：DeepSeek-V3.2 把整段答成按 id 作键的对象，
/// 第二票还把键包进单元素数组。两票都得读出来，词才绑得上
#[tokio::test]
async fn an_id_keyed_reply_still_binds_the_kind_word() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        json!({"0": "organization"}),
        json!({"b": {"0": ["organization"]}}),
    ])
    .await?
    else {
        return Ok(());
    };
    f.run().await?;
    let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
    let projected: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(f.entity)
        .fetch_one(&f.pool)
        .await?;
    let count = f.requests().len();
    let class = f.class;
    f.cleanup().await?;
    assert_eq!(count, 2);
    assert_eq!(binding.status, "bound");
    assert_eq!(binding.type_id, Some(class));
    assert_eq!(projected, Some(class));
    Ok(())
}

/// 读不出的回复不再是「一项都没答」然后没了：这一轮不下结论，任务自己再排一份
/// （带 reask），排够 MAX_REASK 次就停，等下一篇文档或本体改动。
#[tokio::test]
async fn an_unreadable_reply_is_asked_again_a_bounded_number_of_times() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        json!({"answer": "organization"}),
        json!({"answer": "organization"}),
        json!({"answer": "organization"}),
        json!({"answer": "organization"}),
    ])
    .await?
    else {
        return Ok(());
    };
    f.run().await?;
    let bindings = type_bindings::bindings(&f.pool, f.kb).await?.len();
    let reasks: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT status, (payload->>'reask')::bigint FROM jobs
         WHERE kind='align_types' AND payload->>'kb_id'=$1 ORDER BY id",
    )
    .bind(f.kb.to_string())
    .fetch_all(&f.pool)
    .await?;
    // 还要再问一轮：短语对齐先不排，两端的类还没定
    let phrases_queued_early: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='align_phrases' AND payload->>'kb_id'=$1",
    )
    .bind(f.kb.to_string())
    .fetch_one(&f.pool)
    .await?;
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .execute(&f.pool)
        .await?;
    // 已经是最后一次自己排的：不再排
    align_types_reasking(&f.state, f.kb, MAX_REASK).await?;
    let after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='align_types' AND payload->>'kb_id'=$1",
    )
    .bind(f.kb.to_string())
    .fetch_one(&f.pool)
    .await?;
    // 这是最后一轮：短语对齐现在排
    let phrases_queued_late: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind='align_phrases' AND payload->>'kb_id'=$1",
    )
    .bind(f.kb.to_string())
    .fetch_one(&f.pool)
    .await?;
    let count = f.requests().len();
    f.cleanup().await?;
    assert_eq!(count, 4, "两轮各问两票");
    assert_eq!(bindings, 0, "读不出的回复不写任何判定");
    assert_eq!(
        reasks,
        vec![("queued".to_string(), Some(1))],
        "第一轮之后自己排一份 reask=1"
    );
    assert_eq!(after, 0, "排够次数就不再排");
    assert_eq!(phrases_queued_early, 0, "还要再问时短语对齐不排");
    assert_eq!(phrases_queued_late, 1, "最后一轮排短语对齐");
    Ok(())
}

/// #795：模型还在答的时候类的定义改了。两票读的都是旧定义，它们的答案不能当成对新定义
/// 的判定留下来；下一轮要拿新定义再问，问完之后输入没变就不再问
#[tokio::test]
async fn an_edit_during_the_model_request_is_asked_again() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        vote(None),
        vote(None),
        vote(Some("organization")),
        vote(Some("organization")),
    ])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        f.model
            .hold
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let state = f.state.clone();
        let kb = f.kb;
        let worker = tokio::spawn(async move { align_types_reasking(&state, kb, 0).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            f.model.entered.notified(),
        )
        .await?;
        sqlx::query(
            "UPDATE entity_types SET description='NEW definition', updated_at=clock_timestamp()
              WHERE id=$1",
        )
        .bind(f.class)
        .execute(&f.pool)
        .await?;
        f.model.release.notify_one();
        worker.await??;
        let first = f.requests();
        anyhow::ensure!(first.len() == 2, "two votes, got {}", first.len());
        anyhow::ensure!(
            first
                .iter()
                .all(|r| r.to_string().contains("OLD definition")),
            "both votes were built before the edit"
        );
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(f.kb.to_string())
            .execute(&f.pool)
            .await?;
        f.run().await?;
        let second = f.requests();
        anyhow::ensure!(
            second.len() == 4,
            "the edit made during the request was taken as seen: the next run asked {} more",
            second.len() - 2
        );
        anyhow::ensure!(
            second[2..]
                .iter()
                .all(|r| r.to_string().contains("NEW definition")),
            "the next run asks with the new definition"
        );
        let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
        anyhow::ensure!(binding.status == "bound" && binding.type_id == Some(f.class));
        f.run().await?;
        anyhow::ensure!(f.requests().len() == 4, "unchanged inputs ask nothing");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

async fn align_types_jobs(f: &Fx) -> anyhow::Result<Vec<Option<i64>>> {
    Ok(sqlx::query_scalar(
        "SELECT (payload->>'reask')::bigint FROM jobs
          WHERE kind='align_types' AND payload->>'kb_id'=$1 ORDER BY id",
    )
    .bind(f.kb.to_string())
    .fetch_all(&f.pool)
    .await?)
}

async fn clear_jobs(f: &Fx) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .execute(&f.pool)
        .await?;
    Ok(())
}

/// 父边的增删不碰 `updated_at`，时间戳看不见它；指纹里有祖先闭包，看得见
#[tokio::test]
async fn a_parent_edge_makes_an_agent_binding_stale() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        vote(Some("organization")),
        vote(Some("organization")),
        vote(Some("organization")),
        vote(Some("organization")),
    ])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        let legal = Uuid::now_v7();
        sqlx::query("INSERT INTO entity_types(id,kb_id,key,label,description) VALUES($1,$2,'legal_entity','Legal entity','A body the law treats as a person')")
            .bind(legal).bind(f.kb).execute(&f.pool).await?;
        f.run().await?;
        anyhow::ensure!(f.requests().len() == 2);
        clear_jobs(&f).await?;
        let versions = "SELECT id, updated_at FROM entity_types WHERE kb_id=$1 ORDER BY id";
        let before: Vec<(Uuid, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(versions)
            .bind(f.kb)
            .fetch_all(&f.pool)
            .await?;
        sqlx::query(
            "INSERT INTO entity_type_parents(child_id,parent_id,is_primary) VALUES($1,$2,true)",
        )
        .bind(f.class)
        .bind(legal)
        .execute(&f.pool)
        .await?;
        let after: Vec<(Uuid, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(versions)
            .bind(f.kb)
            .fetch_all(&f.pool)
            .await?;
        anyhow::ensure!(before == after, "a parent edge leaves updated_at alone");
        f.run().await?;
        anyhow::ensure!(
            f.requests().len() == 4,
            "the ancestor closure is part of the basis, so the word is asked again"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

/// 这一列出现之前代理判的行没有指纹：各重判一次，之后不再问
#[tokio::test]
async fn rows_without_a_basis_are_decided_again_once() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![vote(Some("organization")), vote(Some("organization"))]).await?
    else {
        return Ok(());
    };
    let run = async {
        f.seed_bound().await?;
        f.run().await?;
        anyhow::ensure!(f.requests().len() == 2);
        let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
        anyhow::ensure!(binding.basis.is_some() && binding.decided_by == "agent");
        f.run().await?;
        anyhow::ensure!(
            f.requests().len() == 2,
            "a recorded basis that still matches asks nothing"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

/// 过期的词这一轮问了却没问出结论（端点一直报错、回复读不出）：走有上限的延时再问，
/// 不能因为「还过期」立刻再排——那样一个永久报错的端点会让任务一轮接一轮地跑
#[tokio::test]
async fn a_stale_word_whose_batch_fails_takes_the_bounded_reask() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        json!({"answer": "organization"}),
        json!({"answer": "organization"}),
    ])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        f.seed_bound().await?;
        f.run().await?;
        anyhow::ensure!(f.requests().len() == 2);
        let jobs = align_types_jobs(&f).await?;
        anyhow::ensure!(
            jobs == vec![Some(1)],
            "only the delayed re-ask is queued, got {jobs:?}"
        );
        let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
        anyhow::ensure!(binding.basis.is_none(), "nothing was decided this round");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

/// 嵌入端点一直坏着：检索退回整表，小库照样绑得上（#894 修过的「整库没有类型」不能回来）；
/// 一直坏下去候选不变、指纹不变，也不会每轮再问一遍
#[tokio::test]
async fn a_retrieval_error_falls_back_to_the_whole_class_list() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![vote(Some("organization")), vote(Some("organization"))]).await?
    else {
        return Ok(());
    };
    let run = async {
        let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM knowledge_bases WHERE id=$1")
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
        let chat = utopia_store::settings::get(&f.pool, ws)
            .await?
            .and_then(|s| s.chat_base_url)
            .expect("fixture endpoint");
        // upsert 整行覆盖（只有密钥传 None 才保留旧值），对话端点得原样再给一次
        utopia_store::settings::upsert(
            &f.pool,
            ws,
            Some(&chat),
            None,
            Some("scripted"),
            Some(&chat),
            None,
            Some("scripted-embedding"),
            Some(4),
        )
        .await?;
        f.run().await?;
        anyhow::ensure!(f.requests().len() == 2, "the fallback still asks");
        let binding = type_bindings::bindings(&f.pool, f.kb).await?.remove(0);
        anyhow::ensure!(binding.status == "bound" && binding.type_id == Some(f.class));
        let jobs = align_types_jobs(&f).await?;
        anyhow::ensure!(
            jobs.is_empty(),
            "a settled word queues nothing, got {jobs:?}"
        );
        f.run().await?;
        anyhow::ensure!(
            f.requests().len() == 2,
            "while retrieval keeps failing the candidates, and so the basis, stay the same"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
