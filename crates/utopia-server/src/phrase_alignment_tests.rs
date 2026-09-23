//! 一条短语判定的生命周期（0053，#807，#795）：候选经继承命中、父边增删让判定过期、
//! 无候选与超限各自落库、端点类换了旧行不再循环、请求途中的编辑留下可见的过期、
//! 人的判定不被覆盖。模型是脚本化的 HTTP 端点，库是真的 PostgreSQL。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use utopia_store::{materialize, phrase_bindings};

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<Value>>>,
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
    let text = {
        let mut replies = m.replies.lock().unwrap();
        if replies.is_empty() {
            panic!("unexpected model request #{n}");
        }
        replies.remove(0).to_string()
    };
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
    legal_entity: Uuid,
    organization: Uuid,
    acme: Uuid,
    based_in: Uuid,
    model: Model,
    server: tokio::task::JoinHandle<()>,
    dir: tempfile::TempDir,
}

impl Fx {
    /// 一个库：legal_entity ⊃ organization，place；属性 based_in 声明在 legal_entity → place；
    /// Acme（organization）—based in→ London（place）一条开放陈述
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, legal_entity, organization, place, acme, london, based_in, statement) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','phrase-lifecycle');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','phrase-lifecycle');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','phrase-lifecycle');
             INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
                 ('{legal_entity}','{kb}','legal_entity','Legal entity','#000','circle'),
                 ('{organization}','{kb}','organization','Organization','#000','circle'),
                 ('{place}','{kb}','place','Place','#000','circle');
             INSERT INTO entity_type_parents(child_id,parent_id,is_primary) VALUES
                 ('{organization}','{legal_entity}',true);
             INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
                 ('{based_in}','{kb}','based_in','based in','relation','state','where an entity is based');
             INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES ('{based_in}','{legal_entity}');
             INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES ('{based_in}','{place}');
             INSERT INTO entities(id,kb_id,canonical_name,type_id) VALUES
                 ('{acme}','{kb}','Acme','{organization}'), ('{london}','{kb}','London','{place}');
             INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES
                 ('{statement}','{kb}','{acme}','{london}','open','based in');"
        ))
        .execute(&pool)
        .await?;
        let model = Model {
            replies: Arc::new(Mutex::new(Vec::new())),
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
            legal_entity,
            organization,
            acme,
            based_in,
            model,
            server,
            dir,
        }))
    }
    fn script(&self, replies: Vec<Value>) {
        *self.model.replies.lock().unwrap() = replies;
    }
    async fn run(&self) -> anyhow::Result<()> {
        align_phrases(&self.state, self.kb).await
    }
    fn requests(&self) -> Vec<Value> {
        self.model.requests.lock().unwrap().clone()
    }
    fn prompt_of(&self, n: usize) -> String {
        self.requests()[n]["messages"][1]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
    async fn binding(&self) -> anyhow::Result<phrase_bindings::Binding> {
        let mut all = phrase_bindings::bindings(&self.pool, self.kb).await?;
        anyhow::ensure!(!all.is_empty(), "no binding");
        Ok(all.remove(0))
    }
    /// 这条签名落库时记的原因（`votes.reason`）：结构性结果不问模型，原因写在票里
    async fn reason(&self) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT votes->>'reason' FROM phrase_bindings WHERE kb_id=$1 ORDER BY decided_at DESC LIMIT 1")
            .bind(self.kb)
            .fetch_one(&self.pool)
            .await?)
    }
    async fn typed(&self) -> anyhow::Result<i64> {
        Ok(materialize::count(&self.pool, self.kb).await?)
    }
    /// 这一轮结束后有没有再排一次对齐：收敛的判据
    async fn requeued(&self) -> anyhow::Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind='align_phrases' AND status='queued' AND payload->>'kb_id'=$1",
        )
        .bind(self.kb.to_string())
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }
    async fn clear_jobs(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn cleanup(self) -> anyhow::Result<()> {
        self.server.abort();
        self.clear_jobs().await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        drop(self.state);
        self.dir.close()?;
        Ok(())
    }
}

