//! 驱动可取消的生成，再不受取消打断地保存唯一结果。
use super::{carried_sources, delta_event, error_event, Failure};
use crate::live::{Frame, Handle};
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use utopia_store::conversations::{append_message, TurnRecord};
use uuid::Uuid;

pub(super) enum ProducerEvent {
    Progress(Frame),
    /// 仅供历史使用，不通过 SSE 发送；停止时只保留已完成的调用。
    Context {
        resolved: Vec<Value>,
        tool_exchange: Vec<Value>,
        gathered: bool,
    },
    Outcome(Result<Answer, Failure>),
}

pub(super) struct Answer {
    pub content: String,
    pub record: TurnRecord,
    /// 验证过的最终回答只有保存成功后才发布。
    pub final_text: Option<String>,
}

pub(super) async fn run(
    pool: sqlx::PgPool,
    conversation_id: Uuid,
    handle: Handle,
    producer: impl Stream<Item = ProducerEvent>,
    previous_answer: String,
    previous_sources: Vec<Value>,
) {
    // collect 持有模型和工具的 future，在持久化前将它们丢弃。
    // 迟到的 Stop 不能打断提交，也不能再创建一条助手消息。
    let result = collect(&handle, producer, &previous_answer, &previous_sources).await;
    let terminal = match result {
        Ok(answer) => save(&pool, conversation_id, &handle, answer).await,
        Err(failure) => error_event(failure.code, &failure.message),
    };
    handle.complete(terminal).await;
}

async fn collect(
    handle: &Handle,
    producer: impl Stream<Item = ProducerEvent>,
    previous_answer: &str,
    previous_sources: &[Value],
) -> Result<Answer, Failure> {
    let mut producer = Box::pin(producer);
    let cancellation = handle.cancellation();
    let mut resolved = Vec::new();
    let mut tool_exchange = Vec::new();
    let mut gathered = false;

    loop {
        let event = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                // 先丢弃生成器，保存部分答案期间不能继续执行模型或工具。
                drop(producer);
                let mut snapshot = handle.snapshot().await;
                if !gathered && snapshot.sources.is_empty() {
                    snapshot.sources = carried_sources(
                        &snapshot.content, previous_answer, previous_sources,
                    );
                }
                return Ok(Answer {
                    content: snapshot.content,
                    record: TurnRecord {
                        stopped: true,
                        steps: json!(snapshot.steps),
                        sources: json!(snapshot.sources),
                        resolved: json!(resolved),
                        tool_exchange: json!(completed_exchange(tool_exchange)),
                    },
                    final_text: None,
                });
            }
            event = producer.next() => event,
        };
        match event {
            Some(ProducerEvent::Progress(frame)) => {
                if matches!(frame.event, "done" | "error") {
                    return Err(Failure::new(
                        "answer_failed",
                        "Producer sent a terminal as progress",
                    ));
                }
                // 旧 RAG 路径即使没搜到结果也会发布 sources。
                gathered |= matches!(frame.event, "step" | "sources");
                handle.emit(frame).await;
            }
            Some(ProducerEvent::Context {
                resolved: entities,
                tool_exchange: exchange,
                gathered: did_gather,
            }) => {
                resolved = entities;
                tool_exchange = exchange;
                gathered |= did_gather;
            }
            Some(ProducerEvent::Outcome(result)) if !cancellation.is_cancelled() => return result,
            Some(ProducerEvent::Outcome(_)) => continue,
            None => {
                return Err(Failure::new(
                    "stream_ended",
                    "Answer stream ended unexpectedly",
                ))
            }
        }
    }
}

async fn save(
    pool: &sqlx::PgPool,
    conversation_id: Uuid,
    handle: &Handle,
    answer: Answer,
) -> Frame {
    let saved = append_message(
        pool,
        conversation_id,
        "assistant",
        &answer.content,
        &answer.record,
    )
    .await;
    if let Err(error) = saved {
        tracing::error!(%error, %conversation_id, "Could not persist chat answer");
        return error_event(
            "answer_not_saved",
            "Could not confirm that the answer was saved.",
        );
    }

    handle
        .emit(Frame::new("sources", answer.record.sources.to_string()))
        .await;
    if let Some(text) = answer.final_text {
        handle.emit(delta_event(&text)).await;
    }
    Frame::new(
        "done",
        json!({"stopped": answer.record.stopped}).to_string(),
    )
}

