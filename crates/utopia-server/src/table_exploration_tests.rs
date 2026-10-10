//! Exercise the real schema reader and store with a scripted model. No external
//! provider or business data is needed; SQL and rollback must actually run.
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use std::sync::{Arc, Mutex};
use utopia_store::table_alignments as store;

#[tokio::test]
async fn two_owners_reuse_an_attribute_but_keep_separate_raw_column_bindings() -> anyhow::Result<()>
{
    let Some(fx) = Fx::new().await? else {
        return Ok(());
    };
    let mut raw = fx.raw("orders");
    raw["attribute_types"] = json!([{"key":"code","label":"code","description":"a numeric code","domains":["order","customer"],"datatype":"number"}]);
    raw["columns"][0]["property"] = json!("code");
    raw["columns"][0]["expression"] = json!("amt_pay");
    raw["columns"][2]["property"] = json!("code");
    raw["columns"][2]["expression"] = json!("buyer_lvl");
    let mut ambiguous = raw.clone();
    ambiguous["columns"][0]["expression"] = json!("amt_pay + buyer_lvl");
    assert!(fx.prepare(ambiguous).await.is_err());
    let p = fx.prepare(raw).await?;
    store::save(&fx.pool, fx.kb, &p).await?;
    store::decide(
        &fx.pool,
        fx.kb,
        &store::key(fx.source, &p.draft.table),
        p.version,
        true,
        fx.actor,
    )
    .await?;
    let rows:Vec<(String,Uuid,Value)>=sqlx::query_as("SELECT c.column_name,c.property_id,c.input_columns FROM table_alignment_columns c JOIN table_alignments a ON a.id=c.alignment_id WHERE a.kb_id=$1 AND c.expression IS NOT NULL ORDER BY c.column_name")
        .bind(fx.kb).fetch_all(&fx.pool).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].1, rows[1].1);
    for (column, id, inputs) in &rows {
        assert_eq!(inputs[&id.to_string()], json!(column));
    }
    assert!(matches!(
        utopia_store::ontology::delete_relation_type(&fx.pool, fx.kb, rows[0].1).await,
        Err(AppError::Conflict(_))
    ));
    let class: Uuid = sqlx::query_scalar("SELECT class_id FROM table_alignments WHERE kb_id=$1")
        .bind(fx.kb)
        .fetch_one(&fx.pool)
        .await?;
    assert!(matches!(
        utopia_store::ontology::delete_entity_type(&fx.pool, fx.kb, class).await,
        Err(AppError::Conflict(_))
    ));
    fx.cleanup().await
}

#[derive(Clone)]
struct Model(Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>);
async fn reply(State(model): State<Model>, Json(request): Json<Value>) -> impl IntoResponse {
    model.1.lock().unwrap().push(request);
    let text = model.0.lock().unwrap().remove(0).to_string();
    Json(json!({"choices":[{"message":{"role":"assistant","content":text}}]}))
}

