//! Worker regressions use real ontology writes, statements and a local streaming model.
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Model {
    requests: Arc<Mutex<Vec<Value>>>,
    hold: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    let n = {
        let mut seen = m.requests.lock().unwrap();
        seen.push(body.clone());
        seen.len() - 1
    };
    if n == 0 && m.hold.load(std::sync::atomic::Ordering::SeqCst) {
        m.entered.notify_one();
        m.release.notified().await;
    }
    let user = body["messages"][1]["content"].as_str().unwrap();
    let mut votes = Vec::new();
    for item in user.split("Item ").skip(1) {
        let id: i64 = item.split(':').next().unwrap().parse().unwrap();
        let property = if item.contains("- located_in ·") {
            Some("located_in")
        } else if item.contains("- revenue ·") {
            Some("revenue")
        } else {
            None
        };
        let direction = if item.contains("phrase \"hosts\"") {
            "reverse"
        } else {
            "forward"
        };
        votes.push(json!([id, property, property.map(|_| direction)]));
    }
    let text = json!({"b":votes}).to_string();
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
    model: Model,
    server: tokio::task::JoinHandle<()>,
    dir: tempfile::TempDir,
}
impl Fx {
    async fn new() -> anyhow::Result<Option<Self>> {
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
            model,
            server,
            dir,
        }))
    }
    async fn run(&self) -> anyhow::Result<()> {
        tokio::time::timeout(
            std::time::Duration::from_secs(20),
            align_phrases(&self.state, self.kb),
        )
        .await?
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
}

impl Fx {
    async fn class(&self, key: &str, parents: &[Uuid]) -> anyhow::Result<Uuid> {
        Ok(utopia_store::ontology::create_entity_type(
            &self.pool, self.kb, key, key, "#123456", "circle", parents, key,
        )
        .await?)
    }
    async fn property(
        &self,
        key: &str,
        kind: &str,
        domains: &[Uuid],
        ranges: &[Uuid],
    ) -> anyhow::Result<Uuid> {
        Ok(utopia_store::ontology::create_relation_type(
            &self.pool,
            self.kb,
            key,
            key,
            "state",
            Default::default(),
            key,
            kind,
            domains,
            ranges,
            if kind == "attribute" {
                Some("number")
            } else {
                None
            },
            None,
        )
        .await?)
    }
    async fn statement(
        &self,
        name: &str,
        class: Option<Uuid>,
        object: Uuid,
        phrase: &str,
    ) -> anyhow::Result<Uuid> {
        let entity = utopia_store::resolution::resolve_mention(
            &self.pool,
            self.kb,
            class,
            name,
            None,
            None,
            &[],
        )
        .await?
        .entity_id;
        let statement = utopia_store::graph::insert_open_statement(
            &self.pool,
            self.kb,
            entity,
            phrase,
            utopia_store::graph::FactObject::Entity(object),
            None,
            1.0,
        )
        .await?
        .0;
        let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
        let object_name: String =
            sqlx::query_scalar("SELECT canonical_name FROM entities WHERE id=$1")
                .bind(object)
                .fetch_one(&self.pool)
                .await?;
        let quote = format!("{name} {phrase} {object_name}.");
        sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256) VALUES($1,$2,$3,$3)")
            .bind(doc)
            .bind(self.kb)
            .bind(doc.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,0,$4)")
            .bind(chunk)
            .bind(self.kb)
            .bind(doc)
            .bind(&quote)
            .execute(&self.pool)
            .await?;
        utopia_store::graph::add_evidence_located(
            &self.pool,
            statement,
            chunk,
            Some(&quote),
            Some(phrase),
            Some((0, quote.chars().count() as i32)),
        )
        .await?;
        Ok(statement)
    }
}

