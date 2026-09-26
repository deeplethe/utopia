//! 模型把工具调用写成了正文（#937）。
//!
//! 端点偶尔不走协议里的 `tool_calls`，而是把调用的标记原样写进 `delta.content`：
//! DeepSeek 的 `<｜tool▁calls▁begin｜>…`，或者 DSML。什么都没执行，那段也不是答案。
//! 从前只有预算用完后的收尾那一轮会查它；普通回合里它照常流给用户、存成答案、
//! 以 `done` 收尾。这里钉的是现在的样子：
//!
//! 1. 标记不外发：流里、存下的答案里、回放给下一轮的工具往返里都没有；
//! 2. 只有文字的那一回合退回一次（`MARKUP_RETRY`），模型接着调工具或作答；
//! 3. 再写一次就以 error 收尾，什么都不存；
//! 4. 叙述照常流出，只扣可能是标记的那几行。只是以 `<` 开头的正文、围栏里的示例，
//!    分辨出来之后原样放行，也不会被退回。
use super::*;

/// DeepSeek 的原生写法：一次工具调用被原样写成了正文
const CALL: &str = "<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>search_chunks\n\
                    ```json\n{\"query\":\"Acme revenue\"}\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>";
const ANSWER: &str = "Acme's revenue rose 5% last quarter.";
const TOOL: Reply = Reply::Tool("find_entities", r#"{"name":"Acme"}"#);
const QUESTION: &str = "What was Acme's revenue last quarter?";

/// 流给用户的全部正文，按帧拼起来
fn streamed(sse: &str) -> String {
    sse.split("\n\n")
        .filter_map(|frame| frame.strip_prefix("event: delta\ndata: "))
        .map(|data| {
            serde_json::from_str::<serde_json::Value>(data).expect("delta is JSON")["text"]
                .as_str()
                .expect("delta has text")
                .to_string()
        })
        .collect()
}

/// 有几次请求以退回的那句话结尾
fn sent_back(f: &Fx) -> usize {
    f.requests()
        .iter()
        .filter(|r| last_message(r) == agent::MARKUP_RETRY)
        .count()
}

/// 存下的这一轮工具往返，下一轮回放给模型的就是它
async fn stored_exchange(f: &Fx) -> anyhow::Result<serde_json::Value> {
    Ok(sqlx::query_scalar(
        "SELECT m.tool_exchange FROM conversation_messages m
           JOIN conversations c ON c.id = m.conversation_id
          WHERE c.kb_id = $1 AND m.role = 'assistant'",
    )
    .bind(f.kb)
    .fetch_one(&f.pool)
    .await?)
}

#[tokio::test]
async fn a_call_written_as_text_after_a_tool_is_sent_back_once() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        TOOL,
        Reply::Text(CALL),
        Reply::Text(ANSWER),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask(QUESTION).await?;

    assert!(sse.contains("event: done"), "{sse}");
    assert!(!sse.contains("event: error"), "{sse}");
    assert_eq!(
        streamed(&sse),
        ANSWER,
        "the markup never reaches the reader"
    );
    assert_eq!(f.stored_answer().await?.as_deref(), Some(ANSWER));
    let requests = f.requests();
    assert_eq!(requests.len(), 3, "tool, markup, then one retry");
    assert_eq!(last_message(&requests[2]), agent::MARKUP_RETRY);
    assert_eq!(sent_back(&f), 1);
    // 退回时保留那一回合：模型看得见自己写了什么
    let messages = requests[2]["messages"].as_array().expect("messages");
    assert_eq!(messages[messages.len() - 2]["role"], "assistant");
    assert_eq!(messages[messages.len() - 2]["content"], CALL);
    f.cleanup().await
}

#[tokio::test]
async fn a_call_written_as_text_in_the_first_turn_is_left_out_of_the_answer() -> anyhow::Result<()>
{
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Text(CALL),
        TOOL,
        Reply::Text(ANSWER),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask(QUESTION).await?;

    assert!(sse.contains("event: done"), "{sse}");
    assert!(
        sse.contains("event: step"),
        "the retry made the call for real:\n{sse}"
    );
    assert_eq!(streamed(&sse), ANSWER);
    assert_eq!(f.stored_answer().await?.as_deref(), Some(ANSWER));
    let requests = f.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        last_message(&requests[1]),
        agent::MARKUP_RETRY,
        "sent back as markup, not as a turn that skipped the tools"
    );
    assert_eq!(sent_back(&f), 1);
    f.cleanup().await
}

