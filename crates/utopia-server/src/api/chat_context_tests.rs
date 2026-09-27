//! Real handlers and storage, a scripted endpoint: no paid model or tokenizer.
use super::*;

const REFUSAL: &str = r#"{"error":{"code":"context_length_exceeded","message":"maximum context length is 4096 tokens; request has 12000 tokens"}}"#;
const GATE: Reply = Reply::Tool(
    "no_evidence_needed",
    r#"{"reason":"restating the conversation"}"#,
);

async fn seed(f: &Fx, count: usize, chars: usize) -> anyhow::Result<Uuid> {
    let id = utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "context test").await?;
    for i in 0..count {
        for (role, text) in [
            ("user", format!("question {i}")),
            ("assistant", format!("answer {i}: {}", "界".repeat(chars))),
        ] {
            utopia_store::conversations::append_message(
                &f.pool,
                id,
                role,
                &text,
                &utopia_store::conversations::TurnRecord::empty(),
            )
            .await?;
        }
    }
    Ok(id)
}

fn history_chars(request: &serde_json::Value, current: &str) -> usize {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system" && m["content"] != current)
        .filter_map(|m| m["content"].as_str())
        .map(|s| s.chars().count())
        .sum()
}

fn preserves_question(request: &serde_json::Value, current: &str) {
    let messages = request["messages"].as_array().unwrap();
    assert_eq!(
        messages
            .iter()
            .filter(|m| m["role"] == "user" && m["content"] == current)
            .count(),
        1
    );
    assert_eq!(messages[0]["role"], "system");
    let first = messages.iter().find(|m| m["role"] != "system").unwrap();
    assert_eq!(
        first["role"], "user",
        "no orphan answer at the history boundary"
    );
}

#[tokio::test]
async fn long_history_is_bounded_before_the_first_request() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![GATE, Reply::Text("Recovered.")])).await? else {
        return Ok(());
    };
    let id = seed(&f, 10, 6000).await?;
    let sse = history_tests::ask_in(&f, id, "current question").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let requests = f.requests();
    assert_eq!(requests.len(), 2);
    assert!(history_chars(&requests[0], "current question") <= 32_000);
    assert!(!requests[0].to_string().contains("question 0"));
    assert!(requests[0].to_string().contains("question 9"));
    preserves_question(&requests[0], "current question");
    // Trimming is request-local, not deletion from the conversation.
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conversation_messages WHERE conversation_id=$1")
            .bind(id)
            .fetch_one(&f.pool)
            .await?;
    assert_eq!(count, 22);
    f.cleanup().await
}

#[tokio::test]
async fn overflow_trims_once_and_the_next_turn_remembers_the_window() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::HttpError(400, REFUSAL),
        GATE,
        Reply::Text("Recovered."),
        GATE,
        Reply::Text("Next answer."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let id = seed(&f, 6, 3000).await?;
    let sse = history_tests::ask_in(&f, id, "current question").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let requests = f.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        history_chars(&requests[1], "current question")
            <= history_chars(&requests[0], "current question") / 2
    );
    assert_eq!(requests[0]["tools"], requests[1]["tools"]);
    assert_eq!(requests[0]["tool_choice"], requests[1]["tool_choice"]);
    assert_eq!(requests[0]["messages"][0], requests[1]["messages"][0]);
    for r in &requests {
        preserves_question(r, "current question");
    }
    let sse = history_tests::ask_in(&f, id, "next question").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let requests = f.requests();
    assert_eq!(requests.len(), 5);
    assert!(history_chars(&requests[3], "next question") <= 4096);
    preserves_question(&requests[3], "next question");
    // A changed model and another workspace cannot inherit this refusal.
    let ws: Uuid = sqlx::query_scalar("SELECT workspace_id FROM knowledge_bases WHERE id=$1")
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
    let mut settings = utopia_store::settings::get(&f.pool, ws).await?.unwrap();
    assert_eq!(
        f.state
            .chat_clients
            .get(ws, &settings)
            .unwrap()
            .history_char_budget(),
        4096
    );
    assert_eq!(
        f.state
            .chat_clients
            .get(Uuid::now_v7(), &settings)
            .unwrap()
            .history_char_budget(),
        32_000
    );
    settings.chat_model = Some("changed-model".into());
    assert_eq!(
        f.state
            .chat_clients
            .get(ws, &settings)
            .unwrap()
            .history_char_budget(),
        32_000
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_second_refusal_has_its_own_code_and_never_falls_back() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::HttpError(400, REFUSAL),
        Reply::HttpError(422, REFUSAL),
    ]))
    .await?
    else {
        return Ok(());
    };
    let id = seed(&f, 4, 2000).await?;
    let sse = history_tests::ask_in(&f, id, "current question").await?;
    assert!(sse.contains("event: error"), "{sse}");
    assert!(sse.contains("context_too_long"), "{sse}");
    assert!(!sse.contains("event: done"));
    let requests = f.requests();
    assert_eq!(requests.len(), 2);
    for r in requests {
        assert!(r.get("tools").is_some());
        preserves_question(&r, "current question");
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conversation_messages WHERE conversation_id=$1 AND role='assistant'",
    )
    .bind(id)
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(count, 4, "no failed answer is stored as success");
    f.cleanup().await
}

#[tokio::test]
async fn overflow_after_a_tool_keeps_its_result_without_executing_it_again() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        GATE,
        Reply::HttpError(400, REFUSAL),
        Reply::Text("Recovered after the tool."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let id = seed(&f, 4, 2000).await?;
    let sse = history_tests::ask_in(&f, id, "current question").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let requests = f.requests();
    assert_eq!(requests.len(), 3);
    let tools = |r: &serde_json::Value| {
        r["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool" || m.get("tool_calls").is_some())
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(tools(&requests[1]), tools(&requests[2]));
    assert_eq!(tools(&requests[2]).len(), 2);
    preserves_question(&requests[2], "current question");
    f.cleanup().await
}

#[tokio::test]
async fn rag_uses_the_same_bound_and_can_recover_without_changing_its_system() -> anyhow::Result<()>
{
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Http(400),
        Reply::Http(400),
        Reply::HttpError(400, REFUSAL),
        Reply::Text("Fallback recovered."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let id = seed(&f, 10, 6000).await?;
    let sse = history_tests::ask_in(&f, id, "current question").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let requests = f.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].get("tools").is_none());
    assert!(history_chars(&requests[2], "current question") <= 32_000);
    assert!(history_chars(&requests[3], "current question") <= 4096);
    assert_eq!(requests[2]["messages"][0], requests[3]["messages"][0]);
    for r in &requests {
        preserves_question(r, "current question");
    }
    f.cleanup().await
}

#[tokio::test]
async fn even_an_oversized_current_question_is_never_cut() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::HttpError(400, REFUSAL),
        Reply::HttpError(400, REFUSAL),
    ]))
    .await?
    else {
        return Ok(());
    };
    let question = "新问题😀".repeat(10_000);
    let sse = f.ask(&question).await?;
    assert!(sse.contains("context_too_long"), "{sse}");
    let requests = f.requests();
    assert_eq!(requests.len(), 2);
    for r in requests {
        preserves_question(&r, &question);
    }
    f.cleanup().await
}