struct Fx {
    pool: sqlx::PgPool,
    state: crate::state::AppState,
    kb: Uuid,
    org: Uuid,
    actor: Uuid,
    source: Uuid,
    schema: String,
    model: Model,
    server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fx {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        // Holding a transaction and then acquiring a second connection would
        // hang this fixture, exposing accidental pool calls inside adoption.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, actor, source) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let schema = format!("alignment_{}", kb.simple());
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES('{org}','alignment');
             INSERT INTO workspaces(id,org_id,name) VALUES('{ws}','{org}','alignment');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES('{kb}','{ws}','alignment');
             INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES('{actor}','{org}','{actor}@alignment.test','unused','Reviewer');
             CREATE SCHEMA {schema};
             CREATE TABLE {schema}.orders(amt_pay bigint,buyer_id bigint,buyer_lvl integer,etl_dt timestamptz);
             CREATE TABLE {schema}.archive(LIKE {schema}.orders);"
        )).execute(&pool).await?;
        sqlx::query(
            "INSERT INTO data_sources(id,name,engine,conn_string) VALUES($1,$2,'postgres',$3)",
        )
        .bind(source)
        .bind(format!("alignment-{source}"))
        .bind(utopia_core::secrets::seal(&url))
        .execute(&pool)
        .await?;
        utopia_store::datasources::mount(&pool, kb, source).await?;
        let model = Model(Default::default(), Default::default());
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
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
        Ok(Some(Self {
            pool,
            state,
            kb,
            org,
            actor,
            source,
            schema,
            model,
            server,
            _dir: dir,
        }))
    }

    fn raw(&self, table: &str) -> Value {
        json!({"source":format!("alignment-{}",self.source),"table":format!("{}.{table}",self.schema),"class":"order","summary":"One row per order; amount in cents",
            "entity_types":[
                {"key":"order","label":"Order","description":"a purchase order"},
                {"key":"customer","label":"Customer","description":"the purchaser"}],
            "attribute_types":[
                {"key":"paid_amount","label":"paidAmount","description":"paid amount","domains":["order"],"datatype":"number","unit":"CNY"},
                {"key":"tier","label":"tier","description":"customer tier","domains":["customer"],"datatype":"text"}],
            "relation_types":[{"key":"buyer","label":"buyer","description":"the purchaser","domains":["order"],"ranges":["customer"]}],
            "columns":[
                {"column":"amt_pay","class":"order","property":"paid_amount","expression":"CAST(amt_pay AS DOUBLE PRECISION) / 100"},
                {"column":"buyer_id","class":"order","property":"buyer","target_class":"customer","expression":null},
                {"column":"buyer_lvl","class":"customer","property":"tier","expression":"CASE buyer_lvl WHEN 2 THEN 'gold' ELSE 'standard' END"}],
            "omitted":[{"column":"etl_dt","reason":"ETL bookkeeping"}]})
    }

    async fn columns(&self) -> anyhow::Result<Vec<crate::query_engine::SchemaColumn>> {
        let (engine, conn) =
            utopia_store::datasources::engine_and_conn(&self.pool, self.source).await?;
        crate::query_engine::engine_for(&engine, &conn)?
            .fetch_schema()
            .await
    }

    async fn prepare(&self, raw: Value) -> AppResult<Proposal> {
        prepare(
            &self.pool,
            self.kb,
            self.source,
            &format!("alignment-{}", self.source),
            raw,
            &self.columns().await.unwrap(),
        )
        .await
    }

    async fn counts(&self) -> anyhow::Result<(i64, i64, i64, i64)> {
        Ok(sqlx::query_as(
            "SELECT
            (SELECT count(*) FROM entity_types WHERE kb_id=$1),
            (SELECT count(*) FROM relation_types WHERE kb_id=$1),
            (SELECT count(*) FROM table_alignments WHERE kb_id=$1),
            (SELECT count(*) FROM entities WHERE kb_id=$1)",
        )
        .bind(self.kb)
        .fetch_one(&self.pool)
        .await?)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        self.server.abort();
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM data_sources WHERE id=$1")
            .bind(self.source)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        sqlx::raw_sql(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn exploration_waits_for_a_whole_table_decision_and_stores_executable_conversions(
) -> anyhow::Result<()> {
    let Some(fx) = Fx::new().await? else {
        return Ok(());
    };
    fx.model.0.lock().unwrap().extend([
        json!({"description":"Orders with purchaser tier","questions":[]}),
        json!([fx.raw("orders")]),
    ]);
    crate::mappings::explore_mappings(&fx.state, fx.kb).await?;
    assert_eq!(
        fx.counts().await?,
        (0, 0, 0, 0),
        "exploration cannot create ontology or graph rows"
    );
    let items = store::list(&fx.pool, fx.kb).await?;
    assert_eq!(items.len(), 1);
    let p: Proposal = serde_json::from_value(items[0].payload.clone())?;
    assert!(p.draft.columns[0].expression.as_ref().unwrap().is_object());
    decide(&fx.state, fx.kb, &items[0].key, p.version, true, fx.actor).await?;
    assert_eq!(fx.counts().await?, (2, 3, 1, 0));
    let rows: Vec<(String,Uuid,Value)> = sqlx::query_as(
        "SELECT c.column_name,c.property_id,c.expression FROM table_alignment_columns c
         JOIN table_alignments a ON a.id=c.alignment_id WHERE a.kb_id=$1 AND c.expression IS NOT NULL")
        .bind(fx.kb).fetch_all(&fx.pool).await?;
    for (column, id, tree) in rows {
        use utopia_reason::{expressions::Scalar, rules::Expr};
        let result = Expr::from_json(&tree)
            .unwrap()
            .evaluate(&|input| {
                assert_eq!(
                    input, id,
                    "persisted leaves name actual ontology attributes"
                );
                Some(Scalar::Number(if column == "amt_pay" {
                    12345.0
                } else {
                    2.0
                }))
            })
            .unwrap()
            .to_json();
        assert_eq!(
            result,
            if column == "amt_pay" {
                json!(123.45)
            } else {
                json!("gold")
            }
        );
    }
    assert!(
        !store::save(&fx.pool, fx.kb, &p).await?,
        "a rerun cannot replace the decision"
    );
    let runs = utopia_store::exploration_runs::recent(&fx.pool, fx.kb, 1).await?;
    assert_eq!(runs[0].accepted, 1);
    assert_eq!(fx.model.1.lock().unwrap().len(), 2);
    fx.cleanup().await
}

#[tokio::test]
async fn adoption_rolls_back_every_write_and_serializes_double_clicks() -> anyhow::Result<()> {
    let Some(fx) = Fx::new().await? else {
        return Ok(());
    };
    let p = fx.prepare(fx.raw("orders")).await?;
    store::save(&fx.pool, fx.kb, &p).await?;
    let key = store::key(fx.source, &p.draft.table);
    // The final decision write fails its actor FK, after the class/property,
    // alignment, column and queue writes have all run.
    assert!(
        store::decide(&fx.pool, fx.kb, &key, p.version, true, Uuid::now_v7())
            .await
            .is_err()
    );
    assert_eq!(fx.counts().await?, (0, 0, 0, 0));
    assert_eq!(store::list(&fx.pool, fx.kb).await?[0].status, "open");
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(fx.kb.to_string())
        .fetch_one(&fx.pool)
        .await?;
    assert_eq!(queued, 0);
    let (a, b) = tokio::join!(
        store::decide(&fx.pool, fx.kb, &key, p.version, true, fx.actor),
        store::decide(&fx.pool, fx.kb, &key, p.version, true, fx.actor)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(fx.counts().await?, (2, 3, 1, 0));
    // A second table can reuse the same declarations, including attributes
    // proposed before either table was adopted, without changing their meaning.
    let second = fx.prepare(fx.raw("archive")).await?;
    store::save(&fx.pool, fx.kb, &second).await?;
    store::decide(
        &fx.pool,
        fx.kb,
        &store::key(fx.source, &second.draft.table),
        second.version,
        true,
        fx.actor,
    )
    .await?;
    assert_eq!(fx.counts().await?, (2, 3, 2, 0));
    fx.cleanup().await
}

#[tokio::test]
async fn a_refresh_rejection_or_unmount_cannot_be_bypassed_by_an_old_review() -> anyhow::Result<()>
{
    let Some(fx) = Fx::new().await? else {
        return Ok(());
    };
    let first = fx.prepare(fx.raw("orders")).await?;
    store::save(&fx.pool, fx.kb, &first).await?;
    let second = fx.prepare(fx.raw("orders")).await?;
    store::save(&fx.pool, fx.kb, &second).await?;
    let key = store::key(fx.source, &first.draft.table);
    assert!(
        store::decide(&fx.pool, fx.kb, &key, first.version, true, fx.actor)
            .await
            .is_err()
    );
    assert!(store::decide(
        &fx.pool,
        Uuid::now_v7(),
        &key,
        second.version,
        true,
        fx.actor
    )
    .await
    .is_err());
    store::decide(&fx.pool, fx.kb, &key, second.version, false, fx.actor).await?;
    assert!(!store::save(&fx.pool, fx.kb, &first).await?);
    assert_eq!(store::list(&fx.pool, fx.kb).await?[0].status, "rejected");
    assert_eq!(fx.counts().await?, (0, 0, 0, 0));
    let other = fx.prepare(fx.raw("archive")).await?;
    store::save(&fx.pool, fx.kb, &other).await?;
    utopia_store::datasources::unmount(&fx.pool, fx.kb, fx.source).await?;
    assert!(store::list(&fx.pool, fx.kb).await?.is_empty());
    assert!(store::decide(
        &fx.pool,
        fx.kb,
        &store::key(fx.source, &other.draft.table),
        other.version,
        true,
        fx.actor
    )
    .await
    .is_err());
    fx.cleanup().await
}

#[tokio::test]
async fn invalid_columns_signatures_and_expressions_never_become_proposals() -> anyhow::Result<()> {
    let Some(fx) = Fx::new().await? else {
        return Ok(());
    };
    let base = fx.raw("orders");
    for (pointer, value) in [
        ("/table", json!("missing.orders")),
        ("/columns/0/column", json!("does_not_exist")),
        ("/columns/0/class", json!("customer")),
        ("/columns/0/expression", json!("SUM(amt_pay)")),
        ("/columns/0/expression", json!("missing / 100")),
        ("/columns/0/expression", json!({"attr":Uuid::now_v7()})),
        ("/columns/1/target_class", json!("order")),
        ("/columns/1/expression", json!("buyer_id")),
        ("/attribute_types/0/datatype", json!("money")),
        ("/omitted/0/column", json!("amt_pay")),
    ] {
        let mut raw = base.clone();
        *raw.pointer_mut(pointer)
            .unwrap_or_else(|| panic!("missing {pointer}")) = value;
        assert!(fx.prepare(raw).await.is_err(), "accepted {pointer}");
    }
    let p = fx.prepare(base).await?;
    store::save(&fx.pool, fx.kb, &p).await?;
    // The schema can change while a person is reviewing it. Re-read before
    // adoption rather than trusting the exploration's earlier snapshot.
    sqlx::raw_sql(&format!(
        "ALTER TABLE {}.orders DROP COLUMN buyer_lvl",
        fx.schema
    ))
    .execute(&fx.pool)
    .await?;
    assert!(decide(
        &fx.state,
        fx.kb,
        &store::key(fx.source, &p.draft.table),
        p.version,
        true,
        fx.actor
    )
    .await
    .is_err());
    assert_eq!(fx.counts().await?, (0, 0, 0, 0));
    fx.cleanup().await
}
