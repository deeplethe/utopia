//! Provider rejection reaches real retrieval before any plain RAG request.
use super::*;
use axum::http::StatusCode;

#[derive(Clone)]
struct FallbackModel {
    pool: sqlx::PgPool,
    break_retrieval: bool,
    status: u16,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn respond(
    State(m): State<FallbackModel>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    let tools = body.get("tools").is_some();
    m.seen.lock().unwrap().push(body);
    if tools {
        // Authentication, settings and user persistence have already succeeded.
        // Only the ensuing retrieval sees the closed database pool.
        if m.break_retrieval {
            m.pool.close().await;
        }
        return (
            StatusCode::from_u16(m.status).unwrap(),
            Json(json!({"error":{"message":"tools unsupported"}})),
        )
            .into_response();
    }
    ([ ("content-type", "text/event-stream") ], "data: {\"choices\":[{\"delta\":{\"content\":\"Fallback answer.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").into_response()
}

async fn exercise(status: u16, break_retrieval: bool, hit: bool) -> anyhow::Result<()> {
    let Some(mut f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    if hit {
        let doc = utopia_store::documents::create(
            &f.pool,
            f.kb,
            "fallback.md",
            "text/plain",
            20,
            "fallback-test",
            None,
            None,
            None,
        )
        .await?;
        let chunks = utopia_store::documents::replace_chunks(
            &f.pool,
            f.kb,
            doc.id,
            &[utopia_ingest::ChunkPiece {
                seq: 0,
                text: "orchard evidence".into(),
                char_start: 0,
                char_end: 16,
                heading: None,
                provenance: utopia_ingest::Provenance::stated(),
            }],
        )
        .await?;
        f.state
            .search
            .reindex_document(&f.kb.to_string(), &doc.id.to_string(), &chunks)?;
    }
    let model = FallbackModel {
        pool: f.pool.clone(),
        break_retrieval,
        status,
        seen: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = axum::Router::new()
        .route("/chat/completions", axum::routing::post(respond))
        .route(
            "/embeddings",
            axum::routing::post(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .with_state(model.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    sqlx::query("UPDATE llm_settings SET chat_base_url=$1,embed_base_url=$1,embed_model=$2 WHERE workspace_id=(SELECT workspace_id FROM knowledge_bases WHERE id=$3)")
        .bind(base).bind(if hit {Some("broken-embedding")} else {None}).bind(f.kb).execute(&f.pool).await?;
    let response = tokio::time::timeout(std::time::Duration::from_secs(20), f.ask("orchard")).await;
    // Reconnect solely to read the result and clean the isolated fixture.
    if break_retrieval {
        f.pool = sqlx::PgPool::connect(&utopia_store::test_db::url().unwrap()).await?;
    }
    let result = async {
        let sse = response??;
        let seen = model.seen.lock().unwrap().clone();
        let plain: Vec<_> = seen.iter().filter(|r| r.get("tools").is_none()).collect();
        let rejected = status == 400 || status == 422;
        if break_retrieval || !rejected {
            anyhow::ensure!(
                sse.contains("event: error") && !sse.contains("event: done"),
                "{sse}"
            );
            anyhow::ensure!(
                plain.is_empty(),
                "retrieval/provider failure must not request a RAG answer: {seen:?}"
            );
            anyhow::ensure!(f.stored_answer().await?.is_none());
            if break_retrieval {
                anyhow::ensure!(sse.contains("Could not search the documents."), "{sse}");
                anyhow::ensure!(!sse.contains("pool closed") && !sse.contains("postgres"));
            }
        } else {
            anyhow::ensure!(
                sse.contains("event: done") && sse.contains("Fallback answer."),
                "{sse}"
            );
            anyhow::ensure!(plain.len() == 1);
            anyhow::ensure!(f.stored_answer().await?.as_deref() == Some("Fallback answer."));
            if hit {
                anyhow::ensure!(
                    plain[0].to_string().contains("orchard evidence")
                        && sse.contains("fallback.md")
                );
            } else {
                anyhow::ensure!(sse.contains("data: []"));
            }
        }
        anyhow::ensure!(
            seen.iter().filter(|r| r.get("tools").is_some()).count()
                == if rejected { 2 } else { 1 },
            "compatibility retries: {seen:?}"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;
    server.abort();
    let _ = server.await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn fallback_retrieval_failure_is_not_an_empty_result() -> anyhow::Result<()> {
    exercise(400, true, false).await
}
#[tokio::test]
async fn fallback_empty_search_still_answers() -> anyhow::Result<()> {
    exercise(400, false, false).await
}
#[tokio::test]
async fn fallback_embedding_failure_keeps_bm25_evidence() -> anyhow::Result<()> {
    exercise(422, false, true).await
}
#[tokio::test]
async fn other_provider_failures_do_not_enter_fallback() -> anyhow::Result<()> {
    for status in [401, 402, 429, 500] {
        exercise(status, false, false).await?;
    }
    Ok(())
}
