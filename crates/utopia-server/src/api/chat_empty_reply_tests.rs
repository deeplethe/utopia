//! 空回复重问一次（`EMPTY_REPLY_RETRY`）。
//!
//! 走整条对话：直接调 `chat` 处理函数，把它返回的 SSE 收完读帧。模型由 wiremock 按脚本
//! 假扮成 OpenAI 兼容的 `/chat/completions`——第几次请求回什么事先写好，于是「模型回空」
//! 这件真模型上几十轮才碰一次的事，在这里每次都发生。
//!
//! 1. **查完之后空一次，重问后答出来。** 这是台子上碰到的真实形状：工具先跑完，模型接着
//!    回空。用户看到的是查证步骤接着答案，不是报错；重问只发了一次，它的末尾正是那句
//!    重问；落库的助手消息就是重问后的答案。
//!    （脚本若让模型一步不查就答，#509 的守卫会因为「答案什么都不站在上面」再追问一次——
//!    那是另一件事，所以这里先调一个工具，与真实失败一致。）
//! 2. **一直空，报错，而且只重问一次。** 两次请求之后是 `error` 帧——不是第三次、第四次：
//!    一个始终不说话的端点不能让循环空转。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

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
enum Reply {
    /// 没有正文，也不调工具
    Empty,
    Text(&'static str),
    /// 调一个工具：(名字, 参数 JSON)
    Tool(&'static str, &'static str),
}

/// 按脚本回话的假模型。`replies[i]` 是第 i+1 次请求的回复，脚本读完之后一律回空。
/// 每次请求的正文都记下来，好查重问那一次问了什么
#[derive(Clone)]
struct Scripted {
    replies: Arc<Vec<Reply>>,
    seen: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Scripted {
    fn new(replies: Vec<Reply>) -> Self {
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

struct Fx {
    state: AppState,
    pool: sqlx::PgPool,
    org: Uuid,
    kb: Uuid,
    user: User,
    fake: Scripted,
    _server: MockServer,
    dir: std::path::PathBuf,
}

async fn fixture(fake: Scripted) -> anyhow::Result<Option<Fx>> {
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
    async fn ask(&self, message: &str) -> anyhow::Result<String> {
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

    async fn cleanup(self) -> anyhow::Result<()> {
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