#[tokio::test]
async fn a_call_written_as_text_twice_is_an_error_and_nothing_is_stored() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        TOOL,
        Reply::Text(CALL),
        Reply::Text(CALL),
        Reply::Text("Never requested"),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask(QUESTION).await?;

    assert!(sse.contains("event: error"), "{sse}");
    assert!(!sse.contains("event: done"), "{sse}");
    assert!(sse.contains(agent::CONTROL_TEXT), "{sse}");
    assert_eq!(streamed(&sse), "", "neither turn reached the reader");
    assert!(f.stored_answer().await?.is_none());
    assert_eq!(f.requests().len(), 3, "one retry, not a loop");
    assert_eq!(sent_back(&f), 1);
    f.cleanup().await
}

#[tokio::test]
async fn narration_streams_while_a_split_marker_is_held() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        TOOL,
        Reply::SplitText(&[
            "I will search the filings.\n",
            "<｜tool",
            "▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>search_chunks\n",
            "```json\n{\"query\":\"Acme filings\"}\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>",
        ]),
        Reply::Text(ANSWER),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask(QUESTION).await?;

    assert!(sse.contains("event: done"), "{sse}");
    let shown = streamed(&sse);
    assert!(
        shown.starts_with("I will search the filings.\n"),
        "{shown:?}"
    );
    assert!(shown.ends_with(ANSWER), "{shown:?}");
    assert!(
        !shown.contains("<｜tool") && !shown.contains("```json"),
        "no part of the split marker went out: {shown:?}"
    );
    assert_eq!(f.stored_answer().await?.as_deref(), Some(shown.as_str()));
    assert_eq!(sent_back(&f), 1);
    f.cleanup().await
}

#[tokio::test]
async fn markup_beside_a_real_call_is_dropped_and_not_replayed() -> anyhow::Result<()> {
    const SAID: &str = "Checking the ledger.\n<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function\
                        <｜tool▁sep｜>find_entities\n```json\n{\"name\":\"Acme\"}\n```\
                        <｜tool▁call▁end｜><｜tool▁calls▁end｜>";
    let Some(f) = fixture(Scripted::new(vec![
        Reply::TextWithTool(SAID),
        Reply::Text(ANSWER),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask(QUESTION).await?;

    assert!(sse.contains("event: done"), "{sse}");
    let shown = streamed(&sse);
    assert!(shown.starts_with("Checking the ledger.\n"), "{shown:?}");
    assert!(shown.ends_with(ANSWER), "{shown:?}");
    assert!(!shown.contains("tool▁"), "{shown:?}");
    assert_eq!(f.stored_answer().await?.as_deref(), Some(shown.as_str()));
    assert_eq!(
        f.requests().len(),
        2,
        "the real call ran; nothing is sent back"
    );
    assert_eq!(sent_back(&f), 0);
    // 这一步落在叙述之后、答案之前，不在扣掉的标记之后
    let step: serde_json::Value = sse
        .split("\n\n")
        .find_map(|frame| frame.strip_prefix("event: step\ndata: "))
        .map(serde_json::from_str)
        .expect("a step")?;
    let before_answer = shown.len() - ANSWER.len();
    assert_eq!(step["at"], serde_json::json!(before_answer));
    let exchange = stored_exchange(&f).await?;
    assert_eq!(exchange[0]["content"], "Checking the ledger.\n");
    assert!(!exchange.to_string().contains("tool▁"), "{exchange}");
    f.cleanup().await
}

#[tokio::test]
async fn text_that_only_starts_like_a_marker_goes_out_whole_and_once() -> anyhow::Result<()> {
    const TEXT: &str = "<b>Acme</b> grew 5% [1].\n<DSML> is the name of that markup, not a call.";
    let Some(f) = fixture(Scripted::new(vec![
        TOOL,
        Reply::SplitText(&[
            "<",
            "b>Acme</b> grew 5% [1].",
            "\n<DS",
            "ML> is the name of that markup, not a call.",
        ]),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask(QUESTION).await?;

    assert!(sse.contains("event: done"), "{sse}");
    assert_eq!(streamed(&sse), TEXT);
    assert_eq!(f.stored_answer().await?.as_deref(), Some(TEXT));
    assert_eq!(f.requests().len(), 2);
    assert_eq!(sent_back(&f), 0);
    f.cleanup().await
}

#[tokio::test]
async fn a_fenced_example_of_the_markup_is_released_when_the_turn_ends() -> anyhow::Result<()> {
    const EXAMPLE: &str = "It looks like this:\n~~~\n<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>\
                           function<｜tool▁sep｜>search_chunks\n~~~\nThat is the raw form of one call.";
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("no_evidence_needed", r#"{"reason":"about the format"}"#),
        Reply::Text(EXAMPLE),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f
        .ask("What does DeepSeek's raw tool-call format look like?")
        .await?;

    assert!(sse.contains("event: done"), "{sse}");
    assert_eq!(streamed(&sse), EXAMPLE);
    assert_eq!(f.stored_answer().await?.as_deref(), Some(EXAMPLE));
    assert_eq!(f.requests().len(), 2);
    assert_eq!(sent_back(&f), 0);
    f.cleanup().await
}
