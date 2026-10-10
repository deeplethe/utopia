//! End-to-end conflict governance with a real DB, scripted HTTP model and the
//! real review routes. The script tests delivery/state transitions, not model accuracy.

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::Notify;
use tower::ServiceExt;
use uuid::Uuid;

use crate::state::AppState;

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<VecDeque<Value>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    pause: Arc<AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    m.requests.lock().unwrap().push(body);
    if m.pause.swap(false, Ordering::SeqCst) {
        m.entered.notify_one();
        m.release.notified().await;
    }
    let value = m
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected model call");
    Json(json!({"choices":[{"message":{"role":"assistant","content":value.to_string()}}]}))
}

struct Fx {
    pool: sqlx::PgPool,
    state: AppState,
    app: Router,
    org: Uuid,
    kb: Uuid,
    other_kb: Uuid,
    editor: String,
    viewer: String,
    model: Model,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fx {
    async fn new(replies: Vec<Value>) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        let [org, ws, kb, other_kb, editor, viewer] = std::array::from_fn(|_| Uuid::now_v7());
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','conflict-agent-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','conflict-agent-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                ('{editor}','{org}','{editor}@agent.test','Reviewer','unused'),
                ('{viewer}','{org}','{viewer}@agent.test','Reader','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name,governance) VALUES
                ('{kb}','{ws}','conflicts',true), ('{other_kb}','{ws}','other',false);
             INSERT INTO kb_members(kb_id,user_id,role) VALUES
                ('{kb}','{editor}','editor'), ('{kb}','{viewer}','viewer'),
                ('{other_kb}','{editor}','editor');"
        ))
        .execute(&pool)
        .await?;
        let model = Model {
            replies: Arc::new(Mutex::new(replies.into())),
            requests: Arc::new(Mutex::new(Vec::new())),
            pause: Arc::new(AtomicBool::new(false)),
            entered: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let app = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(model.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
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
        let editor = crate::auth::issue_token(&state, editor)?;
        let viewer = crate::auth::issue_token(&state, viewer)?;
        let app = crate::api::router(state.clone(), &cfg);
        Ok(Some(Self {
            pool,
            state,
            app,
            org,
            kb,
            other_kb,
            editor,
            viewer,
            model,
            server,
            _dir: dir,
        }))
    }

    async fn conflict(&self) -> anyhow::Result<(Uuid, Uuid)> {
        let [subject, a, b, property, old, new, conflict] = std::array::from_fn(|_| Uuid::now_v7());
        let kb = self.kb;
        sqlx::raw_sql(&format!(
            "INSERT INTO entities(id,kb_id,canonical_name) VALUES
                ('{subject}','{kb}','Building'), ('{a}','{kb}','Tenant A'), ('{b}','{kb}','Tenant B');
             INSERT INTO relation_types(id,kb_id,key,label,temporal,functional) VALUES
                ('{property}','{kb}','tenant_{property}','tenant','state',true);"
        )).execute(&self.pool).await?;
        for (fact, object, name, text, date) in [
            (
                old,
                a,
                "old",
                "Tenant A occupies the building from January 1, 2020.",
                "2020-01-01",
            ),
            (
                new,
                b,
                "new",
                "Tenant B became the tenant on June 1, 2021.",
                "2021-06-01",
            ),
        ] {
            let [doc, chunk, statement] = std::array::from_fn(|_| Uuid::now_v7());
            sqlx::raw_sql(&format!(
                "INSERT INTO documents(id,kb_id,filename,sha256,doc_time,doc_time_source)
                    VALUES ('{doc}','{kb}','{name}.txt','{doc}','{date}','content');
                 INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ('{chunk}','{kb}','{doc}',0,'{text}');
                 INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase,attested_from)
                    VALUES ('{statement}','{kb}','{subject}','{object}','open','tenant','{date}');
                 INSERT INTO fact_evidence(fact_id,chunk_id,document_id,quote) VALUES ('{statement}','{chunk}','{doc}','{text}');
                 INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,confidence,valid_from,
                                   valid_from_precision,layer,from_statement_id,attested_from)
                    VALUES ('{fact}','{kb}','{subject}','{property}','{object}',0.9,'2020-01-01',
                            'day','typed','{statement}','{date}');
                 INSERT INTO typed_fact_sources(fact_id,statement_id) VALUES ('{fact}','{statement}');"
            )).execute(&self.pool).await?;
        }
        sqlx::query(
            "INSERT INTO fact_conflicts(id,kb_id,old_fact_id,new_fact_id,reason)
            VALUES ($1,$2,$3,$4,'simultaneous')",
        )
        .bind(conflict)
        .bind(kb)
        .bind(old)
        .bind(new)
        .execute(&self.pool)
        .await?;
        Ok((conflict, new))
    }

    async fn post(
        &self,
        token: &str,
        path: &str,
        body: Value,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))?;
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        self.server.abort();
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM knowledge_bases WHERE id = ANY($1)")
            .bind([self.kb, self.other_kb])
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_batch_records_bad_items_without_blocking_good_ones() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({"verdicts":[
        {"i":0,"action":"keep_both","confidence":0.95,"why":"different scopes"},
        {"i":"1","action":"reject_new","confidence":0.99,"why":"invalid index"},
        {"i":2,"action":"reject_new","confidence":60,"why":"not a probability"}
    ]})])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        let first = f.conflict().await?;
        f.conflict().await?;
        f.conflict().await?;
        crate::governance::govern(&f.state, f.kb).await?;
        let decisions = utopia_store::governance::list(&f.pool, f.kb, 20, 0).await?;
        assert_eq!(decisions.len(), 3);
        assert_eq!(
            decisions.iter().filter(|d| d.status == "applied").count(),
            1
        );
        assert_eq!(
            decisions
                .iter()
                .filter(|d| d.status == "proposed" && d.action == "unsure")
                .count(),
            2
        );
        let requests = f.model.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let sent = requests[0]["messages"].to_string();
        assert!(sent.contains("June 1, 2021"));
        assert!(sent.contains("new.txt"));
        assert!(
            !sent.contains(&first.1.to_string()),
            "database IDs are not model handles"
        );
        crate::governance::govern(&f.state, f.kb).await?;
        assert_eq!(
            f.model.requests.lock().unwrap().len(),
            1,
            "unsure items wait for people"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn the_persons_answer_wins_while_the_model_is_thinking() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({"verdicts":[
        {"i":0,"action":"reject_new","confidence":0.99,"why":"late response"}
    ]})])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        let (conflict, new) = f.conflict().await?;
        f.model.pause.store(true, Ordering::SeqCst);
        let state = f.state.clone();
        let kb = f.kb;
        let work = tokio::spawn(async move { crate::governance::govern(&state, kb).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            f.model.entered.notified(),
        )
        .await?;
        let (status, body) = f
            .post(
                &f.editor,
                &format!("/api/v1/kbs/{}/conflicts/{conflict}", f.kb),
                json!({"action":"keep","rationale":"both statements hold"}),
            )
            .await?;
        f.model.release.notify_one();
        work.await??;
        assert_eq!(status, StatusCode::OK, "{body}");
        let live: bool =
            sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id = $1")
                .bind(new)
                .fetch_one(&f.pool)
                .await?;
        assert!(live);
        assert!(utopia_store::governance::list(&f.pool, f.kb, 20, 0)
            .await?
            .is_empty());
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn dates_can_be_edited_by_an_editor_but_never_by_a_viewer_or_another_base(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({"verdicts":[
        {"i":0,"action":"retime_new","confidence":0.4,"why":"check the date","date":"2021-05"}
    ]})])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        let (_, new) = f.conflict().await?;
        crate::governance::govern(&f.state, f.kb).await?;
        let d = utopia_store::governance::list(&f.pool, f.kb, 10, 0).await?.remove(0);
        assert_eq!(d.status, "proposed");
        let path = format!("/api/v1/kbs/{}/review/agent/{}", f.kb, d.id);
        let edit = json!({"action":"retime_new","date":"2021-06-01T00:00:00Z","date_precision":"day",
            "rationale":"the amendment states June 1"});
        assert_eq!(f.post(&f.viewer, &path, edit.clone()).await?.0, StatusCode::FORBIDDEN);
        assert_eq!(f.post(&f.editor, &format!("/api/v1/kbs/{}/review/agent/{}", f.other_kb, d.id), edit.clone()).await?.0,
            StatusCode::NOT_FOUND);
        let (status, body) = f.post(&f.editor, &path, edit).await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        let row = utopia_store::governance::get(&f.pool, f.kb, d.id).await?;
        assert_eq!(row.status, "overridden", "a changed date is an override even with the same action");
        let start: String = sqlx::query_scalar("SELECT valid_from::date::text FROM facts WHERE supersedes = $1 AND invalidated_at IS NULL")
            .bind(new).fetch_one(&f.pool).await?;
        assert_eq!(start, "2021-06-01");
        let p = utopia_store::conflict_governance::precedents(&f.pool, f.kb).await?;
        assert_eq!(p[0]["detail"]["why"], "the amendment states June 1");
        anyhow::Ok(())
    }.await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn two_real_reverts_trip_the_shared_fuse() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({"verdicts":[
        {"i":0,"action":"keep_both","confidence":0.95,"why":"scoped values"},
        {"i":1,"action":"keep_both","confidence":0.95,"why":"scoped values"}
    ]})])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        f.conflict().await?;
        f.conflict().await?;
        crate::governance::govern(&f.state, f.kb).await?;
        let decisions = utopia_store::governance::list(&f.pool, f.kb, 10, 0).await?;
        assert_eq!(decisions.len(), 2);
        for d in decisions {
            let (status, body) = f
                .post(
                    &f.editor,
                    &format!("/api/v1/kbs/{}/review/agent/{}", f.kb, d.id),
                    json!({"action":"revert","rationale":"the scope was not supported"}),
                )
                .await?;
            assert_eq!(status, StatusCode::OK, "{body}");
        }
        assert!(!utopia_store::kbs::get(&f.pool, f.kb).await?.governance);
        let alerts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM alerts WHERE kb_id = $1 AND kind = 'governance.tripped'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(alerts, 1);
        crate::governance::govern(&f.state, f.kb).await?;
        assert_eq!(f.model.requests.lock().unwrap().len(), 1);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn the_conflict_budget_and_governance_switch_prevent_model_calls() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![]).await? else {
        return Ok(());
    };
    let run = async {
        f.conflict().await?;
        sqlx::query("UPDATE knowledge_bases SET governance = false WHERE id = $1")
            .bind(f.kb).execute(&f.pool).await?;
        crate::governance::govern(&f.state, f.kb).await?;
        assert!(f.model.requests.lock().unwrap().is_empty());
        sqlx::query("UPDATE knowledge_bases SET governance = true WHERE id = $1")
            .bind(f.kb).execute(&f.pool).await?;
        sqlx::query("INSERT INTO agent_decisions(id,kb_id,run_id,target_kind,target_id,action,status)
            SELECT gen_random_uuid(),$1,gen_random_uuid(),'conflict',gen_random_uuid(),'unsure','superseded'
            FROM generate_series(1,2000)")
            .bind(f.kb).execute(&f.pool).await?;
        crate::governance::govern(&f.state, f.kb).await?;
        assert!(f.model.requests.lock().unwrap().is_empty());
        assert_eq!(utopia_store::conflict_governance::queue(&f.pool, f.kb, 10).await?.len(), 1);
        anyhow::Ok(())
    }.await;
    f.cleanup().await?;
    run
}