#[tokio::test]
async fn stale_no_candidate_retires_projection_and_converges() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let place = f.class("place", &[]).await?;
    let person = f.class("person", &[]).await?;
    let city = utopia_store::resolution::resolve_mention(
        &f.pool,
        f.kb,
        Some(place),
        "City",
        None,
        None,
        &[],
    )
    .await?
    .entity_id;
    let property = f
        .property("located_in", "relation", &[f.class], &[place])
        .await?;
    let statement = f
        .statement("Factory", Some(f.class), city, "located in")
        .await?;
    f.run().await?;
    assert_eq!(f.requests().len(), 2);
    let typed: Uuid = sqlx::query_scalar(
        "SELECT id FROM facts WHERE from_statement_id=$1 AND invalidated_at IS NULL",
    )
    .bind(statement)
    .fetch_one(&f.pool)
    .await?;
    utopia_store::ontology::update_relation_type(
        &f.pool,
        f.kb,
        property,
        "locatedIn",
        "state",
        Default::default(),
        "Only people may use this relation",
        None,
        None,
        Some(&[person]),
        Some(&[place]),
    )
    .await?;
    assert_eq!(phrase_bindings::stale(&f.pool, f.kb).await?.len(), 1);
    f.run().await?;
    assert_eq!(
        f.requests().len(),
        2,
        "no candidate must not call the model"
    );
    let binding = phrase_bindings::bindings(&f.pool, f.kb).await?.remove(0);
    assert_eq!(
        binding.status, "none",
        "stale automatic bound must be retired"
    );
    let retired: bool =
        sqlx::query_scalar("SELECT invalidated_at IS NOT NULL FROM facts WHERE id=$1")
            .bind(typed)
            .fetch_one(&f.pool)
            .await?;
    assert!(retired);
    let source_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM typed_fact_sources WHERE fact_id=$1")
            .bind(typed)
            .fetch_one(&f.pool)
            .await?;
    assert_eq!(source_count, 0);
    let open: bool = sqlx::query_scalar("SELECT invalidated_at IS NULL FROM facts WHERE id=$1")
        .bind(statement)
        .fetch_one(&f.pool)
        .await?;
    assert!(open, "original statement survives");
    let evidence: i64 = sqlx::query_scalar("SELECT count(*) FROM fact_evidence WHERE fact_id=$1")
        .bind(statement)
        .fetch_one(&f.pool)
        .await?;
    assert_eq!(evidence, 1, "original quote survives retirement");
    f.run().await?;
    assert!(phrase_bindings::stale(&f.pool, f.kb).await?.is_empty());
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .fetch_one(&f.pool)
        .await?;
    assert_eq!(jobs, 0, "unchanged input must not enqueue another run");
    utopia_store::ontology::update_relation_type(
        &f.pool,
        f.kb,
        property,
        "locatedIn",
        "state",
        Default::default(),
        "Organizations are eligible again",
        None,
        None,
        Some(&[f.class]),
        Some(&[place]),
    )
    .await?;
    f.run().await?;
    assert_eq!(
        f.requests().len(),
        4,
        "a later edit reopens the negative binding"
    );
    assert_eq!(
        phrase_bindings::bindings(&f.pool, f.kb).await?[0].status,
        "bound"
    );
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn structural_none_keeps_shared_person_support() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let place = f.class("place", &[]).await?;
    let person = f.class("person", &[]).await?;
    let city = utopia_store::resolution::resolve_mention(
        &f.pool,
        f.kb,
        Some(place),
        "City",
        None,
        None,
        &[],
    )
    .await?
    .entity_id;
    let property = f
        .property("located_in", "relation", &[f.class], &[place])
        .await?;
    let first = f
        .statement("Factory", Some(f.class), city, "located in")
        .await?;
    let second = f
        .statement("Factory", Some(f.class), city, "based in")
        .await?;
    f.run().await?;
    let sig = phrase_bindings::signatures(&f.pool, f.kb)
        .await?
        .into_iter()
        .find(|s| s.phrase == "based in")
        .unwrap();
    phrase_bindings::decide(
        &f.pool,
        f.kb,
        &sig,
        Decision {
            relation_type_id: Some(property),
            direction: Some("forward"),
            status: "bound",
            votes: &json!({"reason":"human"}),
            decided_by: "person",
        },
    )
    .await?;
    utopia_store::ontology::update_relation_type(
        &f.pool,
        f.kb,
        property,
        "locatedIn",
        "state",
        Default::default(),
        "people",
        None,
        None,
        Some(&[person]),
        Some(&[place]),
    )
    .await?;
    f.run().await?;
    let sources: Vec<Uuid> = sqlx::query_scalar("SELECT s.statement_id FROM typed_fact_sources s JOIN facts f ON f.id=s.fact_id WHERE f.kb_id=$1 AND f.invalidated_at IS NULL").bind(f.kb).fetch_all(&f.pool).await?;
    assert_eq!(sources, vec![second]);
    assert!(!sources.contains(&first));
    assert_eq!(f.requests().len(), 2);
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn over_limit_is_not_a_structural_negative() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let place = f.class("place", &[]).await?;
    let city = utopia_store::resolution::resolve_mention(
        &f.pool,
        f.kb,
        Some(place),
        "City",
        None,
        None,
        &[],
    )
    .await?
    .entity_id;
    f.statement("Factory", Some(f.class), city, "located in")
        .await?;
    for n in 0..=CANDIDATE_LIMIT {
        f.property(&format!("property_{n}"), "relation", &[f.class], &[place])
            .await?;
    }
    f.run().await?;
    assert!(f.requests().is_empty());
    assert!(
        phrase_bindings::bindings(&f.pool, f.kb).await?.is_empty(),
        "over-limit must remain undecided, not semantic none"
    );
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_person_deciding_during_the_worker_is_not_overwritten() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let place = f.class("place", &[]).await?;
    let person = f.class("person", &[]).await?;
    let city = utopia_store::resolution::resolve_mention(
        &f.pool,
        f.kb,
        Some(place),
        "City",
        None,
        None,
        &[],
    )
    .await?
    .entity_id;
    let property = f
        .property("located_in", "relation", &[f.class], &[place])
        .await?;
    f.statement("Factory", Some(f.class), city, "located in")
        .await?;
    let statement = f
        .statement("Person", Some(person), city, "person says")
        .await?;
    let sig = phrase_bindings::signatures(&f.pool, f.kb)
        .await?
        .into_iter()
        .find(|s| s.phrase == "person says")
        .unwrap();
    f.model
        .hold
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let state = f.state.clone();
    let kb = f.kb;
    let worker = tokio::spawn(async move { align_phrases(&state, kb).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        f.model.entered.notified(),
    )
    .await?;
    phrase_bindings::decide(
        &f.pool,
        f.kb,
        &sig,
        Decision {
            relation_type_id: Some(property),
            direction: Some("forward"),
            status: "bound",
            votes: &json!({"reason":"human"}),
            decided_by: "person",
        },
    )
    .await?;
    // New work arriving after the snapshot must still be queued by the existing tail check.
    f.statement("New factory", Some(f.class), city, "new wording")
        .await?;
    f.model.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(10), worker).await???;
    let binding = phrase_bindings::bindings(&f.pool, f.kb)
        .await?
        .into_iter()
        .find(|b| b.phrase == "person says")
        .unwrap();
    assert_eq!(binding.decided_by, "person");
    assert_eq!(binding.status, "bound");
    let sources: i64 =
        sqlx::query_scalar("SELECT count(*) FROM typed_fact_sources WHERE statement_id=$1")
            .bind(statement)
            .fetch_one(&f.pool)
            .await?;
    assert_eq!(sources, 1);
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .fetch_one(&f.pool)
        .await?;
    assert_eq!(jobs, 1);
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn an_empty_ontology_records_structural_none_without_model_calls() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    // Name resolution may create built-in properties, so use the existing untyped fixture entity.
    let entity: Uuid = sqlx::query_scalar("SELECT id FROM entities WHERE kb_id=$1 LIMIT 1")
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
    let value = json!({"value":100});
    utopia_store::graph::insert_open_statement(
        &f.pool,
        f.kb,
        entity,
        "reported revenue",
        utopia_store::graph::FactObject::Value(&value),
        None,
        1.0,
    )
    .await?;
    assert!(utopia_store::ontology::relation_type_views(&f.pool, f.kb)
        .await?
        .is_empty());
    f.run().await?;
    let bindings = phrase_bindings::bindings(&f.pool, f.kb).await?;
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].status, "none");
    assert!(f.requests().is_empty());
    f.run().await?;
    assert!(f.requests().is_empty());
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn a_saved_negative_is_materialized_after_a_failed_recompute() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let place = f.class("place", &[]).await?;
    let person = f.class("person", &[]).await?;
    let city = utopia_store::resolution::resolve_mention(
        &f.pool,
        f.kb,
        Some(place),
        "City",
        None,
        None,
        &[],
    )
    .await?
    .entity_id;
    let property = f
        .property("located_in", "relation", &[f.class], &[place])
        .await?;
    let statement = f
        .statement("Factory", Some(f.class), city, "located in")
        .await?;
    f.run().await?;
    utopia_store::ontology::update_relation_type(
        &f.pool,
        f.kb,
        property,
        "locatedIn",
        "state",
        Default::default(),
        "people",
        None,
        None,
        Some(&[person]),
        Some(&[place]),
    )
    .await?;
    // Inject failure only into this fixture's retirement. The decision is committed
    // before materialization; a retry must repair the projection with no new votes.
    let name = format!("audit_{}", f.kb.simple());
    sqlx::raw_sql(&format!("CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected retirement failure'; END $$; CREATE TRIGGER {name} BEFORE UPDATE OF invalidated_at ON facts FOR EACH ROW WHEN (OLD.kb_id='{}'::uuid AND OLD.layer='typed') EXECUTE FUNCTION {name}();", f.kb)).execute(&f.pool).await?;
    let result = f.run().await;
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {name} ON facts; DROP FUNCTION {name}();"
    ))
    .execute(&f.pool)
    .await?;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("injected retirement failure"));
    assert_eq!(
        phrase_bindings::bindings(&f.pool, f.kb).await?[0].status,
        "none"
    );
    f.run().await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM facts WHERE from_statement_id=$1 AND invalidated_at IS NULL",
    )
    .bind(statement)
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(count, 0);
    assert_eq!(f.requests().len(), 2);
    f.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn an_over_limit_stale_binding_is_not_falsely_rejected() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let place = f.class("place", &[]).await?;
    let city = utopia_store::resolution::resolve_mention(
        &f.pool,
        f.kb,
        Some(place),
        "City",
        None,
        None,
        &[],
    )
    .await?
    .entity_id;
    let property = f
        .property("located_in", "relation", &[f.class], &[place])
        .await?;
    f.statement("Factory", Some(f.class), city, "located in")
        .await?;
    f.run().await?;
    for n in 0..CANDIDATE_LIMIT {
        f.property(&format!("property_{n}"), "relation", &[f.class], &[place])
            .await?;
    }
    utopia_store::ontology::update_relation_type(
        &f.pool,
        f.kb,
        property,
        "locatedIn",
        "state",
        Default::default(),
        "revised definition",
        None,
        None,
        None,
        None,
    )
    .await?;
    f.run().await?;
    assert_eq!(f.requests().len(), 2);
    assert_eq!(
        phrase_bindings::bindings(&f.pool, f.kb).await?[0].status,
        "bound"
    );
    assert_eq!(phrase_bindings::stale(&f.pool, f.kb).await?.len(), 1);
    // Existing limitation: over-limit stale work still queues another run.
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .fetch_one(&f.pool)
        .await?;
    assert_eq!(jobs, 1);
    f.cleanup().await?;
    Ok(())
}
