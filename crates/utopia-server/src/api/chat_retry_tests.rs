//! 重答（#936）：没有回答的最后一问，就地再答一次。
//!
//! 从前答到一半失败，问题留在库里没有回答；想再问只能重发，同一个问题就在历史里存了
//! 两遍，没回答的那一遍还会在之后每一问里作为「没回答的一轮」回放给模型。重答带着
//! 那一问存下的 id 来：不再存一遍，答案落在它后面。
use super::*;
use axum::response::IntoResponse;

const TOOL: Reply = Reply::Tool("find_entities", r#"{"name":"Acme"}"#);
const QUESTION: &str = "What changed at Acme?";

/// 一次请求的结果：SSE 全文，或者被拒时的状态码与 `{error, code}`
enum Outcome {
    Streamed(String),
    Refused(u16, serde_json::Value),
}

async fn post(f: &Fx, body: serde_json::Value) -> anyhow::Result<Outcome> {
    let req: ChatReq = serde_json::from_value(body)?;
    match chat(
        State(f.state.clone()),
        AuthUser(f.user.clone()),
        Path(f.kb),
        Json(req),
    )
    .await
    {
        Ok(sse) => {
            let body =
                axum::body::to_bytes(sse.into_response().into_body(), 4 * 1024 * 1024).await?;
            Ok(Outcome::Streamed(
                String::from_utf8_lossy(&body).into_owned(),
            ))
        }
        Err(refused) => {
            let response = refused.into_response();
            let status = response.status().as_u16();
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await?;
            Ok(Outcome::Refused(
                status,
                serde_json::from_slice(&body).unwrap_or_default(),
            ))
        }
    }
}

/// 这场对话存下的消息：(id, 角色, 正文)，按时间序
async fn stored(f: &Fx, conversation_id: Uuid) -> anyhow::Result<Vec<(Uuid, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, role, content FROM conversation_messages
          WHERE conversation_id = $1 ORDER BY created_at, id",
    )
    .bind(conversation_id)
    .fetch_all(&f.pool)
    .await?)
}

async fn only_conversation(f: &Fx) -> anyhow::Result<Uuid> {
    Ok(
        sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id = $1")
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?,
    )
}

/// 直接往库里放一条消息：失败之后、接着问之后那些状态，不必每次都让模型演一遍
async fn append(f: &Fx, conversation_id: Uuid, role: &str, content: &str) -> anyhow::Result<Uuid> {
    Ok(utopia_store::conversations::append_message(
        &f.pool,
        conversation_id,
        role,
        content,
        &utopia_store::conversations::TurnRecord::empty(),
    )
    .await?)
}

fn retry(conversation_id: Uuid, question_id: Uuid) -> serde_json::Value {
    json!({
        "conversation_id": conversation_id,
        "message": QUESTION,
        "retry_message_id": question_id,
    })
}

/// 答到一半失败的那一问，就地再答一次：库里还是一问一答，模型也只看见它一次
#[tokio::test]
async fn a_question_whose_answer_failed_is_answered_in_place() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Http(401),
        TOOL,
        Reply::Text("Retried answer."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let first = f.ask(QUESTION).await?;
    assert!(
        first.contains("event: error"),
        "the first answer fails:\n{first}"
    );
    let conversation_id = only_conversation(&f).await?;
    let before = stored(&f, conversation_id).await?;
    assert_eq!(before.len(), 1, "only the question is stored: {before:?}");
    let question_id = before[0].0;
    let asked_before = f.requests().len();

    let Outcome::Streamed(sse) = post(&f, retry(conversation_id, question_id)).await? else {
        panic!("the retry was refused");
    };
    assert!(sse.contains("event: done"), "{sse}");
    // `conversation` 那一帧带着这一问存下的 id：界面凭它重答
    assert!(
        sse.contains(&format!("\"message_id\":\"{question_id}\"")),
        "{sse}"
    );
    // 还是那一问，后面跟着答案；问题没有存第二遍
    let after = stored(&f, conversation_id).await?;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(
        (after[0].0, after[0].1.as_str(), after[0].2.as_str()),
        (question_id, "user", QUESTION)
    );
    assert_eq!(
        (after[1].1.as_str(), after[1].2.as_str()),
        ("assistant", "Retried answer.")
    );
    // 重答的第一次模型请求里，这个问题只出现一次
    let request = &f.requests()[asked_before];
    let asked = request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["role"] == "user" && m["content"].as_str().is_some_and(|c| c.contains(QUESTION))
        })
        .count();
    assert_eq!(asked, 1, "{request}");
    f.cleanup().await
}

