//! What the model is sent for one turn: the stored conversation, then the question once.
//!
//! The question is stored before the history is read, so the history already holds it, and
//! the runner sends the question again as its prompt. Each request the scripted endpoint
//! receives is checked for how often each question appears and what sits right before it.
use super::*;

/// How many user messages in one request say exactly `text`
fn user_turns(request: &serde_json::Value, text: &str) -> usize {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["role"] == "user" && m["content"] == text)
        .count()
}

fn last_user(request: &serde_json::Value) -> String {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|m| m["role"] == "user")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Ask in an existing conversation and collect the whole SSE body
pub(super) async fn ask_in(f: &Fx, conversation_id: Uuid, message: &str) -> anyhow::Result<String> {
    let sse = chat(
        State(f.state.clone()),
        AuthUser(f.user.clone()),
        Path(f.kb),
        Json(ChatReq {
            conversation_id: Some(conversation_id),
            message: message.into(),
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("chat handler refused the request"))?;
    let body = axum::body::to_bytes(sse.into_response().into_body(), 4 * 1024 * 1024).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn the_question_reaches_the_model_once() -> anyhow::Result<()> {
    let gate = Reply::Tool("no_evidence_needed", r#"{"reason":"greeting"}"#);
    let Some(f) = fixture(Scripted::new(vec![
        gate,
        Reply::Text("First answer."),
        gate,
        Reply::Text("Second answer."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let sse = f.ask("first question").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id = $1")
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
    let sse = ask_in(&f, id, "second question").await?;
    assert!(sse.contains("event: done"), "{sse}");

    let requests = f.requests();
    assert_eq!(requests.len(), 4, "a tool turn and an answer per question");
    for (i, request) in requests.iter().enumerate() {
        let current = if i < 2 {
            "first question"
        } else {
            "second question"
        };
        assert_eq!(
            user_turns(request, current),
            1,
            "request {i} must carry its question once:\n{request}"
        );
        assert_eq!(last_user(request), current, "request {i}:\n{request}");
        // The earlier question is replayed from the history, and only from there
        assert_eq!(
            user_turns(request, "first question"),
            1,
            "request {i}:\n{request}"
        );
    }
    f.cleanup().await
}

#[tokio::test]
async fn earlier_entities_sit_right_before_the_question() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("no_evidence_needed", r#"{"reason":"follow-up"}"#),
        Reply::Text("Answered."),
    ]))
    .await?
    else {
        return Ok(());
    };
    // An earlier turn resolved an entity; the next turn sends its id as a system message
    let id = utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "Acme").await?;
    utopia_store::conversations::append_message(
        &f.pool,
        id,
        "user",
        "Who founded Acme?",
        &utopia_store::conversations::TurnRecord::empty(),
    )
    .await?;
    utopia_store::conversations::append_message(
        &f.pool,
        id,
        "assistant",
        "Acme was founded by Ada.",
        &utopia_store::conversations::TurnRecord {
            steps: json!([]),
            sources: json!([]),
            resolved: json!([{ "id": Uuid::now_v7(), "name": "Acme", "type": "Organization" }]),
            tool_exchange: json!([]),
        },
    )
    .await?;
    let sse = ask_in(&f, id, "When was it founded?").await?;
    assert!(sse.contains("event: done"), "{sse}");

    let requests = f.requests();
    let first = &requests[0];
    let messages = first["messages"].as_array().expect("messages");
    let block = messages
        .iter()
        .position(|m| {
            m["role"] == "system"
                && m["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("Entities already identified"))
        })
        .expect("the entities resolved earlier are sent");
    assert_eq!(messages[block + 1]["role"], "user", "{first}");
    assert_eq!(
        messages[block + 1]["content"],
        "When was it founded?",
        "{first}"
    );
    assert_eq!(user_turns(first, "When was it founded?"), 1, "{first}");
    assert_eq!(user_turns(first, "Who founded Acme?"), 1, "{first}");
    f.cleanup().await
}
