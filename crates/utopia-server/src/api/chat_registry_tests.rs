//! 用真实生成流程和可控的本地模型验证 Stop，与 SSE 断开分开验证。
use super::*;
use axum::http::StatusCode;
use axum::response::Response;
use tokio::sync::{broadcast, Notify};

const PARTIAL: &str = "Checking the available evidence.\n";
const DRAFT: &str = "Unpublished final draft.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pause {
    BeforeText,
    BeforeTool,
    DuringTool,
    AfterTool,
    Legacy,
    FinalAnswer,
}

#[derive(Clone)]
struct Gates {
    pause: Pause,
    ready: [Arc<Notify>; 2],
    release: [Arc<Notify>; 2],
    requests: Arc<Mutex<Vec<serde_json::Value>>>,
    tool_started: Arc<Notify>,
    tool_disconnected: Arc<Notify>,
}

async fn upstream(State(gates): State<Gates>, Json(body): Json<serde_json::Value>) -> Response {
    let messages = body["messages"].as_array().expect("messages");
    let question = messages.iter().rposition(|m| m["role"] == "user").unwrap();
    let index = usize::from(
        messages[question]["content"]
            .to_string()
            .contains("second greeting"),
    );
    let tool_ran = messages[question + 1..].iter().any(|m| m["role"] == "tool");
    let has_tools = body.get("tools").is_some();
    let request_number = {
        let mut requests = gates.requests.lock().unwrap();
        requests.push(body);
        requests.len()
    };
    if gates.pause == Pause::Legacy && has_tools {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":{"message":"tools unsupported"}})),
        )
            .into_response();
    }
    let tool_turn = match gates.pause {
        Pause::Legacy => false,
        Pause::FinalAnswer => has_tools,
        _ => !tool_ran,
    };
    let paused = match gates.pause {
        Pause::BeforeText | Pause::BeforeTool => tool_turn,
        Pause::DuringTool => false,
        _ => !tool_turn,
    };
    let stream = async_stream::stream! {
        let text = if gates.pause == Pause::FinalAnswer && !tool_turn { DRAFT } else { PARTIAL };
        let delta = json!({"choices":[{"delta":{"content":text}}]});
        if gates.pause != Pause::BeforeText {
            yield Ok::<_, Infallible>(Event::default().data(delta.to_string()));
        }
        if paused {
            gates.ready[index].notify_one();
            gates.release[index].notified().await;
        }
        let payload = if tool_turn {
            let (name, arguments) = match gates.pause {
                Pause::FinalAnswer => ("find_entities", r#"{"name":"Acme"}"#),
                Pause::DuringTool => ("search_chunks", r#"{"query":"Acme"}"#),
                _ => ("no_evidence_needed", r#"{"reason":"greeting"}"#),
            };
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":format!("call_{request_number}"),"function":{"name":name,"arguments":arguments}}]},"finish_reason":"tool_calls"}]})
        } else {
            json!({"choices":[{"delta":{"content":if index == 0 {"First answer."} else {"Second answer."}},"finish_reason":"stop"}]})
        };
        yield Ok::<_, Infallible>(Event::default().data(payload.to_string()));
        yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
    };
    Sse::new(stream).into_response()
}

struct ConnectionDropped(Arc<Notify>);

impl Drop for ConnectionDropped {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

async fn embedding(State(gates): State<Gates>, Json(body): Json<serde_json::Value>) -> Response {
    assert_eq!(body["input"], json!(["Acme"]));
    let stream = async_stream::stream! {
        let _connection = ConnectionDropped(gates.tool_disconnected.clone());
        yield Ok::<_, Infallible>(axum::body::Bytes::from_static(b"{\"data\":["));
        // 服务端已发出部分响应；真实 search_chunks 工具仍停在 embed()，
        // 等待这个 JSON 文档的剩余部分。
        gates.tool_started.notify_one();
        gates.release[0].notified().await;
        yield Ok::<_, Infallible>(axum::body::Bytes::from_static(b"]}"));
    };
    (
        [("content-type", "application/json")],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

type ModelServer = tokio::task::JoinHandle<std::io::Result<()>>;

async fn setup(pause: Pause) -> anyhow::Result<Option<(Fx, Gates, ModelServer)>> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(None);
    };
    let gates = Gates {
        pause,
        ready: Default::default(),
        release: Default::default(),
        requests: Default::default(),
        tool_started: Default::default(),
        tool_disconnected: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = axum::Router::new()
        .route("/chat/completions", axum::routing::post(upstream))
        .route("/embeddings", axum::routing::post(embedding))
        .with_state(gates.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    sqlx::query("UPDATE llm_settings SET chat_base_url=$1 WHERE workspace_id=(SELECT workspace_id FROM knowledge_bases WHERE id=$2)")
        .bind(base).bind(f.kb).execute(&f.pool).await?;
    if pause == Pause::DuringTool {
        sqlx::query("UPDATE llm_settings SET embed_base_url=chat_base_url, embed_model='gated-embedding' WHERE workspace_id=(SELECT workspace_id FROM knowledge_bases WHERE id=$1)")
            .bind(f.kb).execute(&f.pool).await?;
    }
    Ok(Some((f, gates, server)))
}

async fn start(f: &Fx, id: Uuid, question: &str) -> ApiResult<Response> {
    chat(
        State(f.state.clone()),
        AuthUser(f.user.clone()),
        Path(f.kb),
        Json(ChatReq {
            conversation_id: Some(id),
            message: question.into(),
            retry_message_id: None,
        }),
    )
    .await
    .map(IntoResponse::into_response)
}

async fn request_stop(f: &Fx, id: Uuid, generation_id: Uuid) -> anyhow::Result<()> {
    let _ = stop(
        State(f.state.clone()),
        AuthUser(f.user.clone()),
        Path((f.kb, id)),
        Json(StopReq { generation_id }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("stop refused"))?;
    Ok(())
}

async fn finished(rx: &mut broadcast::Receiver<Frame>) -> anyhow::Result<Vec<Frame>> {
    let mut frames = Vec::new();
    loop {
        match rx.recv().await {
            Ok(frame) => frames.push(frame),
            Err(broadcast::error::RecvError::Closed) => return Ok(frames),
            Err(error) => return Err(error.into()),
        }
    }
}

fn assert_done(frames: &[Frame], stopped: bool) {
    let terminals: Vec<_> = frames
        .iter()
        .filter(|f| matches!(f.event, "done" | "error"))
        .collect();
    assert_eq!(terminals.len(), 1, "{frames:?}");
    assert_eq!(terminals[0].event, "done", "{frames:?}");
    let data: serde_json::Value = serde_json::from_str(&terminals[0].data).unwrap();
    assert_eq!(data["stopped"].as_bool().unwrap_or(false), stopped);
}

#[tokio::test]
async fn stop_saves_partial_text_and_stops_model_requests_at_each_stage() -> anyhow::Result<()> {
    for (pause, expected_requests) in [
        (Pause::BeforeText, 1),
        (Pause::BeforeTool, 1),
        (Pause::AfterTool, 2),
        (Pause::Legacy, 3),
        (Pause::FinalAnswer, 7),
    ] {
        let Some((f, gates, server)) = setup(pause).await? else {
            return Ok(());
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let id = utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "stop").await?;
            let response = start(&f, id, "first greeting")
                .await
                .map_err(|_| anyhow::anyhow!("chat refused"))?;
            let (snapshot, mut rx) = f.state.live.attach(id).await.unwrap();
            gates.ready[0].notified().await;
            // 上游可能在响应字节到达生成器前发信号。
            // 等待正文真正发布，不依赖固定睡眠。
            if pause != Pause::BeforeText && snapshot.content.is_empty() {
                while rx.recv().await?.event != "delta" {}
            }
            request_stop(&f, id, snapshot.generation_id).await?;
            request_stop(&f, id, snapshot.generation_id).await?;
            let frames = finished(&mut rx).await?;
            assert_done(&frames, true);
            let body = axum::body::to_bytes(response.into_body(), 65536).await?;
            let sse = String::from_utf8_lossy(&body);
            anyhow::ensure!(
                sse.matches("event: done").count() == 1 && !sse.contains("event: error"),
                "{pause:?}: {sse}"
            );
            let messages = utopia_store::conversations::messages(&f.pool, id).await?;
            anyhow::ensure!(messages.len() == 2 && messages[0].role == "user");
            let answer = &messages[1];
            anyhow::ensure!(answer.role == "assistant" && answer.stopped);
            if pause == Pause::BeforeText {
                anyhow::ensure!(answer.content.is_empty(), "stopped before the first token");
            } else {
                anyhow::ensure!(answer.content.contains(PARTIAL), "{pause:?}: {answer:?}");
            }
            anyhow::ensure!(
                !answer.content.contains("First answer.") && !answer.content.contains(DRAFT)
            );
            anyhow::ensure!(f.state.live.attach(id).await.is_none());
            anyhow::ensure!(
                gates.requests.lock().unwrap().len() == expected_requests,
                "{pause:?}: no request may start after Stop"
            );
            if pause == Pause::AfterTool {
                let followup = start(&f, id, "second greeting")
                    .await
                    .map_err(|_| anyhow::anyhow!("follow-up refused after Stop"))?;
                gates.ready[1].notified().await;
                gates.release[1].notify_one();
                let body = axum::body::to_bytes(followup.into_body(), 65536).await?;
                anyhow::ensure!(String::from_utf8_lossy(&body).contains("Second answer."));
                let messages = utopia_store::conversations::messages(&f.pool, id).await?;
                anyhow::ensure!(messages.len() == 4 && messages[1].stopped && !messages[3].stopped);
            }
            Ok::<_, anyhow::Error>(())
        })
        .await;
        gates.release[0].notify_one();
        gates.release[1].notify_one();
        server.abort();
        let _ = server.await;
        f.cleanup().await?;
        result??;
    }
    Ok(())
}

#[tokio::test]
async fn stop_during_a_real_search_tool_closes_its_http_request_and_saves_partial(
) -> anyhow::Result<()> {
    for retry in [false, true] {
        let Some((f, gates, server)) = setup(Pause::DuringTool).await? else {
            return Ok(());
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let id =
                utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "tool in flight")
                    .await?;
            // 无回答的问题代表上一轮失败；重试必须沿用这个 id，不能再插入问题。
            let question_id = if retry {
                Some(
                    utopia_store::conversations::append_message(
                        &f.pool,
                        id,
                        "user",
                        "first greeting",
                        &utopia_store::conversations::TurnRecord::empty(),
                    )
                    .await?,
                )
            } else {
                None
            };
            let response = chat(
                State(f.state.clone()),
                AuthUser(f.user.clone()),
                Path(f.kb),
                Json(ChatReq {
                    conversation_id: Some(id),
                    message: "first greeting".into(),
                    retry_message_id: question_id,
                }),
            )
            .await
            .map_err(|_| anyhow::anyhow!("chat refused"))?
            .into_response();
            let (snapshot, mut rx) = f.state.live.attach(id).await.unwrap();
            gates.tool_started.notified().await;

            let stored_question = utopia_store::conversations::messages(&f.pool, id).await?[0].id;
            for retry_message_id in [None, Some(stored_question)] {
                let busy = chat(
                    State(f.state.clone()),
                    AuthUser(f.user.clone()),
                    Path(f.kb),
                    Json(ChatReq {
                        conversation_id: Some(id),
                        message: "overlapping question".into(),
                        retry_message_id,
                    }),
                )
                .await
                .map(IntoResponse::into_response)
                .expect_err("new questions and retries share admission")
                .into_response();
                anyhow::ensure!(busy.status() == StatusCode::CONFLICT);
                let body = axum::body::to_bytes(busy.into_body(), 4096).await?;
                anyhow::ensure!(
                    serde_json::from_slice::<serde_json::Value>(&body)?["code"] == "answer_running"
                );
            }
            request_stop(&f, id, snapshot.generation_id).await?;
            // 保持 embedding 响应阻塞，只有取消才能关闭它。
            gates.tool_disconnected.notified().await;
            assert_done(&finished(&mut rx).await?, true);
            let body = axum::body::to_bytes(response.into_body(), 65536).await?;
            let sse = String::from_utf8_lossy(&body);
            anyhow::ensure!(
                sse.matches("event: done").count() == 1 && !sse.contains("event: error"),
                "{sse}"
            );
            anyhow::ensure!(
                !sse.contains("event: step"),
                "the interrupted tool did not complete"
            );
            anyhow::ensure!(sse.contains(&format!("\"message_id\":\"{stored_question}\"")));
            anyhow::ensure!(
                sse.contains(&format!("\"generation_id\":\"{}\"", snapshot.generation_id))
            );
            let messages = utopia_store::conversations::messages(&f.pool, id).await?;
            if let Some(question_id) = question_id {
                anyhow::ensure!(messages[0].id == question_id);
            }
            anyhow::ensure!(messages.len() == 2 && messages[1].stopped);
            anyhow::ensure!(messages[1].content == PARTIAL && messages[1].steps == json!([]));
            let exchange: serde_json::Value =
                sqlx::query_scalar("SELECT tool_exchange FROM conversation_messages WHERE id=$1")
                    .bind(messages[1].id)
                    .fetch_one(&f.pool)
                    .await?;
            anyhow::ensure!(
                exchange == json!([]),
                "no unfinished tool call may enter the next turn"
            );
            anyhow::ensure!(
                gates.requests.lock().unwrap().len() == 1,
                "the stopped tool must not trigger another model turn"
            );
            anyhow::ensure!(f.state.live.attach(id).await.is_none());
            Ok::<_, anyhow::Error>(())
        })
        .await;
        gates.release[0].notify_one();
        server.abort();
        let _ = server.await;
        f.cleanup().await?;
        result??;
    }
    Ok(())
}

#[tokio::test]
async fn disconnect_keeps_running_busy_does_not_append_and_stale_stop_spares_followup(
) -> anyhow::Result<()> {
    let Some((f, gates, server)) = setup(Pause::AfterTool).await? else {
        return Ok(());
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let id =
            utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "disconnect").await?;
        let response = start(&f, id, "first greeting")
            .await
            .map_err(|_| anyhow::anyhow!("chat refused"))?;
        gates.ready[0].notified().await;
        let (first, mut rx) = f.state.live.attach(id).await.unwrap();
        drop(response);
        let busy = start(&f, id, "second greeting")
            .await
            .expect_err("one generation per conversation")
            .into_response();
        anyhow::ensure!(busy.status() == StatusCode::CONFLICT);
        let body = axum::body::to_bytes(busy.into_body(), 4096).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        anyhow::ensure!(body["code"] == "answer_running");
        anyhow::ensure!(
            utopia_store::conversations::messages(&f.pool, id)
                .await?
                .len()
                == 1,
            "409 must not write the follow-up question"
        );
        gates.release[0].notify_one();
        assert_done(&finished(&mut rx).await?, false);

        let response = start(&f, id, "second greeting")
            .await
            .map_err(|_| anyhow::anyhow!("follow-up refused after completion"))?;
        gates.ready[1].notified().await;
        let (second, mut rx) = f.state.live.attach(id).await.unwrap();
        anyhow::ensure!(first.generation_id != second.generation_id);
        request_stop(&f, id, first.generation_id).await?;
        let mut other = f.user.clone();
        other.id = Uuid::now_v7();
        let unauthorized = stop(
            State(f.state.clone()),
            AuthUser(other),
            Path((f.kb, id)),
            Json(StopReq {
                generation_id: second.generation_id,
            }),
        )
        .await;
        anyhow::ensure!(
            unauthorized
                .expect_err("only conversation owner may stop")
                .into_response()
                .status()
                == StatusCode::NOT_FOUND
        );
        let wrong_base = stop(
            State(f.state.clone()),
            AuthUser(f.user.clone()),
            Path((Uuid::now_v7(), id)),
            Json(StopReq {
                generation_id: second.generation_id,
            }),
        )
        .await;
        anyhow::ensure!(
            wrong_base
                .expect_err("conversation belongs to its own base")
                .into_response()
                .status()
                == StatusCode::NOT_FOUND
        );
        gates.release[1].notify_one();
        assert_done(&finished(&mut rx).await?, false);
        let body = axum::body::to_bytes(response.into_body(), 65536).await?;
        anyhow::ensure!(String::from_utf8_lossy(&body).contains("Second answer."));
        let messages = utopia_store::conversations::messages(&f.pool, id).await?;
        let roles: Vec<_> = messages.iter().map(|m| m.role.as_str()).collect();
        anyhow::ensure!(roles == ["user", "assistant", "user", "assistant"]);
        anyhow::ensure!(messages[1].content.contains("First answer.") && !messages[1].stopped);
        anyhow::ensure!(messages[3].content.contains("Second answer.") && !messages[3].stopped);
        Ok::<_, anyhow::Error>(())
    })
    .await;
    gates.release[0].notify_one();
    gates.release[1].notify_one();
    server.abort();
    let _ = server.await;
    f.cleanup().await?;
    result??;
    Ok(())
}

