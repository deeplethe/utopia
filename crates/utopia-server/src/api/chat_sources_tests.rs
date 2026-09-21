//! Exercise citation publication through real tools, the chat producer, reattach,
//! and stored history. The final model turn waits until the snapshot is inspected.
use super::*;
use tokio::sync::Notify;

#[derive(Clone)]
struct SourceModel {
    calls: Arc<Vec<(&'static str, serde_json::Value)>>,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
    ready: Arc<Notify>,
    release: Arc<Notify>,
}

async fn model(
    State(m): State<SourceModel>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let index = {
        let mut seen = m.seen.lock().unwrap();
        let index = seen.len();
        seen.push(body);
        index
    };
    let stream = async_stream::stream! {
        let payload = if let Some((name, args)) = m.calls.get(index) {
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":format!("call_{index}"),"function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]})
        } else {
            m.ready.notify_one();
            m.release.notified().await;
            json!({"choices":[{"delta":{"content":"The answer is in the second section [2]."},"finish_reason":"stop"}]})
        };
        yield Ok::<_, Infallible>(Event::default().data(payload.to_string()));
        yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
    };
    Sse::new(stream)
}

fn frames(sse: &[u8], event: &str) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(sse)
        .split("\n\n")
        .filter_map(|frame| {
            if !frame.lines().any(|l| l == format!("event: {event}")) {
                return None;
            }
            frame
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .and_then(|s| serde_json::from_str(s).ok())
        })
        .collect()
}

async fn exercise(search_first: bool) -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let doc = utopia_store::documents::create(
        &f.pool,
        f.kb,
        "citation.md",
        "text/plain",
        30,
        "citation-test",
        None,
        None,
        None,
    )
    .await?;
    let pieces: Vec<_> = ["orchard introduction", "second section contains the answer"]
        .into_iter()
        .enumerate()
        .map(|(seq, text)| utopia_ingest::ChunkPiece {
            seq: seq as i32,
            text: text.into(),
            char_start: 0,
            char_end: text.len() as i32,
            heading: None,
            provenance: utopia_ingest::Provenance::stated(),
        })
        .collect();
    let chunks = utopia_store::documents::replace_chunks(&f.pool, f.kb, doc.id, &pieces).await?;
    f.state
        .search
        .reindex_document(&f.kb.to_string(), &doc.id.to_string(), &chunks)?;
    let mut calls = Vec::new();
    if search_first {
        calls.push(("search_chunks", json!({"query":"orchard"})));
    }
    calls.extend([
        ("get_document", json!({"document_id":doc.id})),
        ("get_document", json!({"document_id":doc.id})),
        ("find_entities", json!({"name":"no such entity"})),
    ]);
    let model = SourceModel {
        calls: Arc::new(calls),
        seen: Default::default(),
        ready: Default::default(),
        release: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = axum::Router::new()
        .route("/chat/completions", axum::routing::post(self::model))
        .with_state(model.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    sqlx::query("UPDATE llm_settings SET chat_base_url=$1 WHERE workspace_id=(SELECT workspace_id FROM knowledge_bases WHERE id=$2)").bind(base).bind(f.kb).execute(&f.pool).await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let response = chat(
            State(f.state.clone()),
            AuthUser(f.user.clone()),
            Path(f.kb),
            Json(ChatReq {
                conversation_id: None,
                message: "Read the second section".into(),
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("chat refused"))?;
        model.ready.notified().await;
        let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id=$1")
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
        let snapshot = f.state.live.attach(id).await.unwrap().0;
        let attached = reattach(
            State(f.state.clone()),
            AuthUser(f.user.clone()),
            Path((f.kb, id)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("reattach refused"))?;
        model.release.notify_one();
        let live = axum::body::to_bytes(response.into_response().into_body(), 65536).await?;
        let replay = axum::body::to_bytes(attached.into_response().into_body(), 65536).await?;
        let Json(history) = conversation_detail(
            State(f.state.clone()),
            AuthUser(f.user.clone()),
            Path((f.kb, id)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("history refused"))?;
        let stored = &history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "assistant")
            .unwrap()["sources"];
        anyhow::ensure!(
            snapshot.sources.len() == 2,
            "document sources missing before final answer: {:?}",
            snapshot.sources
        );
        let events = frames(&live, "sources");
        anyhow::ensure!(
            events.len() == if search_first { 2 } else { 1 },
            "only source changes should publish: {events:?}"
        );
        anyhow::ensure!(events.last() == Some(stored));
        anyhow::ensure!(json!(snapshot.sources) == *stored);
        anyhow::ensure!(frames(&replay, "snapshot")[0]["sources"] == *stored);
        for (index, (chunk, _)) in chunks.iter().enumerate() {
            anyhow::ensure!(stored[index]["n"] == index + 1);
            anyhow::ensure!(stored[index]["chunk_id"] == *chunk);
            anyhow::ensure!(stored[index]["document_id"] == doc.id.to_string());
        }
        let seen = model.seen.lock().unwrap();
        let final_messages = seen.last().unwrap()["messages"].to_string();
        anyhow::ensure!(
            final_messages.contains("[2]")
                && final_messages.contains("second section contains the answer")
        );
        anyhow::ensure!(seen.len() == model.calls.len() + 1);
        Ok::<_, anyhow::Error>(())
    })
    .await;
    model.release.notify_one();
    server.abort();
    let _ = server.await;
    f.cleanup().await?;
    result??;
    Ok(())
}

#[tokio::test]
async fn document_reads_publish_new_sources_after_search() -> anyhow::Result<()> {
    exercise(true).await
}
#[tokio::test]
async fn direct_document_reads_publish_sources_without_a_search_step() -> anyhow::Result<()> {
    exercise(false).await
}
