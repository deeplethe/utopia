//! The fallback answer can be streamed before its save; a failed save must still
//! terminate as error, including when the initiating browser has disconnected.
use super::*;

#[derive(Clone, Default)]
struct LegacyOnly(Arc<Mutex<Vec<serde_json::Value>>>);
impl Respond for LegacyOnly {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = request.body_json().unwrap();
        let tools = body.get("tools").is_some();
        self.0.lock().unwrap().push(body);
        if tools {
            return ResponseTemplate::new(422)
                .set_body_json(json!({"error":{"message":"tools unsupported"}}));
        }
        ResponseTemplate::new(200).insert_header("content-type","text/event-stream")
            .set_body_string("data: {\"choices\":[{\"delta\":{\"content\":\"Generated answer.\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
    }
}

async fn exercise(deny: bool, disconnect: bool) -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![])).await? else {
        return Ok(());
    };
    f._server.reset().await;
    let model = LegacyOnly::default();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(model.clone())
        .mount(&f._server)
        .await;
    if deny {
        f.reject_answer_saves().await?;
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let sse = if disconnect {
            let id =
                utopia_store::conversations::create(&f.pool, f.kb, f.user.id, "disconnect").await?;
            let response = chat(
                State(f.state.clone()),
                AuthUser(f.user.clone()),
                Path(f.kb),
                Json(ChatReq {
                    conversation_id: Some(id),
                    message: "hello".into(),
                    retry_message_id: None,
                }),
            )
            .await
            .map_err(|_| anyhow::anyhow!("chat refused"))?;
            let (_, mut rx) = f
                .state
                .live
                .attach(id)
                .await
                .ok_or_else(|| anyhow::anyhow!("producer ended before attachment"))?;
            drop(response);
            let mut frames = String::new();
            loop {
                match rx.recv().await {
                    Ok(frame) => frames
                        .push_str(&format!("event: {}\ndata: {}\n\n", frame.event, frame.data)),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(e) => return Err(e.into()),
                }
            }
            anyhow::ensure!(f.state.live.attach(id).await.is_none());
            frames
        } else {
            f.ask("hello").await?
        };
        if deny {
            anyhow::ensure!(
                sse.contains("event: error") && !sse.contains("event: done"),
                "{sse}"
            );
            anyhow::ensure!(sse.contains("Could not confirm that the answer was saved."));
            anyhow::ensure!(!sse.contains("private persistence diagnostic"));
            anyhow::ensure!(f.stored_answer().await?.is_none());
        } else {
            anyhow::ensure!(sse.contains("event: done") && !sse.contains("event: error"));
            anyhow::ensure!(f.stored_answer().await?.as_deref() == Some("Generated answer."));
        }
        anyhow::ensure!(
            model.0.lock().unwrap().len() == 3,
            "compatibility negotiation plus exactly one answer, no save retry"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    f.cleanup().await?;
    result??;
    Ok(())
}
#[tokio::test]
async fn failed_fallback_save_never_reports_done() -> anyhow::Result<()> {
    exercise(true, false).await
}
#[tokio::test]
async fn failed_fallback_save_after_disconnect_cleans_up() -> anyhow::Result<()> {
    exercise(true, true).await
}
#[tokio::test]
async fn committed_fallback_answer_is_readable_at_done() -> anyhow::Result<()> {
    exercise(false, false).await
}