fn vote(key: Option<&str>, dir: Option<&str>) -> Value {
    json!({"b":[[0, key, dir]]})
}
/// 两票之后对齐还会问一次「这种形状还蕴含什么」（0044 决定 3 第五片）：脚本里答「没有」
fn nothing_implied() -> Value {
    json!({"i":[[0,null,null]]})
}
fn bound() -> Vec<Value> {
    vec![
        vote(Some("based_in"), Some("forward")),
        vote(Some("based_in"), Some("forward")),
        nothing_implied(),
    ]
}
fn none() -> Vec<Value> {
    vec![vote(None, None), vote(None, None), nothing_implied()]
}

#[tokio::test]
async fn a_property_declared_on_an_ancestor_is_offered_with_its_basis_and_bound(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        f.script(bound());
        f.run().await?;
        assert_eq!(
            f.requests().len(),
            2,
            "two votes; bound to the only property, so no rule question"
        );
        let prompt = f.prompt_of(0);
        assert!(prompt.contains("based_in"), "{prompt}");
        assert!(
            prompt.contains("fits by inheritance: organization is a subclass of legal_entity"),
            "the model is told why the candidate fits: {prompt}"
        );
        let b = f.binding().await?;
        assert_eq!(
            (b.status.as_str(), b.decided_by.as_str()),
            ("bound", "agent")
        );
        assert!(b.basis.is_some(), "an agent decision records its basis");
        assert_eq!(f.typed().await?, 1, "the projection follows");
        assert!(
            !f.requeued().await?,
            "unchanged inputs leave no queued work"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn removing_the_parent_edge_retires_the_projection_without_a_model_call() -> anyhow::Result<()>
{
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        f.script(bound());
        f.run().await?;
        assert_eq!(f.typed().await?, 1);
        sqlx::query("DELETE FROM entity_type_parents WHERE child_id=$1")
            .bind(f.organization)
            .execute(&f.pool)
            .await?;
        f.clear_jobs().await?;
        f.run().await?;
        assert_eq!(f.requests().len(), 2, "no candidate, nothing to ask");
        let b = f.binding().await?;
        assert_eq!(b.status, "none");
        assert_eq!(f.reason().await?.as_deref(), Some("no_candidates"));
        assert_eq!(f.typed().await?, 0, "the unsupported projection is retired");
        assert!(!f.requeued().await?);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn adding_a_parent_edge_reopens_a_structural_none() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        sqlx::query("DELETE FROM entity_type_parents WHERE child_id=$1")
            .bind(f.organization)
            .execute(&f.pool)
            .await?;
        f.run().await?;
        assert_eq!(f.requests().len(), 0);
        assert_eq!(f.binding().await?.status, "none");
        assert!(!f.requeued().await?);
        sqlx::query(
            "INSERT INTO entity_type_parents(child_id,parent_id,is_primary) VALUES($1,$2,true)",
        )
        .bind(f.organization)
        .bind(f.legal_entity)
        .execute(&f.pool)
        .await?;
        f.script(bound());
        f.run().await?;
        assert_eq!(
            f.requests().len(),
            2,
            "the edge changed the basis, so it is asked again"
        );
        assert_eq!(f.binding().await?.status, "bound");
        assert_eq!(f.typed().await?, 1);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn overflow_is_recorded_for_a_person_and_recovers_when_candidates_shrink(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        // 61 条不声明域/值域的关系：哪一端都接受，加上 based_in 共 62 > 60
        for i in 0..61 {
            sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,kind,temporal) VALUES($1,$2,$3,$3,'relation','state')")
                .bind(Uuid::now_v7()).bind(f.kb).bind(format!("filler_{i}")).execute(&f.pool).await?;
        }
        f.run().await?;
        assert_eq!(f.requests().len(), 0, "too many to ask");
        let b = f.binding().await?;
        assert_eq!(b.status, "undecided");
        assert_eq!(f.reason().await?.as_deref(), Some("too_many_candidates"));
        assert!(!f.requeued().await?, "overflow must not queue a run it cannot execute");
        sqlx::query("DELETE FROM relation_types WHERE kb_id=$1 AND key LIKE 'filler_%'")
            .bind(f.kb)
            .execute(&f.pool)
            .await?;
        f.script(bound());
        f.run().await?;
        assert_eq!(f.requests().len(), 2);
        assert_eq!(f.binding().await?.status, "bound");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn an_endpoint_class_change_moves_the_signature_and_the_old_row_stops_looping(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        // Acme 还没有类：签名 (based in, ?, place)，声明了域的属性不接受空的一端
        sqlx::query("UPDATE entities SET type_id=NULL WHERE id=$1")
            .bind(f.acme)
            .execute(&f.pool)
            .await?;
        f.run().await?;
        assert_eq!(f.requests().len(), 0);
        let old = f.binding().await?;
        assert_eq!((old.status.as_str(), old.subject_type_id), ("none", None));
        // 类别词绑上了：签名换成 (based in, organization, place)，旧行成了孤儿
        sqlx::query("UPDATE entities SET type_id=$2 WHERE id=$1")
            .bind(f.acme)
            .bind(f.organization)
            .execute(&f.pool)
            .await?;
        f.script(bound());
        f.run().await?;
        assert_eq!(f.requests().len(), 2);
        let all = phrase_bindings::bindings(&f.pool, f.kb).await?;
        assert_eq!(all.len(), 2, "the orphan stays as a cached decision");
        assert!(all
            .iter()
            .any(|b| b.subject_type_id == Some(f.organization) && b.status == "bound"));
        assert!(
            !f.requeued().await?,
            "an orphan must not queue work it cannot execute"
        );
        f.run().await?;
        assert_eq!(
            f.requests().len(),
            2,
            "a second run asks nothing and queues nothing"
        );
        assert!(!f.requeued().await?);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn an_edit_during_the_model_request_leaves_the_decision_stale() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        f.model.hold.store(true, std::sync::atomic::Ordering::SeqCst);
        f.script(none());
        let state = f.state.clone();
        let kb = f.kb;
        let worker = tokio::spawn(async move { align_phrases(&state, kb).await });
        tokio::time::timeout(std::time::Duration::from_secs(10), f.model.entered.notified()).await?;
        // 模型还在答，定义改了：两票读的都是旧定义
        sqlx::query("UPDATE relation_types SET description='NEW definition', updated_at=clock_timestamp() WHERE id=$1")
            .bind(f.based_in)
            .execute(&f.pool)
            .await?;
        f.model.release.notify_one();
        worker.await??;
        let b = f.binding().await?;
        assert_eq!(b.status, "none");
        assert!(
            f.requeued().await?,
            "the run noticed its own basis is already stale and queued another"
        );
        f.clear_jobs().await?;
        f.script(bound());
        f.run().await?;
        assert_eq!(f.requests().len(), 5, "two votes, one rule question (nothing implied), then two votes again: the basis differs, not the clock");
        assert_eq!(f.binding().await?.status, "bound");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn a_person_decision_made_during_the_request_is_not_overwritten() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        f.model
            .hold
            .store(true, std::sync::atomic::Ordering::SeqCst);
        f.script(none());
        let state = f.state.clone();
        let kb = f.kb;
        let worker = tokio::spawn(async move { align_phrases(&state, kb).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            f.model.entered.notified(),
        )
        .await?;
        let sig = phrase_bindings::signatures(&f.pool, f.kb).await?.remove(0);
        phrase_bindings::decide(
            &f.pool,
            f.kb,
            &sig,
            phrase_bindings::Decision {
                relation_type_id: Some(f.based_in),
                direction: Some("forward"),
                status: "bound",
                votes: &json!({}),
                decided_by: "person",
                basis: None,
            },
        )
        .await?;
        f.model.release.notify_one();
        worker.await??;
        let b = f.binding().await?;
        assert_eq!(
            (b.status.as_str(), b.decided_by.as_str()),
            ("bound", "person")
        );
        assert!(
            !f.requeued().await?,
            "a person's decision is never re-evaluated"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
