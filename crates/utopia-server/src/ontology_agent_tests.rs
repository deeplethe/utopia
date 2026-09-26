//! 本体代理的一轮（0061）：没绑上的形状和接受了的问题交给脚本化的模型，提案落进
//! `ontology_proposals` 并记下服务的问题与形状；采纳建出属性并排对齐；提过的形状下一轮
//! 不再送。没有 `UTOPIA_DATABASE_URL` 时跳过。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use utopia_store::phrase_bindings::Decision;

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<Value>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    m.requests.lock().unwrap().push(body);
    let text = {
        let mut replies = m.replies.lock().unwrap();
        if replies.is_empty() {
            panic!("unexpected model request");
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
    organization: Uuid,
    place: Uuid,
    located_in: Uuid,
    /// 采纳的人（decided_by 是外键）
    user: Uuid,
    model: Model,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fx {
    /// 一个库：organization、place 两个类，一条属性 located_in（organization → place）；
    /// Acme —founded in→ London、Acme —located in→ London 两条开放陈述（形状按短语排：
    /// s0 = founded in，s1 = located in）
    async fn new(replies: Vec<Value>) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, organization, place, acme, london, statement, user, located_in, st2) = (
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
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','ontology-agent');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','ontology-agent');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','ontology-agent');
             INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES ('{user}','{org}','{user}@agent.test','unused','Adopter');
             INSERT INTO entity_types(id,kb_id,key,label,color,shape,description) VALUES
                 ('{organization}','{kb}','organization','Organization','#000','circle','a company or institution'),
                 ('{place}','{kb}','place','Place','#000','circle','a geographic location');
             INSERT INTO entities(id,kb_id,canonical_name,type_id) VALUES
                 ('{acme}','{kb}','Acme','{organization}'), ('{london}','{kb}','London','{place}');
             INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
                 ('{located_in}','{kb}','located_in','located in','relation','state','where an organization sits');
             INSERT INTO relation_type_domains(relation_type_id,entity_type_id) VALUES ('{located_in}','{organization}');
             INSERT INTO relation_type_ranges(relation_type_id,entity_type_id) VALUES ('{located_in}','{place}');
             INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES
                 ('{statement}','{kb}','{acme}','{london}','open','founded in'),
                 ('{st2}','{kb}','{acme}','{london}','open','located in');"
        ))
        .execute(&pool)
        .await?;
        let model = Model {
            replies: Arc::new(Mutex::new(replies)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(model.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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
            organization,
            place,
            located_in,
            user,
            model,
            server,
            _dir: dir,
        }))
    }

    /// 对齐判过这两条形状：没有属性可绑
    async fn decide_none(&self) -> anyhow::Result<()> {
        let sigs = phrase_bindings::signatures(&self.pool, self.kb).await?;
        assert_eq!(sigs.len(), 2, "two open statements, two signatures");
        for sig in &sigs {
            phrase_bindings::decide(
                &self.pool,
                self.kb,
                sig,
                Decision {
                    relation_type_id: None,
                    direction: None,
                    status: "none",
                    votes: &json!({ "reason": "no_candidates" }),
                    decided_by: "agent",
                    basis: Some("test"),
                },
            )
            .await?;
        }
        Ok(())
    }

    async fn requests(&self) -> usize {
        self.model.requests.lock().unwrap().len()
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        self.server.abort();
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        // decided_by 指向 users，不级联：先清提案再删组织
        sqlx::query("DELETE FROM ontology_agent_reviews WHERE kb_id=$1")
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM ontology_proposals WHERE kb_id=$1")
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn user_text(request: &Value) -> String {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn an_unbound_shape_becomes_a_proposal_that_serves_a_question_and_adoption_builds_the_property(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({
        "p": [{
            "kind": "property", "key": "founded_in", "label": "founded in",
            "definition": "the place where an organization was founded",
            "value": false, "datatype": null,
            "domains": ["organization"], "ranges": ["place"],
            "s": [0], "k": [], "q": [0]
        }],
        "existing": []
    })])
    .await?
    else {
        return Ok(());
    };
    f.decide_none().await?;
    let qid = competency_questions::create(
        &f.pool,
        f.kb,
        competency_questions::NewQuestion {
            question: "Where was each organization founded?",
            expected_answer: None,
            needs: None,
            origin: "person",
            status: "accepted",
            created_by: None,
        },
    )
    .await?;

    propose(&f.state, f.kb).await?;

    // 模型看到了形状、词表和问题
    let requests = f.model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "one batch, one call");
    let text = user_text(&requests[0]);
    assert!(
        text.contains("founded in"),
        "the open shape is offered: {text}"
    );
    assert!(
        text.contains("organization"),
        "the glossary is offered: {text}"
    );
    assert!(
        text.contains("located_in"),
        "existing properties are in the glossary: {text}"
    );
    assert!(
        text.contains("Where was each organization founded?"),
        "the accepted question is offered: {text}"
    );

    // 提案落库，带着谁提的、服务的问题、会绑的形状
    let open = utopia_store::ontology::open_proposals(&f.pool, f.kb).await?;
    assert_eq!(open.len(), 1, "one proposal: {open:?}");
    let p = &open[0];
    assert_eq!(p.section, "relation_types");
    assert_eq!(p.key, "founded_in");
    assert_eq!(p.proposed_by, "agent");
    assert_eq!(p.serves, vec![qid]);
    assert_eq!(
        p.signatures["phrases"][0]["phrase"], "founded in",
        "the shape it binds is recorded: {}",
        p.signatures
    );
    assert_eq!(p.signatures["phrases"][0]["subject"], "organization");
    assert_eq!(p.payload["forms"][0], "founded in");
    assert_eq!(p.payload["domains"][0], "organization");
    assert_eq!(p.payload["ranges"][0], "place");
    assert_eq!(
        p.payload["description"],
        "the place where an organization was founded"
    );

    // 提过的形状、看过没提的形状（located in）下一轮都不再送：没有模型调用
    let outcomes: Vec<(String, String)> = sqlx::query_as(
        "SELECT shape->>'phrase', outcome FROM ontology_agent_reviews WHERE kb_id=$1 ORDER BY 1",
    )
    .bind(f.kb)
    .fetch_all(&f.pool)
    .await?;
    assert_eq!(
        outcomes,
        vec![
            ("founded in".to_string(), "proposed".to_string()),
            ("located in".to_string(), "declined".to_string())
        ],
        "every offered shape is recorded with its outcome"
    );
    propose(&f.state, f.kb).await?;
    assert_eq!(
        f.requests().await,
        1,
        "already-proposed and declined shapes are not offered again"
    );

    // 采纳：属性建出来，签名域值域照提案，提案标记 adopted，短语对齐排上
    let id = adopt(
        &f.state,
        f.kb,
        "relation_types",
        "founded_in",
        AdoptEdits {
            label: Some("Founded in".into()),
            ..Default::default()
        },
        f.user,
    )
    .await?;
    let props = utopia_store::ontology::relation_type_views(&f.pool, f.kb).await?;
    let built = props
        .iter()
        .find(|p| p.id == id)
        .expect("the property exists");
    assert_eq!(built.key, "founded_in");
    // 属性标签照库里的规矩写成小驼峰：人的改动进去了，只是换了写法
    assert_eq!(built.label, "foundedIn", "the person's edit wins");
    assert_eq!(built.kind, "relation");
    assert_eq!(built.domains, vec![f.organization]);
    assert_eq!(built.ranges, vec![f.place]);
    assert_eq!(
        built.description,
        "the place where an organization was founded"
    );
    assert!(
        utopia_store::ontology::open_proposals(&f.pool, f.kb)
            .await?
            .is_empty(),
        "the proposal is no longer open"
    );
    let status: String = sqlx::query_scalar(
        "SELECT status FROM ontology_proposals WHERE kb_id=$1 AND section='relation_types' AND key='founded_in'",
    )
    .bind(f.kb)
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(status, "adopted");
    assert!(
        utopia_store::jobs::pending_for_kb(&f.pool, "align_phrases", f.kb).await?,
        "adoption re-decides the shapes through alignment"
    );

    // 采纳过的不能再采纳
    let again = adopt(
        &f.state,
        f.kb,
        "relation_types",
        "founded_in",
        AdoptEdits::default(),
        f.user,
    )
    .await;
    assert!(
        matches!(again, Err(AppError::NotFound)),
        "a decided proposal is not open: {again:?}"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_proposal_for_an_existing_key_or_binding_nothing_is_dropped() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({
        "p": [
            // 已有的类：不提
            {"kind": "class", "key": "place", "label": "Place", "definition": "x", "parents": [], "s": [], "k": [], "q": []},
            // 什么都不绑：坏项
            {"kind": "property", "key": "orphan", "label": "orphan", "definition": "x", "value": false,
             "domains": [], "ranges": [], "s": [], "k": [], "q": []},
            // 字面值属性：进 attribute_types，datatype 缺省 text
            {"kind": "property", "key": "founding_note", "label": "founding note", "definition": "x", "value": true,
             "datatype": "weird", "domains": ["organization"], "ranges": [], "s": [0], "k": [], "q": []}
        ],
        "existing": []
    })])
    .await?
    else {
        return Ok(());
    };
    f.decide_none().await?;
    propose(&f.state, f.kb).await?;
    let open = utopia_store::ontology::open_proposals(&f.pool, f.kb).await?;
    assert_eq!(open.len(), 1, "only the attribute survives: {open:?}");
    assert_eq!(open[0].section, "attribute_types");
    assert_eq!(open[0].key, "founding_note");
    assert_eq!(open[0].payload["datatype"], "text");
    assert!(open[0].serves.is_empty());
    f.cleanup().await
}