/// 答过的问题不再答：409，模型不被请求，库里不多一条
#[tokio::test]
async fn an_answered_question_is_not_answered_again() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![TOOL, Reply::Text("First answer.")])).await? else {
        return Ok(());
    };
    let sse = f.ask(QUESTION).await?;
    assert!(sse.contains("event: done"), "{sse}");
    let conversation_id = only_conversation(&f).await?;
    let before = stored(&f, conversation_id).await?;
    let asked = f.requests().len();
    let Outcome::Refused(status, body) = post(&f, retry(conversation_id, before[0].0)).await?
    else {
        panic!("an answered question was answered again");
    };
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("retry_answered")),
        "{body}"
    );
    assert_eq!(
        stored(&f, conversation_id).await?,
        before,
        "nothing is stored"
    );
    assert_eq!(f.requests().len(), asked, "the model is not asked");
    f.cleanup().await
}

/// 后面有人接着问了，这一问也不再原地重答：答案会落在后来那些轮之后
#[tokio::test]
async fn a_question_the_conversation_went_on_from_is_not_answered_again() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let conversation_id =
        utopia_store::conversations::create(&f.pool, f.kb, f.user.id, QUESTION).await?;
    let first = append(&f, conversation_id, "user", QUESTION).await?;
    append(&f, conversation_id, "user", "And at Globex?").await?;
    let before = stored(&f, conversation_id).await?;
    let Outcome::Refused(status, body) = post(&f, retry(conversation_id, first)).await? else {
        panic!("a question the conversation went on from was answered again");
    };
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("retry_answered")),
        "{body}"
    );
    assert_eq!(stored(&f, conversation_id).await?, before);
    assert!(f.requests().is_empty(), "the model is not asked");
    f.cleanup().await
}

/// 这场对话里正有一个回答在写：409；它结束之后，同一个请求就能答
#[tokio::test]
async fn a_question_is_not_answered_twice_at_once() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![TOOL, Reply::Text("Retried answer.")])).await? else {
        return Ok(());
    };
    let conversation_id =
        utopia_store::conversations::create(&f.pool, f.kb, f.user.id, QUESTION).await?;
    let question = append(&f, conversation_id, "user", QUESTION).await?;
    let running = f.state.live.begin(conversation_id).await;
    let Outcome::Refused(status, body) = post(&f, retry(conversation_id, question)).await? else {
        panic!("a retry started while an answer was running");
    };
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("answer_running")),
        "{body}"
    );
    assert!(f.requests().is_empty(), "the model is not asked");
    assert_eq!(stored(&f, conversation_id).await?.len(), 1);

    running.finish().await;
    let Outcome::Streamed(sse) = post(&f, retry(conversation_id, question)).await? else {
        panic!("the retry was refused after the running answer finished");
    };
    assert!(sse.contains("event: done"), "{sse}");
    let after = stored(&f, conversation_id).await?;
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0].0, question);
    f.cleanup().await
}

/// 重答点名的必须是这场对话里的一个问题
#[tokio::test]
async fn a_retry_names_a_question_of_this_conversation() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    let conversation_id =
        utopia_store::conversations::create(&f.pool, f.kb, f.user.id, QUESTION).await?;
    let question = append(&f, conversation_id, "user", QUESTION).await?;
    let answer = append(&f, conversation_id, "assistant", "An answer.").await?;
    let elsewhere =
        utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "Another").await?;
    let foreign = append(&f, elsewhere, "user", "Another question").await?;
    for (body, status, code) in [
        (
            json!({ "message": QUESTION, "retry_message_id": question }),
            422,
            Some("retry_needs_conversation"),
        ),
        (
            retry(conversation_id, answer),
            422,
            Some("retry_not_question"),
        ),
        (retry(conversation_id, foreign), 404, None),
        (retry(conversation_id, Uuid::now_v7()), 404, None),
    ] {
        let Outcome::Refused(got, refusal) = post(&f, body.clone()).await? else {
            panic!("{body} was not refused");
        };
        assert_eq!(got, status, "{body}: {refusal}");
        if let Some(code) = code {
            assert_eq!(refusal["code"], code, "{body}");
        }
    }
    assert!(f.requests().is_empty(), "the model is not asked");
    assert_eq!(stored(&f, conversation_id).await?.len(), 2);
    assert_eq!(stored(&f, elsewhere).await?.len(), 1);
    f.cleanup().await
}

/// 每一问都把它存下的 id 交给界面：答到一半失败时，界面凭它重答
#[tokio::test]
async fn every_question_tells_the_interface_its_stored_id() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![TOOL, Reply::Text("An answer.")])).await? else {
        return Ok(());
    };
    let sse = f.ask(QUESTION).await?;
    let conversation_id = only_conversation(&f).await?;
    let question = stored(&f, conversation_id).await?[0].0;
    assert!(
        sse.contains(&format!("\"message_id\":\"{question}\"")),
        "{sse}"
    );
    f.cleanup().await
}
