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

    /// 库里有一条人写的问题：本体代理就不先去提问题（那是没问题的库才走的一步）
    async fn seed_question(&self) -> anyhow::Result<Uuid> {
        Ok(competency_questions::create(
            &self.pool,
            self.kb,
            competency_questions::NewQuestion {
                question: "Where was each organization founded?",
                expected_answer: None,
                needs: None,
                origin: "person",
                status: "accepted",
                created_by: None,
            },
        )
        .await?)
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
    let qid = f.seed_question().await?;

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
    f.seed_question().await?;
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
async fn a_declined_shape_returns_when_its_statements_double_and_an_existing_answer_becomes_a_binding(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        // 第一轮：founded in 没提；located in 本体里已经有（located_in，正向）
        json!({ "p": [], "existing": [{ "key": "located_in", "dir": "forward", "s": [1], "k": [] }] }),
        // 陈述翻倍之后的那轮：只有 founded in 送来（这一批里它是 s0），答成 located_in 反着读
        json!({ "p": [], "existing": [{ "key": "located_in", "dir": "reverse", "s": [0], "k": [] }] }),
    ])
    .await?
    else {
        return Ok(());
    };
    f.decide_none().await?;
    f.seed_question().await?;

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
    assert_eq!(
        m.payload["label"], "located in",
        "the target's label travels with it"
    );
    assert_eq!(m.payload["forms"][0], "located in");
    assert_eq!(m.signatures["phrases"][0]["phrase"], "located in");
    assert_eq!(m.signatures["phrases"][0]["direction"], "forward");

    // 第二轮：一条提过（map_to）、一条看过没提，词表没变——不问
    propose(&f.state, f.kb).await?;
    assert_eq!(f.requests().await, 1, "nothing new to look at");

    // 词表变了（多了一个类）：看过没提的不再问——对齐自己会拿新元素去重判，问代理只是烧 token
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
        1,
        "a changed glossary alone does not re-offer declined shapes"
    );

    // founded in 的陈述翻倍了（1 → 2）：再送一次；map_to 已经提过的 located in 不送
    sqlx::query(
        "INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase)
         SELECT $1, kb_id, subject_id, object_id, 'open', 'founded in' FROM facts
          WHERE kb_id=$2 AND phrase='founded in' LIMIT 1",
    )
    .bind(Uuid::now_v7())
    .bind(f.kb)
    .execute(&f.pool)
    .await?;
    propose(&f.state, f.kb).await?;
    assert_eq!(
        f.requests().await,
        2,
        "a declined shape with twice the statements is offered again"
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

    // 这一轮的答案并进同一条 map_to：两条形状，各自的方向
    let open = utopia_store::ontology::open_proposals(&f.pool, f.kb).await?;
    assert_eq!(open.len(), 1, "still one map_to row: {open:?}");
    let shapes = open[0].signatures["phrases"].as_array().unwrap().clone();
    assert_eq!(
        shapes.len(),
        2,
        "shapes from both rounds are kept: {shapes:?}"
    );
    assert_eq!(shapes[0]["phrase"], "located in");
    assert_eq!(shapes[0]["direction"], "forward");
    assert_eq!(shapes[1]["phrase"], "founded in");
    assert_eq!(shapes[1]["direction"], "reverse");
    assert_eq!(
        open[0].payload["forms"],
        json!(["located in", "founded in"])
    );

    // 采纳 map_to：两条形状都成了人的绑定，绑到 located_in，各按自己的方向
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
    let bindings = phrase_bindings::bindings(&f.pool, f.kb).await?;
    for (phrase, dir) in [("located in", "forward"), ("founded in", "reverse")] {
        let b = bindings
            .iter()
            .find(|b| b.phrase == phrase)
            .expect("the binding exists");
        assert_eq!(b.status, "bound", "{phrase}");
        assert_eq!(b.decided_by, "person", "{phrase}");
        assert_eq!(b.relation_type_id, Some(f.located_in), "{phrase}");
        assert_eq!(b.direction.as_deref(), Some(dir), "{phrase}");
    }
    assert!(
        utopia_store::ontology::open_proposals(&f.pool, f.kb)
            .await?
            .is_empty(),
        "the map_to proposal is adopted"
    );
    // 再来一轮：两条都绑上了——不问
    propose(&f.state, f.kb).await?;
    assert_eq!(f.requests().await, 2);
    f.cleanup().await
}

