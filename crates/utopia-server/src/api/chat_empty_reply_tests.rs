//! 空回复重问一次（`agent::EMPTY_REPLY_RETRY`，由 `Policy::on_model_turn_finished` 发出）。
//!
//! 走整条对话：直接调 `chat` 处理函数，把它返回的 SSE 收完读帧。模型由 wiremock 按脚本
//! 假扮成 OpenAI 兼容的 `/chat/completions`——第几次请求回什么事先写好，于是「模型回空」
//! 这件真模型上几十轮才碰一次的事，在这里每次都发生。
//!
//! 1. **查完之后空一次，重问后答出来。** 这是台子上碰到的真实形状：工具先跑完，模型接着
//!    回空。用户看到的是查证步骤接着答案，不是报错；重问只发了一次，它的末尾正是那句
//!    重问；落库的助手消息就是重问后的答案。
//!    （先调一个工具，与真实失败一致：首轮一步不查就答，会被 `required` 的闸门退回，
//!    那是另一件事。）
//! 2. **一直空，报错，而且只重问一次。** 两次请求之后是 `error` 帧——不是第三次、第四次：
//!    一个始终不说话的端点不能让循环空转。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use super::agent::EMPTY_REPLY_RETRY;
use super::*;
use axum::response::IntoResponse;
use std::sync::{Arc, Mutex};
use utopia_core::models::User;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, Request, Respond, ResponseTemplate,
};

/// 假模型的一次回复
#[derive(Clone, Copy)]
pub(super) enum Reply {
    /// 没有正文，也不调工具
    Empty,
    Document(Uuid),
    Text(&'static str),
    SplitText(&'static [&'static str]),
    NarratedTool,
    ParallelTools,
    OversizedText,
    Finished(&'static str, &'static str),
    Http(u16),
    /// 调一个工具：(名字, 参数 JSON)
    Tool(&'static str, &'static str),
}

/// 按脚本回话的假模型。`replies[i]` 是第 i+1 次请求的回复，脚本读完之后一律回空。
/// 每次请求的正文都记下来，好查重问那一次问了什么
#[derive(Clone)]
pub(super) struct Scripted {
    replies: Arc<Vec<Reply>>,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Scripted {
    pub(super) fn new(replies: Vec<Reply>) -> Self {
        Self {
            replies: Arc::new(replies),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn requests(&self) -> Vec<serde_json::Value> {
        self.seen.lock().expect("seen lock").clone()
    }
}

impl Respond for Scripted {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = request.body_json().expect("chat request is JSON");
        let n = {
            let mut seen = self.seen.lock().expect("seen lock");
            seen.push(body);
            seen.len()
        };
        let frame = match self.replies.get(n - 1).copied().unwrap_or(Reply::Empty) {
            Reply::Http(status) => {
                return ResponseTemplate::new(status).set_body_string("Evidence gathering complete")
            }
            Reply::Finished(text, reason) => Some(
                serde_json::json!({ "choices": [{ "delta": { "content": text }, "finish_reason": reason }] }),
            ),
            Reply::OversizedText => {
                let text = "x".repeat(4096);
                let frame = serde_json::json!({ "choices": [{ "delta": { "content": text } }] });
                let sse = format!("data: {frame}\n\n").repeat(257) + "data: [DONE]\n\n";
                return ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse);
            }
            Reply::SplitText(parts) => {
                let mut sse = String::new();
                for text in parts {
                    let frame =
                        serde_json::json!({ "choices": [{ "delta": { "content": text } }] });
                    sse.push_str(&format!("data: {frame}\n\n"));
                }
                sse.push_str("data: [DONE]\n\n");
                return ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse);
            }
            Reply::ParallelTools => Some(serde_json::json!({"choices":[{"delta":{
                "content":"核查😀",
                "tool_calls":[
                    {"index":0,"id":format!("call_{n}_a"),"function":{"name":"find_entities","arguments":"{\"name\":\"Acme\"}"}},
                    {"index":1,"id":format!("call_{n}_b"),"function":{"name":"find_entities","arguments":"{\"name\":\"Other\"}"}}
                ]
            }}]})),
            Reply::NarratedTool => Some(serde_json::json!({ "choices": [{ "delta": {
                "content": "I will check the evidence.",
                "tool_calls": [{ "index": 0, "id": format!("call_{n}"),
                    "function": { "name": "find_entities", "arguments": "{\"name\":\"Acme\"}" }
                }]
            } }] })),
            Reply::Document(id) => Some(serde_json::json!({ "choices": [{ "delta": {
                "tool_calls": [{ "index": 0, "id": format!("call_{n}"),
                    "function": { "name": "get_document", "arguments":
                        serde_json::json!({"document_id": id}).to_string() }
                }]
            } }] })),
            Reply::Empty => None,
            Reply::Text(text) => {
                Some(serde_json::json!({ "choices": [{ "delta": { "content": text } }] }))
            }
            Reply::Tool(name, args) => Some(serde_json::json!({ "choices": [{ "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": format!("call_{n}"),
                    "function": { "name": name, "arguments": args }
                }]
            } }] })),
        };
        let mut sse = String::new();
        if let Some(frame) = frame {
            sse.push_str(&format!("data: {frame}\n\n"));
        }
        // 空回复就是只有这一帧：没有正文增量，也没有工具调用
        sse.push_str("data: [DONE]\n\n");
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(sse)
    }
}

