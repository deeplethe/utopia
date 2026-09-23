//! 提规则与读数走脚本化的模型端点：提案落到队列，读数落进缓存并解析成库里的实体，
//! 然后排物化。没有 `UTOPIA_DATABASE_URL` 时跳过。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use utopia_store::implication_rules;

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<Value>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    m.requests.lock().unwrap().push(body);
    let text = m.replies.lock().unwrap().remove(0).to_string();
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
    film: Uuid,
    country_of_origin: Uuid,
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
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, film, coo, loud) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','implication-server');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','implication-server');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','implication-server');
             INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES ('{film}','{kb}','film','Film','#000','circle');
             INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
                 ('{coo}','{kb}','country_of_origin','country of origin','relation','state','the country a work comes from');
             INSERT INTO entities(id,kb_id,canonical_name,type_id,specific_type) VALUES ('{loud}','{kb}','Loud Tour','{film}','British film');"
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
            film,
            country_of_origin: coo,
            model,
            server,
            _dir: dir,
        }))
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
        Ok(())
    }
}

#[tokio::test]
async fn a_kind_word_is_offered_and_the_model_proposes_a_rule() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        json!({"i":[[0,"country_of_origin","country_of_nationality"]]}),
    ])
    .await?
    else {
        return Ok(());
    };
    let run = async {
        let kb = utopia_store::kbs::get(&f.pool, f.kb).await?;
        let settings = utopia_store::settings::get(&f.pool, kb.workspace_id)
            .await?
            .unwrap();
        let client = llm_util::chat_client(&settings).unwrap();
        let props = utopia_store::ontology::relation_type_views(&f.pool, f.kb).await?;
        let words = utopia_store::type_bindings::signatures(&f.pool, f.kb).await?;
        assert_eq!(words.len(), 1);
        let class_key: HashMap<Uuid, &str> = HashMap::from([(f.film, "film")]);
        let by_key: HashMap<&str, &RelationTypeView> =
            props.iter().map(|p| (p.key.as_str(), p)).collect();
        let asks = vec![RuleAsk {
            phrase: None,
            kind_word: Some(&words[0]),
            bound_to: None,
            candidates: props.iter().collect(),
            basis: "k1",
        }];
        let (proposed, failed) = propose_rules(
            &f.state, f.kb, &settings, &client, &asks, &class_key, &by_key,
        )
        .await?;
        assert_eq!((proposed, failed), (1, 0));
        let prompt = f.model.requests.lock().unwrap()[0]["messages"][1]["content"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(prompt.contains("kind word \"british film\""), "{prompt}");
        let rules = implication_rules::list(&f.pool, f.kb, Some("proposed")).await?;
        assert_eq!(rules.len(), 1);
        assert_eq!(
            (rules[0].trigger.as_str(), rules[0].reading.as_deref()),
            ("kind_word", Some("country_of_nationality"))
        );
        assert_eq!(rules[0].conclude_property_id, f.country_of_origin);
        // 队列里看得见
        let items = utopia_store::alignment_queue::list(&f.pool, f.kb, 10, 0).await?;
        assert!(items
            .iter()
            .any(|i| matches!(i, utopia_store::alignment_queue::AlignmentItem::Rule { .. })));
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}

#[tokio::test]
async fn read_phrases_fills_the_cache_names_a_thing_and_queues_the_materialization(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![json!({"r":[[0,"United Kingdom"]]})]).await? else {
        return Ok(());
    };
    let run = async {
        let votes = json!({});
        let id = implication_rules::propose(&f.pool, f.kb, &implication_rules::Proposal {
            trigger: "kind_word", phrase: "british film", subject_type_id: None, object_type_id: None, object_is_value: false,
            conclude_property_id: f.country_of_origin, reading: Some("country_of_nationality"), status: "proposed",
            votes: &votes, basis: "k1", statement_count: 1, examples: &[],
        }).await?.unwrap();
        implication_rules::decide_with_delivery(&f.pool, f.kb, id, true, &votes).await?;
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1").bind(f.kb.to_string()).execute(&f.pool).await?;
        read_phrases(&f.state, f.kb).await?;
        let (entity, value): (Option<Uuid>, Option<Value>) = sqlx::query_as(
            "SELECT entity_id, value FROM phrase_readings WHERE kb_id=$1 AND reading='country_of_nationality' AND phrase='british film'",
        ).bind(f.kb).fetch_one(&f.pool).await?;
        assert!(value.is_none());
        let name: String = sqlx::query_scalar("SELECT canonical_name FROM entities WHERE id=$1").bind(entity.unwrap()).fetch_one(&f.pool).await?;
        assert_eq!(name, "United Kingdom");
        let kinds: Vec<(String,)> = sqlx::query_as("SELECT kind FROM jobs WHERE payload->>'kb_id'=$1 AND status='queued'").bind(f.kb.to_string()).fetch_all(&f.pool).await?;
        assert_eq!(kinds, vec![(utopia_store::phrase_bindings::MATERIALIZE_KIND.to_string(),)]);
        // 物化：一条隐含行
        let o = utopia_store::materialize::materialize(&f.pool, f.kb).await?;
        assert_eq!(o.implied, 1);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
