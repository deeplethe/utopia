//! Two real chat producers share a conversation while a local upstream gates their
//! answers independently. No scheduler sleeps and no production-only test hooks.
use super::*;
use tokio::sync::{broadcast, Notify};

#[derive(Clone, Default)]
struct Gates {
    ready: [Arc<Notify>; 2],
    release: [Arc<Notify>; 2],
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
}

async fn upstream(
    State(gates): State<Gates>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let messages = body["messages"].as_array().expect("messages");
    let question = messages.iter().rev().find(|m| m["role"] == "user").unwrap();
    let index = usize::from(question["content"].to_string().contains("second greeting"));
    let tool_ran = messages.iter().any(|m| m["role"] == "tool");
    gates.requests.lock().unwrap().push(body);
    let stream = async_stream::stream! {
        let payload = if tool_ran {
            gates.ready[index].notify_one();
            gates.release[index].notified().await;
            json!({"choices":[{"delta":{"content": if index == 0 {"First answer."} else {"Second answer."}}, "finish_reason":"stop"}]})
        } else {
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":format!("greeting_{index}"),"function":{"name":"no_evidence_needed","arguments":"{\"reason\":\"greeting\"}"}}]},"finish_reason":"tool_calls"}]})
        };
        yield Ok::<_, Infallible>(Event::default().data(payload.to_string()));
        yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
    };
    Sse::new(stream)
}

#[tokio::test]
async fn finishing_old_chat_preserves_reattachment_to_new_chat() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let gates = Gates::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = axum::Router::new()
        .route("/chat/completions", axum::routing::post(upstream))
        .with_state(gates.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    sqlx::query("UPDATE llm_settings SET chat_base_url=$1 WHERE workspace_id=(SELECT workspace_id FROM knowledge_bases WHERE id=$2)")
        .bind(base).bind(f.kb).execute(&f.pool).await?;

    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let first = chat(
            State(f.state.clone()), AuthUser(f.user.clone()), Path(f.kb),
            Json(ChatReq { conversation_id: None, message: "first greeting".into(), retry_message_id: None }),
        ).await.map_err(|_| anyhow::anyhow!("first chat refused"))?;
        gates.ready[0].notified().await;
        let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id=$1")
            .bind(f.kb).fetch_one(&f.pool).await?;
        let (_, mut old_events) = f.state.live.attach(id).await.unwrap();
        let second = chat(
            State(f.state.clone()), AuthUser(f.user.clone()), Path(f.kb),
            Json(ChatReq { conversation_id: Some(id), message: "second greeting".into(), retry_message_id: None }),
        ).await.map_err(|_| anyhow::anyhow!("second chat refused"))?;
        gates.ready[1].notified().await;
        gates.release[0].notify_one();
        let first_body = axum::body::to_bytes(first.into_response().into_body(), 65536).await?;
        anyhow::ensure!(String::from_utf8_lossy(&first_body).contains("First answer."));
        // The old sender closing proves its producer has actually called finish;
        // merely observing done would still leave a scheduling window before it.
        while !matches!(old_events.recv().await, Err(broadcast::error::RecvError::Closed)) {}
        let attached = reattach(
            State(f.state.clone()), AuthUser(f.user.clone()), Path((f.kb, id)),
        ).await.map_err(|_| anyhow::anyhow!("reattach refused"))?;
        gates.release[1].notify_one();
        let attached_body = axum::body::to_bytes(attached.into_response().into_body(), 65536).await?;
        let attached_text = String::from_utf8_lossy(&attached_body);
        let second_body = axum::body::to_bytes(second.into_response().into_body(), 65536).await?;
        let second_text = String::from_utf8_lossy(&second_body);
        anyhow::ensure!(second_text.contains("Second answer."), "{second_text}");
        anyhow::ensure!(attached_text.contains("event: snapshot"), "new answer must remain attachable: {attached_text}");
        anyhow::ensure!(attached_text.contains("Second answer."), "{attached_text}");
        anyhow::ensure!(attached_text.contains("event: done"), "{attached_text}");
        anyhow::ensure!(!attached_text.contains("First answer."));
        anyhow::ensure!(!attached_text.contains("event: idle"));
        anyhow::ensure!(gates.requests.lock().unwrap().len() == 4, "one tool turn and one answer per generation");
        let answers: Vec<String> = sqlx::query_scalar("SELECT content FROM conversation_messages WHERE conversation_id=$1 AND role='assistant' ORDER BY created_at")
            .bind(id).fetch_all(&f.pool).await?;
        anyhow::ensure!(answers == ["First answer.", "Second answer."], "both answers must persist: {answers:?}");
        Ok::<_, anyhow::Error>(())
    }).await;
    gates.release[0].notify_one();
    gates.release[1].notify_one();
    server.abort();
    let _ = server.await;
    f.cleanup().await?;
    result??;
    Ok(())
}