/// 并行工具可能只完成一部分就被停止。不能回放没有结果的 tool_call：
/// 模型服务要求每个保留的调用都有对应结果。
fn completed_exchange(mut exchange: Vec<Value>) -> Vec<Value> {
    // 模型服务可能在后续轮次复用 ID。倒序遍历，让每条助手消息只匹配
    // 它之后、下一条助手消息之前的工具结果。
    let mut answered = HashSet::new();
    for message in exchange.iter_mut().rev() {
        if message["role"] == "tool" {
            if let Some(id) = message["tool_call_id"].as_str() {
                answered.insert(id.to_owned());
            }
            continue;
        }
        if let Some(calls) = message["tool_calls"].as_array_mut() {
            calls.retain(|call| call["id"].as_str().is_some_and(|id| answered.contains(id)));
        }
        answered.clear();
    }
    exchange.retain(|message| {
        if message["role"] != "assistant" {
            return true;
        }
        message["tool_calls"]
            .as_array()
            .is_some_and(|calls| !calls.is_empty())
    });
    exchange
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stopped_parallel_batch_replays_only_answered_calls() {
        let exchange = vec![
            json!({"role":"assistant", "tool_calls":[{"id":"finished"},{"id":"pending"}]}),
            json!({"role":"tool", "tool_call_id":"finished", "content":"found"}),
            json!({"role":"assistant", "tool_calls":[{"id":"never_started"}]}),
        ];
        let kept = completed_exchange(exchange);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0]["tool_calls"], json!([{"id":"finished"}]));
        assert_eq!(kept[1]["tool_call_id"], "finished");
    }

    #[test]
    fn a_reused_call_id_only_matches_results_in_its_own_round() {
        let exchange = vec![
            json!({"role":"assistant", "tool_calls":[{"id":"reused"}]}),
            json!({"role":"tool", "tool_call_id":"reused", "content":"old result"}),
            json!({"role":"assistant", "tool_calls":[{"id":"reused"},{"id":"finished"}]}),
            json!({"role":"tool", "tool_call_id":"finished", "content":"new result"}),
        ];
        let kept = completed_exchange(exchange);
        assert_eq!(kept.len(), 4);
        assert_eq!(kept[0]["tool_calls"], json!([{"id":"reused"}]));
        assert_eq!(kept[2]["tool_calls"], json!([{"id":"finished"}]));
    }

    #[tokio::test]
    async fn stopping_only_carries_sources_when_this_turn_did_not_gather() {
        let source = json!({"n":1, "document_id":"previous-document"});
        let previous_sources = vec![source.clone()];
        let cases = [
            (None, json!([source])),
            (
                Some(ProducerEvent::Progress(Frame::new("sources", "[]".into()))),
                json!([]),
            ),
            (
                Some(ProducerEvent::Context {
                    resolved: vec![],
                    tool_exchange: vec![],
                    gathered: true,
                }),
                json!([]),
            ),
        ];
        for (before_text, expected_sources) in cases {
            let registry = std::sync::Arc::new(crate::live::Registry::default());
            let handle = registry.begin(Uuid::now_v7()).await.unwrap();
            let cancellation = handle.cancellation();
            let producer = async_stream::stream! {
                if let Some(event) = before_text { yield event; }
                yield ProducerEvent::Progress(delta_event("Target: 95% [1].\n"));
                cancellation.cancel();
                std::future::pending::<()>().await;
            };
            let answer = collect(
                &handle,
                producer,
                "The target is 95% [1].",
                &previous_sources,
            )
            .await
            .unwrap_or_else(|failure| panic!("{}", failure.message));
            assert!(answer.record.stopped);
            assert_eq!(answer.record.sources, expected_sources);
        }
    }
}