#[tokio::test]
async fn stop_after_save_starts_keeps_one_complete_answer_and_holds_busy_until_commit(
) -> anyhow::Result<()> {
    let Some((f, gates, server)) = setup(Pause::AfterTool).await? else {
        return Ok(());
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let id =
            utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "commit race").await?;
        let response = start(&f, id, "first greeting")
            .await
            .map_err(|_| anyhow::anyhow!("chat refused"))?;
        gates.ready[0].notified().await;
        let (snapshot, mut rx) = f.state.live.attach(id).await.unwrap();

        // 让助手消息 INSERT 成功，再阻塞 append_message 的 UPDATE。
        // 观察真实数据库锁等待，确认发送 Stop 前保存已经开始。
        let mut lock = f.pool.begin().await?;
        sqlx::query("SELECT id FROM conversations WHERE id=$1 FOR NO KEY UPDATE")
            .bind(id)
            .execute(&mut *lock)
            .await?;
        let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *lock)
            .await?;
        gates.release[0].notify_one();
        while !sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))",
        )
        .bind(blocker)
        .fetch_one(&f.pool)
        .await?
        {}

        request_stop(&f, id, snapshot.generation_id).await?;
        let busy = start(&f, id, "second greeting")
            .await
            .expect_err("saving still owns the conversation")
            .into_response();
        anyhow::ensure!(busy.status() == StatusCode::CONFLICT);
        anyhow::ensure!(
            utopia_store::conversations::messages(&f.pool, id)
                .await?
                .len()
                == 1,
            "the unfinished save is invisible and the refused follow-up was not appended"
        );

        lock.commit().await?;
        assert_done(&finished(&mut rx).await?, false);
        let body = axum::body::to_bytes(response.into_body(), 65536).await?;
        anyhow::ensure!(String::from_utf8_lossy(&body).contains("First answer."));
        let messages = utopia_store::conversations::messages(&f.pool, id).await?;
        anyhow::ensure!(
            messages.len() == 2,
            "completion and Stop must not both write an answer"
        );
        anyhow::ensure!(messages[1].role == "assistant" && !messages[1].stopped);
        anyhow::ensure!(messages[1].content.ends_with("First answer."));
        anyhow::ensure!(f.state.live.attach(id).await.is_none());
        anyhow::ensure!(gates.requests.lock().unwrap().len() == 2);
        Ok::<_, anyhow::Error>(())
    })
    .await;
    gates.release[0].notify_one();
    server.abort();
    let _ = server.await;
    f.cleanup().await?;
    result??;
    Ok(())
}