#[tokio::test]
async fn a_base_with_no_questions_gets_them_proposed_and_the_report_counts_them(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        // 本体代理发现库里一条问题都没有：先提问题（这一轮的第一次调用）
        json!({ "q": [
            { "question": "Where is each organization located?", "classes": ["organization", "place"], "properties": ["located_in", "made_up"] },
            { "question": "Why?", "classes": [], "properties": [] }
        ] }),
        // 然后照旧按说法提本体
        json!({ "p": [], "existing": [] }),
        // 界面上再叫一次提问题：同一句不重复，新的进来
        json!({ "q": [
            { "question": "where is each ORGANIZATION located?", "classes": [], "properties": [] },
            { "question": "Which organizations were founded in each place?", "classes": ["organization"], "properties": [] }
        ] }),
    ])
    .await?
    else {
        return Ok(());
    };
    f.decide_none().await?;

    propose(&f.state, f.kb).await?;
    assert_eq!(
        f.requests().await,
        2,
        "questions first, then the ontology round"
    );
    let text = user_text(&f.model.requests.lock().unwrap()[0]);
    assert!(text.contains("Most frequent statement shapes"), "{text}");
    assert!(
        text.contains("\"founded in\""),
        "the shapes are offered: {text}"
    );
    assert!(
        text.contains("Acme (organization)"),
        "the connected entities are offered: {text}"
    );
    let qs = competency_questions::list(&f.pool, f.kb).await?;
    assert_eq!(qs.len(), 1, "the short one is dropped: {qs:?}");
    assert_eq!(qs[0].origin, "agent");
    assert_eq!(qs[0].status, "proposed");
    assert_eq!(
        qs[0].needs,
        Some(json!({ "classes": ["organization", "place"], "properties": ["located_in"] })),
        "keys the ontology lacks are dropped from needs"
    );
    // 提出来的还没接受：本体那一轮没有问题可用
    let text = user_text(&f.model.requests.lock().unwrap()[1]);
    assert!(text.contains("(none written yet"), "{text}");

    let written = propose_questions(&f.state, f.kb).await?;
    assert_eq!(written, 1, "the duplicate is skipped, the new one written");
    assert_eq!(f.requests().await, 3);
    let text = user_text(&f.model.requests.lock().unwrap()[2]);
    assert!(text.contains("do not repeat them"), "{text}");
    assert!(
        text.contains("Where is each organization located?"),
        "{text}"
    );

    // 接受一条、问过一条：报告数得出来
    let qs = competency_questions::list(&f.pool, f.kb).await?;
    competency_questions::set_status(&f.pool, f.kb, qs[0].id, "accepted").await?;
    competency_questions::set_status(&f.pool, f.kb, qs[1].id, "accepted").await?;
    assert!(
        competency_questions::record_result(
            &f.pool,
            f.kb,
            qs[0].id,
            &json!({ "answered": true, "judged_by": "shape" })
        )
        .await?
    );
    let r = competency_questions::report(&f.pool, f.kb).await?;
    assert_eq!(
        (r.accepted, r.proposed, r.checked, r.answered),
        (2, 0, 1, 1)
    );
    let p = utopia_store::ontology::agent_proposal_report(&f.pool, f.kb).await?;
    assert_eq!(
        (p.open, p.adopted, p.adopted_edited, p.rejected),
        (0, 0, 0, 0)
    );
    f.cleanup().await
}
