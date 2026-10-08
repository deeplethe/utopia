//! An empty balance stops open extraction, fails the job once and raises the alert (#1095).
//! Skipped without `UTOPIA_DATABASE_URL`.
use super::*;
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::post, Router};
use serde_json::json;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// `Some` is a reply, `None` is the empty-balance 429 from OpenAI.
type Script = Arc<Mutex<Vec<Option<String>>>>;

async fn reply(State(script): State<Script>) -> axum::response::Response {
    let Some(text) = script.lock().unwrap().remove(0) else {
        let body = json!({"error": {"message": "You exceeded your current quota.",
            "type": "insufficient_quota", "code": "credit_balance_exhausted"}});
        return (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
    };
    let frame = json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]});
    (
        [("content-type", "text/event-stream")],
        format!("data: {frame}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

#[tokio::test]
async fn an_empty_balance_stops_the_document_and_keeps_what_was_applied() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let [org, ws, kb, doc] = [(); 4].map(|_| Uuid::now_v7());
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','open-out-of-credit');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','open-out-of-credit');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','open-out-of-credit');
         INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','acme.txt','x');
         INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
             (gen_random_uuid(),'{kb}','{doc}',0,'Acme is based in London.'),
             (gen_random_uuid(),'{kb}','{doc}',1,'Acme runs the harbour facility.'),
             (gen_random_uuid(),'{kb}','{doc}',2,'Acme was founded in 1990.');"
    ))
    .execute(&pool)
    .await?;
    let first = json!({
        "e": [["Acme", "organization", 1]],
        "s": [["Acme is based in London.", "Acme", "based in", null, "London", null, null, null]],
        "n": []
    });
    let script: Script = Arc::new(Mutex::new(vec![Some(first.to_string()), None, None]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let router = Router::new()
        .route("/chat/completions", post(reply))
        .with_state(script.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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
    let alerts_before: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM alerts")
        .fetch_all(&pool)
        .await?;

    let run = async {
        let Err(err) = crate::extraction::extract_document(&state, doc, Proposer::default()).await
        else {
            anyhow::bail!("extraction succeeded with no balance");
        };
        // As `main.rs` does for a hopeless error
        anyhow::ensure!(crate::alerting::hopeless(&err), "not hopeless: {err:#}");
        let err = err.context(utopia_core::Terminal);
        let job = utopia_store::jobs::Job {
            id: 0,
            kind: "extract_document".into(),
            payload: json!({}),
            attempts: 1,
            max_attempts: 3,
        };
        crate::alerting::observe_job_failure(&state, &job, &err).await;
        let alerts: Vec<String> = sqlx::query_scalar(
            "DELETE FROM alerts WHERE NOT (id = ANY($1)) AND detail->>'job' = 'extract_document'
             RETURNING kind",
        )
        .bind(&alerts_before)
        .fetch_all(&pool)
        .await?;
        let (facts, extracted, status): (i64, Vec<bool>, String) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM facts WHERE kb_id=$1 AND layer='open'),
                    (SELECT array_agg(extracted_at IS NOT NULL ORDER BY seq)
                       FROM chunks WHERE document_id=$2),
                    graph_status
               FROM documents WHERE id=$2",
        )
        .bind(kb)
        .bind(doc)
        .fetch_one(&pool)
        .await?;
        Ok((alerts, facts, extracted, status))
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    let (alerts, facts, extracted, status) = run?;
    assert_eq!(
        script.lock().unwrap().len(),
        1,
        "the third chunk is not called"
    );
    assert_eq!(alerts, vec![utopia_store::alerts::kind::LLM_OUT_OF_CREDIT]);
    assert_eq!(facts, 1, "chunk 0 stays in the graph");
    assert_eq!(
        extracted,
        vec![true, false, false],
        "the next run resumes at chunk 1"
    );
    assert_eq!(status, "failed");
    Ok(())
}