#[tokio::test]
async fn stopped_answer_save_failure_emits_error_and_releases_the_conversation(
) -> anyhow::Result<()> {
    let Some((f, gates, server)) = setup(Pause::AfterTool).await? else {
        return Ok(());
    };
    let trigger = format!("reject_stopped_{}", f.kb.simple());
    sqlx::raw_sql(&format!("CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.role='assistant' AND EXISTS(SELECT 1 FROM conversations WHERE id=NEW.conversation_id AND kb_id='{}') THEN RAISE EXCEPTION 'private stop persistence diagnostic'; END IF; RETURN NEW; END $$; CREATE TRIGGER {trigger} BEFORE INSERT ON conversation_messages FOR EACH ROW EXECUTE FUNCTION {trigger}();", f.kb)).execute(&f.pool).await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let id =
            utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "save failure").await?;
        let response = start(&f, id, "first greeting")
            .await
            .map_err(|_| anyhow::anyhow!("chat refused"))?;
        gates.ready[0].notified().await;
        let (snapshot, mut rx) = f.state.live.attach(id).await.unwrap();
        request_stop(&f, id, snapshot.generation_id).await?;
        let frames = finished(&mut rx).await?;
        let terminals: Vec<_> = frames
            .iter()
            .filter(|f| matches!(f.event, "done" | "error"))
            .collect();
        anyhow::ensure!(
            terminals.len() == 1 && terminals[0].event == "error",
            "{frames:?}"
        );
        let error: serde_json::Value = serde_json::from_str(&terminals[0].data)?;
        anyhow::ensure!(error["code"] == "answer_not_saved");
        anyhow::ensure!(!terminals[0]
            .data
            .contains("private stop persistence diagnostic"));
        anyhow::ensure!(f.state.live.attach(id).await.is_none());
        anyhow::ensure!(
            utopia_store::conversations::messages(&f.pool, id)
                .await?
                .len()
                == 1
        );
        drop(response);
        Ok::<_, anyhow::Error>(())
    })
    .await;
    gates.release[0].notify_one();
    server.abort();
    let _ = server.await;
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {trigger} ON conversation_messages; DROP FUNCTION {trigger}();"
    ))
    .execute(&f.pool)
    .await?;
    f.cleanup().await?;
    result??;
    Ok(())
}