pub(super) struct Fx {
    state: AppState,
    pool: sqlx::PgPool,
    org: Uuid,
    kb: Uuid,
    user: User,
    fake: Scripted,
    _server: MockServer,
    dir: std::path::PathBuf,
}

pub(super) async fn fixture(fake: Scripted) -> anyhow::Result<Option<Fx>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, uid) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'empty-reply-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'empty-reply-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'empty-reply-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO users(id,org_id,email,password_hash,display_name,is_admin)
         VALUES($1,$2,$3,'','Empty Reply',TRUE)",
    )
    .bind(uid)
    .bind(org)
    .bind(format!("empty-reply-{uid}@test.local"))
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO kb_members(kb_id,user_id,role) VALUES($1,$2,'viewer')")
        .bind(kb)
        .bind(uid)
        .execute(&pool)
        .await?;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(fake.clone())
        .mount(&server)
        .await;
    utopia_store::settings::upsert(
        &pool,
        ws,
        Some(&server.uri()),
        None,
        Some("scripted-chat"),
        None,
        None,
        None,
        None,
    )
    .await?;

    let dir = std::env::temp_dir().join(format!("utopia-empty-reply-{kb}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
        .bind(uid)
        .fetch_one(&pool)
        .await?;
    Ok(Some(Fx {
        state,
        pool,
        org,
        kb,
        user,
        fake,
        _server: server,
        dir,
    }))
}

impl Fx {
    /// 问一句，把整条 SSE 收成文本
    pub(super) async fn ask(&self, message: &str) -> anyhow::Result<String> {
        let sse = chat(
            State(self.state.clone()),
            AuthUser(self.user.clone()),
            Path(self.kb),
            Json(ChatReq {
                conversation_id: None,
                message: message.into(),
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("chat handler refused the request"))?;
        let body = axum::body::to_bytes(sse.into_response().into_body(), 4 * 1024 * 1024).await?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    async fn stored_answer(&self) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT m.content FROM conversation_messages m
               JOIN conversations c ON c.id = m.conversation_id
              WHERE c.kb_id = $1 AND m.role = 'assistant'
              ORDER BY m.created_at DESC LIMIT 1",
        )
        .bind(self.kb)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub(super) async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

/// 发给模型的请求里，最后一条消息的正文
fn last_message(request: &serde_json::Value) -> String {
    request["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn an_empty_reply_after_the_tools_is_asked_again_and_the_answer_arrives() -> anyhow::Result<()>
{
    const ANSWER: &str = "Answered on the second ask.";
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("find_entities", r#"{"name":"Acme"}"#),
        Reply::Empty,
        Reply::Text(ANSWER),
    ]))
    .await?
    else {
        return Ok(());
    };

    let sse = f.ask("What changed at Acme last quarter?").await?;

    assert!(sse.contains("event: step"), "the tool ran first:\n{sse}");
    assert!(
        !sse.contains("event: error"),
        "an empty reply must not reach the user as an error:\n{sse}"
    );
    assert!(sse.contains("event: done"), "the turn finishes:\n{sse}");
    assert!(
        sse.contains(ANSWER),
        "the reply after the retry is streamed:\n{sse}"
    );

    let requests = f.fake.requests();
    assert_eq!(requests.len(), 3, "tool, empty, then one retry");
    assert_eq!(
        requests
            .iter()
            .filter(|r| last_message(r) == EMPTY_REPLY_RETRY)
            .count(),
        1,
        "the retry is sent exactly once"
    );
    assert_eq!(
        last_message(&requests[2]),
        EMPTY_REPLY_RETRY,
        "the request after the empty reply ends with the retry"
    );
    assert_eq!(
        f.stored_answer().await?.as_deref(),
        Some(ANSWER),
        "the stored answer is the one the retry produced"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_reply_that_stays_empty_is_an_error_after_one_retry() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![Reply::Empty; 4])).await? else {
        return Ok(());
    };

    let sse = f.ask("What changed last quarter?").await?;

    assert!(
        sse.contains("event: error"),
        "still empty is an error:\n{sse}"
    );
    assert!(
        sse.contains("Model returned an empty answer"),
        "with the same message as before:\n{sse}"
    );
    assert_eq!(
        f.fake.requests().len(),
        2,
        "one retry, not a loop: a silent endpoint must not be asked again and again"
    );
    f.cleanup().await
}

#[path = "chat_fallback_tests.rs"]
mod fallback_tests;
#[path = "chat_persistence_tests.rs"]
mod persistence_tests;
#[path = "chat_registry_tests.rs"]
mod registry_tests;
#[path = "chat_sources_tests.rs"]
mod sources_tests;

// These are synthetic upstream responses, not a replay of the reported model incident.
const DSML: &str = "<｜DSML｜ calls>\n<｜DSML｜ invoke name=\"entity_facts\">{}</｜DSML｜ invoke>\n</｜DSML｜ calls>";

async fn budget_case(last: Reply, question: &str, answer: Option<&str>) -> anyhow::Result<()> {
    let mut replies = vec![Reply::NarratedTool; 6];
    replies.push(last);
    let Some(f) = fixture(Scripted::new(replies)).await? else {
        return Ok(());
    };
    let sse = f.ask(question).await?;
    assert_eq!(
        f.fake.requests().len(),
        if answer.is_none() && !matches!(last, Reply::OversizedText) {
            8
        } else {
            7
        },
        "six tool rounds, one final call, and at most one answer-only recovery"
    );
    assert_eq!(
        sse.matches("event: step").count(),
        6,
        "no budget-overrun tool execution: {sse}"
    );
    let requests = f.fake.requests();
    assert!(requests[..6].iter().all(|r| r.get("tools").is_some()));
    assert!(requests[6].get("tools").is_none());
    assert!(requests[6].get("tool_choice").is_none());
    assert!(requests[6]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["role"] != "tool" && m.get("tool_calls").is_none()));
    assert!(requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with(SYSTEM_PROMPT));
    assert!(!requests[6]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("ALWAYS gather"));
    let data: serde_json::Value =
        serde_json::from_str(requests[6]["messages"][1]["content"].as_str().unwrap())?;
    assert_eq!(data["evidence"].as_array().unwrap().len(), 6);
    assert!(data["conversation_context"]["turns"]
        .as_array()
        .unwrap()
        .is_empty());
    match answer {
        Some(answer) => {
            assert!(sse.contains("event: done"), "{sse}");
            assert!(!sse.contains("event: error"), "{sse}");
            let expected = "I will check the evidence.\n\n".repeat(6) + answer;
            assert_eq!(f.stored_answer().await?.as_deref(), Some(expected.as_str()));
            let streamed: String = sse
                .split("\n\n")
                .filter(|frame| frame.starts_with("event: delta\n"))
                .map(|frame| {
                    let data = frame.strip_prefix("event: delta\ndata: ").unwrap();
                    serde_json::from_str::<serde_json::Value>(data).unwrap()["text"]
                        .as_str()
                        .unwrap()
                        .to_string()
                })
                .collect();
            assert_eq!(streamed, expected, "final text is published exactly once");
        }
        None => {
            assert!(sse.contains("event: error"), "{sse}");
            assert!(!sse.contains("event: done"), "{sse}");
            assert!(
                !sse.contains("DSML"),
                "protocol text must not escape in deltas: {sse}"
            );
            assert!(f.stored_answer().await?.is_none());
        }
    }
    f.cleanup().await
}

#[tokio::test]
async fn budget_finalization_rejects_protocol_text_despite_earlier_narration() -> anyhow::Result<()>
{
    budget_case(Reply::Text(DSML), "What changed at Acme?", None).await
}

#[tokio::test]
async fn budget_finalization_rejects_split_protocol_variants() -> anyhow::Result<()> {
    for parts in [
        &[
            "<｜DS",
            "ML｜tool_calls>",
            "<｜DSML｜invoke name=\"entity_facts\">{}",
        ] as &[&str],
        &[
            "<｜｜",
            "DSML",
            "｜｜ calls>",
            "<｜｜DSML｜｜ invoke name=\"entity_facts\">{}",
        ],
        &[
            "<|DS",
            "ML|calls>",
            "<|DSML|invoke name=\"entity_facts\">{}",
        ],
    ] {
        budget_case(Reply::SplitText(parts), "What changed at Acme?", None).await?;
    }
    Ok(())
}

#[tokio::test]
async fn budget_finalization_refuses_structured_calls() -> anyhow::Result<()> {
    budget_case(
        Reply::Tool("find_entities", r#"{"name":"Over budget"}"#),
        "What changed at Acme?",
        None,
    )
    .await
}

#[tokio::test]
async fn budget_finalization_refuses_blank_terminal_text() -> anyhow::Result<()> {
    budget_case(Reply::Text(" \n\t"), "What changed at Acme?", None).await
}

#[tokio::test]
async fn budget_finalization_accepts_an_answer_and_protocol_explanations() -> anyhow::Result<()> {
    for answer in [
        "No matching evidence was found.",
        "DSML is a tool-call encoding. For example: <｜DSML｜ calls>...",
        "```xml\n<｜DSML｜ calls>...\n```\nThis is a tool call encoding.",
    ] {
        budget_case(
            Reply::Text(answer),
            "Explain the tool protocol",
            Some(answer),
        )
        .await?;
    }
    budget_case(
        Reply::Text(DSML),
        "Return a DSML example verbatim.",
        Some(DSML),
    )
    .await
}

#[tokio::test]
async fn budget_finalization_bounds_unpublished_text() -> anyhow::Result<()> {
    budget_case(Reply::OversizedText, "What changed at Acme?", None).await
}

#[tokio::test]
async fn budget_finalization_survives_disconnect_and_reattach() -> anyhow::Result<()> {
    for last in [Reply::Text("The final answer."), Reply::Text(DSML)] {
        let mut replies = vec![Reply::NarratedTool; 6];
        replies.push(last);
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let id = utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "question").await?;
        let response = chat(
            State(f.state.clone()),
            AuthUser(f.user.clone()),
            Path(f.kb),
            Json(ChatReq {
                conversation_id: Some(id),
                message: "What changed at Acme?".into(),
            }),
        )
        .await
        .map_err(|_| anyhow::anyhow!("chat handler refused the request"))?;
        // Drop the original HTTP consumer before consuming any SSE bytes.
        drop(response);
        let (snapshot, mut rx) = f
            .state
            .live
            .attach(id)
            .await
            .expect("background producer is running");
        assert!(!snapshot.content.contains("DSML"));
        let mut events = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while let Ok(frame) = rx.recv().await {
                assert!(!frame.data.contains("DSML"));
                events.push(frame.event);
            }
        })
        .await?;
        assert_eq!(
            f.fake.requests().len(),
            if matches!(last, Reply::Text(DSML)) {
                8
            } else {
                7
            }
        );
        if matches!(last, Reply::Text(DSML)) {
            assert!(events.contains(&"error"));
            assert!(!events.contains(&"done"));
            assert!(f.stored_answer().await?.is_none());
        } else {
            assert!(events.contains(&"done"));
            assert!(!events.contains(&"error"));
            assert!(f
                .stored_answer()
                .await?
                .unwrap()
                .ends_with("The final answer."));
        }
        assert!(f.state.live.attach(id).await.is_none());
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn early_retries_do_not_extend_the_tool_budget() -> anyhow::Result<()> {
    for early in [Reply::Empty, Reply::Text("I will look into it.")] {
        let mut replies = vec![early];
        replies.extend(vec![Reply::NarratedTool; 5]);
        replies.push(Reply::Text(DSML));
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed at Acme?").await?;
        assert_eq!(f.fake.requests().len(), 8);
        assert_eq!(sse.matches("event: step").count(), 5);
        assert!(f.fake.requests()[6].get("tools").is_none());
        assert!(sse.contains("event: error"));
        assert!(!sse.contains("event: done"));
        assert!(!sse.contains("DSML"));
        assert!(f.stored_answer().await?.is_none());
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn finalization_recovers_once_from_existing_evidence_without_tools() -> anyhow::Result<()> {
    const ANSWER: &str = "There are no matching entities in the supplied evidence.";
    for invalid in [
        Reply::Text(DSML),
        Reply::SplitText(&[
            "Let me examine it.\n\n<｜｜D",
            "SML｜｜ calls>\n",
            "<｜DSML｜ invoke name=\"entity_facts\">{}",
        ]),
        Reply::Empty,
        Reply::Tool("find_entities", r#"{"name":"forbidden"}"#),
        Reply::Finished("Incomplete final", "length"),
    ] {
        let mut replies = vec![Reply::NarratedTool; 6];
        replies.extend([invalid, Reply::Finished(ANSWER, "stop")]);
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed at Acme?").await?;
        assert!(sse.contains("event: done"), "{sse}");
        assert!(!sse.contains("event: error"), "{sse}");
        assert!(!sse.contains("DSML"));
        assert!(!sse.contains("Incomplete final"));
        assert_eq!(sse.matches(ANSWER).count(), 1);
        assert_eq!(sse.matches("event: step").count(), 6);
        assert_eq!(
            f.stored_answer().await?.unwrap(),
            "I will check the evidence.\n\n".repeat(6) + ANSWER
        );
        let reqs = f.fake.requests();
        assert_eq!(reqs.len(), 8);
        let recovery = &reqs[7];
        assert!(recovery.get("tools").is_none());
        assert!(recovery.get("tool_choice").is_none());
        let msgs = recovery["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(msgs
            .iter()
            .all(|m| m.get("tool_calls").is_none() && m["role"] != "tool"));
        let data: serde_json::Value = serde_json::from_str(msgs[1]["content"].as_str().unwrap())?;
        assert_eq!(data["question"], "What changed at Acme?");
        assert_eq!(
            reqs[6]["messages"][1], reqs[7]["messages"][1],
            "same frozen evidence on repair"
        );
        let stored_exchange: serde_json::Value = sqlx::query_scalar(
            "SELECT m.tool_exchange FROM conversation_messages m JOIN conversations c ON c.id=m.conversation_id WHERE c.kb_id=$1 AND m.role='assistant'"
        ).bind(f.kb).fetch_one(&f.pool).await?;
        let original: Vec<_> = stored_exchange
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool")
            .collect();
        assert_eq!(data["evidence"].as_array().unwrap().len(), original.len());
        for (copied, original) in data["evidence"].as_array().unwrap().iter().zip(original) {
            assert_eq!(
                copied["result"], original["content"],
                "evidence is copied exactly"
            );
            assert_eq!(copied["id"], original["tool_call_id"]);
        }
        assert!(!msgs[1]["content"].as_str().unwrap().contains("DSML"));
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn unsuccessful_recovery_never_loops_or_reopens_tools() -> anyhow::Result<()> {
    for failed in [
        Reply::Text(DSML),
        Reply::SplitText(&[
            "Let me examine it.\n\n<｜｜D",
            "SML｜｜ calls>\n",
            "<｜DSML｜ invoke name=\"entity_facts\">{}",
        ]),
        Reply::Empty,
        Reply::Tool("find_entities", r#"{"name":"forbidden"}"#),
        Reply::Finished("partial", "length"),
        Reply::Http(422),
        Reply::Http(401),
    ] {
        let mut replies = vec![Reply::NarratedTool; 6];
        replies.extend([
            Reply::Text(DSML),
            failed,
            Reply::Text("Must never be requested"),
        ]);
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed at Acme?").await?;
        assert_eq!(f.fake.requests().len(), 8);
        assert_eq!(sse.matches("event: step").count(), 6);
        assert!(sse.contains("event: error"));
        assert!(!sse.contains("event: done"));
        assert!(!sse.contains("DSML"));
        assert!(!sse.contains("partial"));
        assert!(f.stored_answer().await?.is_none());
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn final_sources_include_document_citations_live_and_after_reload() -> anyhow::Result<()> {
    for recover in [false, true] {
        let document = Uuid::now_v7();
        let mut replies = vec![Reply::Document(document); if recover { 6 } else { 1 }];
        if recover {
            replies.push(Reply::Text("<DSMLtool_calls>"));
        }
        replies.push(Reply::Text("The documented target is 95% [1]."));
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256) VALUES($1,$2,'target.md',repeat('0',64))")
            .bind(document).bind(f.kb).execute(&f.pool).await?;
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,0,'The planned target is 95%, not a measured result.')")
            .bind(Uuid::now_v7()).bind(f.kb).bind(document).execute(&f.pool).await?;
        let sse = f.ask("What is the documented target?").await?;
        assert!(sse.contains("event: done"), "{sse}");
        assert!(!sse.contains("event: error"), "{sse}");
        let sources_frame = sse
            .split("\n\n")
            .filter(|frame| frame.starts_with("event: sources\n"))
            .last()
            .expect("final sources frame");
        let sources: serde_json::Value = serde_json::from_str(
            sources_frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap(),
        )?;
        assert_eq!(sources.as_array().unwrap().len(), 1);
        assert_eq!(sources[0]["n"], 1);
        let stored: serde_json::Value = sqlx::query_scalar(
            "SELECT m.sources FROM conversation_messages m JOIN conversations c ON c.id=m.conversation_id WHERE c.kb_id=$1 AND m.role='assistant'"
        ).bind(f.kb).fetch_one(&f.pool).await?;
        assert_eq!(sources, stored);
        assert_eq!(
            f.stored_answer().await?.as_deref(),
            Some("The documented target is 95% [1].")
        );
        assert_eq!(f.fake.requests().len(), if recover { 8 } else { 2 });
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn final_answer_transport_and_nonrepairable_finishes_do_not_retry() -> anyhow::Result<()> {
    for failure in [
        Reply::Http(400),
        Reply::Http(401),
        Reply::Http(402),
        Reply::Http(403),
        Reply::Http(422),
        Reply::Http(429),
        Reply::Finished("", "content_filter"),
        Reply::Finished("partial", "unknown_provider_reason"),
    ] {
        let mut replies = vec![Reply::NarratedTool; 6];
        replies.extend([failure, Reply::Text("Never requested")]);
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed?").await?;
        assert_eq!(f.fake.requests().len(), 7);
        assert!(sse.contains("event: error"));
        assert!(!sse.contains("event: done"));
        assert!(!sse.contains("Never requested"));
        assert!(f.stored_answer().await?.is_none());
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn failed_tool_observation_is_not_reported_as_empty_knowledge() -> anyhow::Result<()> {
    let mut replies = vec![Reply::NarratedTool; 5];
    replies.extend([
        Reply::Tool("get_document", "{}"),
        Reply::Text("The document could not be read."),
    ]);
    let Some(f) = fixture(Scripted::new(replies)).await? else {
        return Ok(());
    };
    let sse = f.ask("Read the document.").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let reqs = f.fake.requests();
    let data: serde_json::Value =
        serde_json::from_str(reqs[6]["messages"][1]["content"].as_str().unwrap())?;
    assert_eq!(data["evidence"][5]["status"], "error");
    assert_eq!(data["evidence"].as_array().unwrap().len(), 6);
    f.cleanup().await
}

#[tokio::test]
async fn save_failure_is_an_error_without_publishing_the_buffered_answer_or_retrying(
) -> anyhow::Result<()> {
    for budget in [false, true] {
        let mut replies = vec![Reply::NarratedTool; if budget { 6 } else { 1 }];
        replies.push(Reply::Text("Accepted final answer."));
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let name = format!("reject_assistant_{}", f.kb.simple());
        sqlx::raw_sql(&format!("CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.role='assistant' AND EXISTS(SELECT 1 FROM conversations WHERE id=NEW.conversation_id AND kb_id='{}') THEN RAISE EXCEPTION 'injected persistence failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER {name} BEFORE INSERT ON conversation_messages FOR EACH ROW EXECUTE FUNCTION {name}();",f.kb)).execute(&f.pool).await?;
        let sse = f.ask("What happened?").await?;
        sqlx::raw_sql(&format!(
            "DROP TRIGGER {name} ON conversation_messages; DROP FUNCTION {name}();"
        ))
        .execute(&f.pool)
        .await?;
        assert!(sse.contains("event: error"), "{sse}");
        assert!(!sse.contains("event: done"), "{sse}");
        if budget {
            assert!(
                !sse.contains("Accepted final answer."),
                "uncommitted final text must remain private"
            );
        }
        assert_eq!(f.fake.requests().len(), if budget { 7 } else { 2 });
        assert!(f.stored_answer().await?.is_none());
        f.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn concurrent_chats_have_independent_handoff_and_repair_budgets() -> anyhow::Result<()> {
    let mut a = vec![Reply::NarratedTool; 6];
    a.extend([Reply::Text(DSML), Reply::Text("Recovered A")]);
    let mut b = vec![Reply::NarratedTool; 6];
    b.push(Reply::Text("Direct B"));
    let Some(a) = fixture(Scripted::new(a)).await? else {
        return Ok(());
    };
    let Some(b) = fixture(Scripted::new(b)).await? else {
        return Ok(());
    };
    let (ra, rb) = tokio::join!(a.ask("Question A"), b.ask("Question B"));
    let ra = ra?;
    let rb = rb?;
    assert!(ra.contains("Recovered A") && !ra.contains("Direct B"));
    assert!(rb.contains("Direct B") && !rb.contains("Recovered A"));
    assert_eq!(a.fake.requests().len(), 8);
    assert_eq!(b.fake.requests().len(), 7);
    a.cleanup().await?;
    b.cleanup().await
}

#[tokio::test]
async fn parallel_tool_results_and_utf16_step_positions_survive_handoff() -> anyhow::Result<()> {
    let mut replies = vec![Reply::ParallelTools; 6];
    replies.push(Reply::Text("最终答案"));
    let Some(f) = fixture(Scripted::new(replies)).await? else {
        return Ok(());
    };
    let sse = f.ask("查到什么？").await?;
    assert!(sse.contains("event: done"), "{sse}");
    assert_eq!(f.fake.requests().len(), 7);
    let data: serde_json::Value = serde_json::from_str(
        f.fake.requests()[6]["messages"][1]["content"]
            .as_str()
            .unwrap(),
    )?;
    let evidence = data["evidence"].as_array().unwrap();
    assert_eq!(evidence.len(), 12);
    let ids: std::collections::HashSet<_> =
        evidence.iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids.len(), 12);
    assert!(evidence.iter().all(|e| e["status"] == "success"));
    let steps: Vec<serde_json::Value> = sse
        .split("\n\n")
        .filter_map(|b| b.strip_prefix("event: step\ndata: "))
        .map(|d| serde_json::from_str(d).unwrap())
        .collect();
    assert_eq!(steps.len(), 12);
    let width = "核查😀\n\n".encode_utf16().count();
    for (i, step) in steps.iter().enumerate() {
        assert_eq!(step["at"], serde_json::json!((i / 2 + 1) * width));
    }
    assert_eq!(
        f.stored_answer().await?.unwrap(),
        "核查😀\n\n".repeat(6) + "最终答案"
    );
    f.cleanup().await
}

#[tokio::test]
async fn early_retry_budget_and_no_evidence_short_path_remain_bounded() -> anyhow::Result<()> {
    for first in [Reply::Empty, Reply::Text("Let me check.")] {
        let mut replies = vec![first];
        replies.extend(vec![Reply::NarratedTool; 5]);
        replies.push(Reply::Text("The evidence is incomplete."));
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed?").await?;
        assert!(sse.contains("event: done"), "{sse}");
        let requests = f.fake.requests();
        assert_eq!(requests.len(), 7);
        assert!(requests[6].get("tools").is_none());
        let data: serde_json::Value =
            serde_json::from_str(requests[6]["messages"][1]["content"].as_str().unwrap())?;
        assert_eq!(data["evidence"].as_array().unwrap().len(), 5);
        f.cleanup().await?;
    }
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("no_evidence_needed", "{\"reason\":\"Greeting\"}"),
        Reply::Text("Hello!"),
    ]))
    .await?
    else {
        return Ok(());
    };
    let sse = f.ask("Hello").await?;
    assert!(sse.contains("event: done"), "{sse}");
    assert_eq!(f.fake.requests().len(), 2);
    assert_eq!(f.stored_answer().await?.as_deref(), Some("Hello!"));
    f.cleanup().await
}

#[tokio::test]
async fn gathering_errors_cannot_spoof_the_private_handoff() -> anyhow::Result<()> {
    for status in [401, 500] {
        let mut replies = vec![Reply::NarratedTool; 5];
        replies.extend([Reply::Http(status), Reply::Text("Never requested")]);
        let Some(f) = fixture(Scripted::new(replies)).await? else {
            return Ok(());
        };
        let sse = f.ask("What changed?").await?;
        assert_eq!(f.fake.requests().len(), 6);
        assert!(sse.contains("event: error"));
        assert!(!sse.contains("event: done"));
        assert!(f.stored_answer().await?.is_none());
        f.cleanup().await?;
    }
    Ok(())
}
