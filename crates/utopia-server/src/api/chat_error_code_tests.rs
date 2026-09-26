//! 对话流的失败带 code（0004）。
//!
//! 服务端不出显示文本：`error` 帧与请求被拒是同一个信封 `{error, code}`，界面拿 code
//! 去 `err.*` 表里查措辞，英文原句留给日志、MCP 与不做本地化的客户端。从前这一帧只有
//! 一句英文，于是中文界面上欠费、限流、空答案、存不下都是英文。
//!
//! 这里逐项核对用户会撞上的那几种失败各是哪个 code；每个 `error` 帧都带 code 这件事
//! 由终态契约（`chat_terminal_tests.rs`）对整张表检查。
use super::*;

const TOOL: Reply = Reply::Tool("find_entities", r#"{"name":"Acme"}"#);

/// 这条 SSE 里那个 `error` 帧的 code 与英文原句
fn failure(sse: &str) -> (String, String) {
    let data = sse
        .split("\n\n")
        .find_map(|frame| frame.strip_prefix("event: error\ndata: "))
        .unwrap_or_else(|| panic!("no error frame:\n{sse}"));
    let body: serde_json::Value = serde_json::from_str(data)
        .unwrap_or_else(|e| panic!("the error frame is not JSON ({e}): {data}"));
    (
        body["code"].as_str().unwrap_or_default().to_string(),
        body["error"].as_str().unwrap_or_default().to_string(),
    )
}

#[tokio::test]
async fn each_failure_a_user_can_meet_carries_its_code() -> anyhow::Result<()> {
    let mut budget = vec![Reply::NarratedTool; 6];
    budget.extend([Reply::Text(DSML), Reply::Text(DSML)]);
    for (what, replies, code) in [
        (
            "empty after a retry",
            vec![TOOL, Reply::Empty, Reply::Empty],
            "answer_empty",
        ),
        (
            "tool-control text at the budget",
            budget,
            "answer_tool_text",
        ),
        (
            "a tool call written as text twice (#946)",
            vec![TOOL, Reply::Text(DSML), Reply::Text(DSML)],
            "answer_tool_text",
        ),
        (
            "out of credit",
            vec![Reply::Http(402)],
            "model_out_of_credit",
        ),
        ("rate limited", vec![Reply::Http(429)], "model_rate_limited"),
        ("unavailable", vec![Reply::Http(503)], "model_unavailable"),
        ("key rejected", vec![Reply::Http(401)], "model_rejected"),
    ] {
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed at Acme?").await?;
        let (got, message) = failure(&sse);
        assert_eq!(got, code, "{what}: {message}");
        assert!(
            !message.is_empty(),
            "{what}: the English sentence stays for logs and MCP"
        );
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn an_answer_that_cannot_be_saved_says_so_with_its_code() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![TOOL, Reply::Text("Accepted answer.")])).await? else {
        return Ok(());
    };
    let name = format!("reject_assistant_{}", f.kb.simple());
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
         IF NEW.role='assistant' AND EXISTS(SELECT 1 FROM conversations \
         WHERE id=NEW.conversation_id AND kb_id='{}') THEN \
         RAISE EXCEPTION 'injected persistence failure'; END IF; RETURN NEW; END $$; \
         CREATE TRIGGER {name} BEFORE INSERT ON conversation_messages \
         FOR EACH ROW EXECUTE FUNCTION {name}();",
        f.kb
    ))
    .execute(&f.pool)
    .await?;
    let sse = f.ask("What happened?").await?;
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {name} ON conversation_messages; DROP FUNCTION {name}();"
    ))
    .execute(&f.pool)
    .await?;
    assert_eq!(failure(&sse).0, "answer_not_saved");
    f.cleanup().await
}

/// 收尾关口的理由以文本穿过 `chat_finalization`，所以按原文认。这里直接拿
/// `finalization_error` 的输出来对：改了措辞在这里失败，而不是悄悄变成「作答失败」
#[test]
fn the_finalization_reasons_keep_their_codes() {
    for (text, calls, code) in [
        ("", false, "answer_empty"),
        ("Answer.", true, "answer_tool_call"),
        (DSML, false, "answer_tool_text"),
    ] {
        let reason = agent::finalization_error(text, calls, "What changed?").expect("a reason");
        assert_eq!(unpublishable_code(reason), Some(code), "{reason}");
        // `chat_finalization` 修一次还不成时，理由包在两层话里交回来
        let wrapped = format!(
            "Model could not produce a final answer: Model could not produce a final answer \
             after one recovery: {reason}"
        );
        assert_eq!(unpublishable_code(&wrapped), Some(code), "{wrapped}");
    }
    assert_eq!(
        unpublishable_code("Model final answer exceeded the size limit"),
        Some("answer_too_long")
    );
    assert_eq!(unpublishable_code("Final answer timed out"), None);
}