#[tokio::test]
async fn a_declined_shape_returns_when_the_glossary_changes_and_an_existing_answer_becomes_a_binding(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        // 第一轮：founded in 没提；located in 本体里已经有（located_in，正向）
        json!({ "p": [], "existing": [{ "key": "located_in", "dir": "forward", "s": [1], "k": [] }] }),
        // 第三轮（词表变了之后）：只剩 founded in 送来，还是没提
        json!({ "p": [], "existing": [] }),
    ])
    .await?
    else {
        return Ok(());
    };
    f.decide_none().await?;

    propose(&f.state, f.kb).await?;
    assert_eq!(f.requests().await, 1);
    let open = utopia_store::ontology::open_proposals(&f.pool, f.kb).await?;
    assert_eq!(
        open.len(),
        1,
        "the existing answer is a map_to proposal: {open:?}"
    );
    let m = &open[0];
    assert_eq!(m.section, "map_to");
    assert_eq!(m.key, "located_in");
    assert_eq!(m.proposed_by, "agent");
    assert_eq!(m.payload["kind"], "relation");
    assert_eq!(m.payload["direction"], "forward");
    assert_eq!(m.payload["forms"][0], "located in");
    assert_eq!(m.signatures["phrases"][0]["phrase"], "located in");

    // 第二轮：一条提过（map_to）、一条看过没提，词表没变——不问
    propose(&f.state, f.kb).await?;
    assert_eq!(f.requests().await, 1, "nothing new to look at");

    // 词表变了（多了一个类）：看过没提的 founded in 再送一次；map_to 已经提过的不送
    utopia_store::ontology::create_entity_type(
        &f.pool,
        f.kb,
        "city",
        "City",
        "#000",
        "circle",
        &[f.place],
        "a city",
    )
    .await?;
    propose(&f.state, f.kb).await?;
    assert_eq!(
        f.requests().await,
        2,
        "a changed glossary re-offers declined shapes"
    );
    let text = user_text(&f.model.requests.lock().unwrap()[1]);
    assert!(
        text.contains("founded in"),
        "the declined shape is offered again: {text}"
    );
    assert!(
        !text.contains("s1"),
        "the shape already proposed as map_to is not offered: {text}"
    );

    // 采纳 map_to：located in 这条形状成了人的绑定，绑到 located_in，正向
    let id = adopt(
        &f.state,
        f.kb,
        "map_to",
        "located_in",
        AdoptEdits::default(),
        f.user,
    )
    .await?;
    assert_eq!(id, f.located_in);
    let b = phrase_bindings::bindings(&f.pool, f.kb)
        .await?
        .into_iter()
        .find(|b| b.phrase == "located in")
        .expect("the binding exists");
    assert_eq!(b.status, "bound");
    assert_eq!(b.decided_by, "person");
    assert_eq!(b.relation_type_id, Some(f.located_in));
    assert_eq!(b.direction.as_deref(), Some("forward"));
    assert!(
        utopia_store::ontology::open_proposals(&f.pool, f.kb)
            .await?
            .is_empty(),
        "the map_to proposal is adopted"
    );
    // 第四轮：founded in 刚看过没提、located in 已经绑上——不问
    propose(&f.state, f.kb).await?;
    assert_eq!(f.requests().await, 2);
    f.cleanup().await
}
