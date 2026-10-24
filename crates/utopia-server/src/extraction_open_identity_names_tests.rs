//! A statements source with `{"names": "identity"}` (0071) gets no embedding call and no
//! `name_vector|` pair, and the name it pushes loses the vector prose gave it first. The fake
//! endpoint returns one vector for every name.
use super::*;
use axum::{routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Push `PUSHED` and `KNOWN` into a base holding `KNOWN` (with a name vector); answers
/// (embedding calls, review reasons, whether `KNOWN` kept its vector).
async fn push(config: Value) -> anyhow::Result<Option<(usize, Vec<String>, bool)>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let [org, ws, kb, source, doc, chunk, known_id]: [Uuid; 7] =
        std::array::from_fn(|_| Uuid::now_v7());
    let text = serde_json::to_string(&json!({
        "external_id": "commit:identity",
        "e": [[PUSHED, "SourceFile", true], [KNOWN, "SourceFile", true]],
        "s": [[null, PUSHED, "path", null, "src/x.ts", null, null, null]],
        "n": []
    }))?;
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','identity-names');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','identity-names');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','identity-names');
         INSERT INTO sources(id,kb_id,kind,name,config) VALUES
             ('{source}','{kb}','statements','lore',$${config}$$::jsonb);
         INSERT INTO documents(id,kb_id,filename,sha256,source_id) VALUES
             ('{doc}','{kb}','commit.json','x{doc}','{source}');
         INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
             ('{chunk}','{kb}','{doc}',0,$${text}$$);"
    ))
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,$3)")
        .bind(known_id)
        .bind(kb)
        .bind(KNOWN)
        .execute(&pool)
        .await?;
    let fact = utopia_store::names::record(&pool, kb, known_id, KNOWN, None, None)
        .await?
        .expect("a named entity gets a name fact");
    utopia_store::name_vectors::set(&pool, kb, &[(fact, known_id, vec![1.0, 0.0, 0.0, 0.0])])
        .await?;

    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let router = Router::new().route(
        "/embeddings",
        post(move |Json(body): Json<Value>| async move {
            counted.fetch_add(1, Ordering::SeqCst);
            let inputs = body["input"].as_array().map_or(0, Vec::len);
            Json(json!({"data": vec![json!({"embedding": [1.0, 0.0, 0.0, 0.0]}); inputs]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    utopia_store::settings::upsert(
        &pool,
        ws,
        None,
        None,
        None,
        Some(&base),
        None,
        Some("fake-embed"),
        Some(4),
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
    crate::extraction::extract_document(&state, doc, utopia_core::models::Proposer::default())
        .await?;

    let reasons: Vec<String> = sqlx::query_scalar(
        "SELECT reason FROM resolution_reviews WHERE kb_id = $1 ORDER BY reason",
    )
    .bind(kb)
    .fetch_all(&pool)
    .await?;
    let known_kept: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM name_vectors WHERE fact_id = $1)")
            .bind(fact)
            .fetch_one(&pool)
            .await?;

    server.abort();
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(Some((calls.load(Ordering::SeqCst), reasons, known_kept)))
}

const KNOWN: &str = "re-cinq/lore:src/a.ts";
const PUSHED: &str = "re-cinq/lore:src/b.ts";

#[tokio::test]
async fn an_identity_source_adds_no_name_vectors_and_asks_no_neighbours() -> anyhow::Result<()> {
    let Some((calls, reasons, _)) = push(json!({"names": "identity"})).await? else {
        return Ok(());
    };
    assert_eq!((calls, reasons), (0, Vec::<String>::new()));
    Ok(())
}

#[tokio::test]
async fn a_name_prose_gave_a_vector_loses_it_when_an_identity_source_pushes_it(
) -> anyhow::Result<()> {
    let Some((_, _, known_kept)) = push(json!({"names": "identity"})).await? else {
        return Ok(());
    };
    assert!(!known_kept);
    Ok(())
}

#[tokio::test]
async fn a_statements_source_without_the_flag_still_proposes_by_name_vector() -> anyhow::Result<()>
{
    let Some((calls, reasons, known_kept)) = push(json!({})).await? else {
        return Ok(());
    };
    assert!(calls > 0 && reasons.iter().any(|r| r.starts_with("name_vector|")) && known_kept);
    Ok(())
}
