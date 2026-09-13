//! `LlmClient` 站到 rig 的 `CompletionModel` 后面（#546）。
//!
//! **循环与钩子是 rig 的，线上的字节是我们的。** `LlmClient` 里攒着的东西都是
//! 事故换来的：读超时（7,459 块的摄取死在第 55 块上没有一条错误）、错误体转发
//! （#538）、欠费与限流的分类、缓存 token 的日志。换成 rig 自带的 provider
//! client 这些全都会丢。所以这里只做两个方向的翻译：rig 的请求 → OpenAI 协议的
//! JSON；`LlmClient` 的回合与流 → rig 的回答与流。

use futures_util::{stream, Stream, StreamExt};
use rig_core::completion::{
    CompletionError, CompletionModel, CompletionRequest, CompletionResponse, Usage,
};
use rig_core::message::{AssistantContent, Message, ToolChoice, ToolResultContent, UserContent};
use rig_core::streaming::{
    RawStreamingChoice, RawStreamingToolCall, StreamFinal, StreamingCompletionResponse,
};
use serde_json::{json, Value};
use utopia_llm::{AssistantTurn, LlmClient, ToolStreamItem};

/// rig 要给每个回答挂一个「谁回的」；这里没有厂商之分，全是 `LlmClient`
const PROVIDER: &str = "utopia";

#[derive(Clone)]
pub struct RigModel {
    client: LlmClient,
}

impl RigModel {
    pub fn new(client: LlmClient) -> Self {
        Self { client }
    }

    /// 开一条流。**端点不接受 `tool_choice` 时去掉它重发一次**：`required` 不在
    /// 每家的协议里，而循环那头拿到 400 会读成「这家不支持工具调用」整个降级——
    /// 那是两件事。去掉之后模型仍可能只说不查，那一层由 turn-finished 钩子兜。
    async fn open_stream(
        &self,
        w: &Wire,
    ) -> Result<impl Stream<Item = anyhow::Result<ToolStreamItem>> + Send + use<>, CompletionError>
    {
        match self
            .client
            .chat_tools_stream_with(&w.messages, w.tools.as_ref(), w.tool_choice.as_ref())
            .await
        {
            Ok(s) => Ok(s),
            Err(e) if w.tool_choice.is_some() && rejected_shape(&e) => {
                tracing::warn!(error = %e, "端点不接受 tool_choice，去掉重发");
                self.client
                    .chat_tools_stream_with(&w.messages, w.tools.as_ref(), None)
                    .await
                    .map_err(completion_error)
            }
            Err(e) => Err(completion_error(e)),
        }
    }
}

impl CompletionModel for RigModel {
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        let w = wire(&request);
        let turn = match self
            .client
            .chat_tools_with(&w.messages, w.tools.as_ref(), w.tool_choice.as_ref())
            .await
        {
            Ok(t) => t,
            Err(e) if w.tool_choice.is_some() && rejected_shape(&e) => {
                tracing::warn!(error = %e, "端点不接受 tool_choice，去掉重发");
                self.client
                    .chat_tools_with(&w.messages, w.tools.as_ref(), None)
                    .await
                    .map_err(completion_error)?
            }
            Err(e) => return Err(completion_error(e)),
        };
        Ok(CompletionResponse::new(
            choice_of(&turn),
            Usage::new(),
            PROVIDER,
        ))
    }

    async fn stream(
        &self,
        request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse, CompletionError> {
        let w = wire(&request);
        let source = self.open_stream(&w).await?;
        // `LlmClient` 的流已经把工具调用按 index 归并好、放在流末的 `Turn` 里；
        // rig 只要每个调用一条 `ToolCall`，再一条 `FinalResponse` 收尾
        let inner = source.flat_map(|item| {
            let items: Vec<Result<RawStreamingChoice, CompletionError>> = match item {
                Ok(ToolStreamItem::Delta(text)) => vec![Ok(RawStreamingChoice::Message(text))],
                Ok(ToolStreamItem::Turn(turn)) => {
                    let mut v: Vec<_> = turn
                        .tool_calls
                        .iter()
                        .map(|c| {
                            Ok(RawStreamingChoice::ToolCall(RawStreamingToolCall::new(
                                call_id(&c.id),
                                c.name.clone(),
                                args_value(&c.arguments),
                            )))
                        })
                        .collect();
                    v.push(Ok(RawStreamingChoice::FinalResponse(StreamFinal::new(
                        PROVIDER,
                        Usage::new(),
                    ))));
                    v
                }
                Err(e) => vec![Err(completion_error(e))],
            };
            stream::iter(items)
        });
        Ok(StreamingCompletionResponse::stream(
            PROVIDER,
            Box::pin(inner),
        ))
    }
}

// ---- 错误：整条 anyhow 链穿过 rig -------------------------------------------

/// `LlmClient` 的错误穿过 rig 的 `CompletionError`。
///
/// rig 的错误枚举里只有 `RequestError(Box<dyn Error>)` 能装外来的东西，而循环
/// 那头要认出限流、欠费、被拒（只在「端点拒绝工具调用」时降级），所以整条
/// anyhow 链原样装进去，那头 downcast 回来。
#[derive(Debug)]
pub struct LlmFailure(pub anyhow::Error);

impl std::fmt::Display for LlmFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for LlmFailure {}

fn completion_error(err: anyhow::Error) -> CompletionError {
    CompletionError::RequestError(Box::new(LlmFailure(err)))
}

/// rig 错误里我们自己的那条链，拿不到（rig 自己的错误）就是 None
pub fn llm_failure(err: &CompletionError) -> Option<&anyhow::Error> {
    match err {
        CompletionError::RequestError(inner) => inner.downcast_ref::<LlmFailure>().map(|f| &f.0),
        _ => None,
    }
}

/// 端点拒绝了请求的形状（400/422）。带工具的首个请求撞上它，才是「这家不支持
/// 工具调用」；网断、密钥错、限流、欠费都不是，降级也救不了它们
fn rejected_shape(err: &anyhow::Error) -> bool {
    utopia_llm::rejected(err).is_some_and(|r| r.status == 400 || r.status == 422)
}

/// 见 [`rejected_shape`]，从 rig 的错误上判
pub fn tool_calling_rejected(err: &CompletionError) -> bool {
    llm_failure(err).is_some_and(rejected_shape)
}

// ---- rig → OpenAI 协议 ---------------------------------------------------------

/// 一次请求在线上的三样：消息、工具清单、`tool_choice`
pub(crate) struct Wire {
    pub messages: Vec<Value>,
    /// None = 不带工具字段（`ToolChoice::None` 或本来就没有工具）
    pub tools: Option<Value>,
    pub tool_choice: Option<Value>,
}

/// rig 的请求摊成 OpenAI 协议。
///
/// - `preamble` 是第一条 system。
/// - `documents`（rig 的「上下文文档」，我们放已认下的实体清单）是一条 system，
///   **插在最后一条 user 消息之前**：位置就是服从性（紧挨当前问题），而角色不再是
///   user——从前它假扮成用户发言，模型会回复它（「看起来你已经获得了实体 ID…」）。
/// - `ToolChoice::None` 直接不带工具：`"tool_choice": "none"` 不是每家都认，
///   没有工具字段则人人都认，效果一样。
pub(crate) fn wire(req: &CompletionRequest) -> Wire {
    let mut messages = Vec::new();
    if let Some(p) = &req.preamble {
        messages.push(json!({ "role": "system", "content": p }));
    }
    for m in &req.chat_history {
        push_message(&mut messages, m);
    }
    if !req.documents.is_empty() {
        let text = req
            .documents
            .iter()
            .map(|d| d.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let at = messages
            .iter()
            .rposition(|m| m["role"] == "user")
            .unwrap_or(messages.len());
        messages.insert(at, json!({ "role": "system", "content": text }));
    }
    let tools_off = req.tools.is_empty() || matches!(req.tool_choice, Some(ToolChoice::None));
    let tools = (!tools_off).then(|| {
        Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect(),
        )
    });
    let tool_choice = if tools_off {
        None
    } else {
        req.tool_choice.as_ref().and_then(tool_choice_json)
    };
    Wire {
        messages,
        tools,
        tool_choice,
    }
}

fn tool_choice_json(choice: &ToolChoice) -> Option<Value> {
    match choice {
        // 默认值不写：写了反而有端点不认
        ToolChoice::Auto => None,
        ToolChoice::None => Some(json!("none")),
        ToolChoice::Required => Some(json!("required")),
        ToolChoice::Specific { function_names } => match function_names.as_slice() {
            [one] => Some(json!({ "type": "function", "function": { "name": one } })),
            // OpenAI 协议点不了「这几个之一」，退成「必须调一个」
            _ => Some(json!("required")),
        },
    }
}

fn push_message(out: &mut Vec<Value>, m: &Message) {
    match m {
        Message::System { content } => out.push(json!({ "role": "system", "content": content })),
        Message::User { content } => {
            let mut text = String::new();
            for c in content {
                match c {
                    UserContent::Text(t) => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&t.text);
                    }
                    // tool 消息要紧跟带 tool_calls 的 assistant 消息，所以先于正文
                    UserContent::ToolResult(r) => out.push(json!({
                        "role": "tool",
                        "tool_call_id": r.call.as_str(),
                        "content": tool_result_text(&r.content),
                    })),
                    // 图片、音频、附件：这条线上没有
                    _ => {}
                }
            }
            if !text.is_empty() {
                out.push(json!({ "role": "user", "content": text }));
            }
        }
        Message::Assistant { content, .. } => {
            let mut text = String::new();
            let mut calls = Vec::new();
            for c in content {
                match c {
                    AssistantContent::Text(t) => text.push_str(&t.text),
                    AssistantContent::ToolCall(tc) => calls.push(json!({
                        "id": tc.id.as_str(),
                        "type": "function",
                        "function": {
                            "name": tc.function.name,
                            "arguments": args_string(&tc.function.arguments),
                        }
                    })),
                    // 推理块与图片不回灌
                    _ => {}
                }
            }
            let mut msg = json!({
                "role": "assistant",
                "content": if text.is_empty() { Value::Null } else { Value::String(text) },
            });
            if !calls.is_empty() {
                msg["tool_calls"] = Value::Array(calls);
            }
            out.push(msg);
        }
    }
}

/// 工具结果的正文：文本原样，结构化 JSON 序列化
pub(crate) fn tool_result_text(content: &[ToolResultContent]) -> String {
    let mut out = String::new();
    for c in content {
        match c {
            ToolResultContent::Text(t) => out.push_str(&t.text),
            ToolResultContent::Json { value, .. } => out.push_str(&value.to_string()),
            _ => {}
        }
    }
    out
}

// ---- OpenAI 协议 → rig ---------------------------------------------------------

/// 参数串 → JSON。**解不开的不补成空对象**：模型撞上 token 上限时参数会在半路
/// 断掉，补成 `{}` 就变成一次拿用户原话去检索的「正常调用」（见 `check_call`）。
/// 原串装进一个 JSON 字符串，到了 `check_call` 那里它不是对象，照样拒绝。
pub(crate) fn args_value(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

/// [`args_value`] 的反向：原本就是一段残串的，原样还回去
pub(crate) fn args_string(v: &Value) -> String {
    match v {
        Value::String(raw) => raw.clone(),
        other => other.to_string(),
    }
}

/// 有的端点（本地 Ollama 一类）不给 tool call id。rig 要一个，协议回灌时也要
/// 一个，所以补一个；端点自己没发过，回去时它也不会对不上
pub(crate) fn call_id(id: &str) -> String {
    if id.is_empty() {
        format!("call_{}", uuid::Uuid::now_v7().simple())
    } else {
        id.to_string()
    }
}

fn choice_of(turn: &AssistantTurn) -> Vec<AssistantContent> {
    let mut out = Vec::new();
    if let Some(text) = turn.content.as_deref().filter(|t| !t.is_empty()) {
        out.push(AssistantContent::text(text));
    }
    for c in &turn.tool_calls {
        out.push(AssistantContent::tool_call(
            call_id(&c.id),
            c.name.clone(),
            args_value(&c.arguments),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rig_core::completion::{Document, ToolDefinition};

    fn req(history: Vec<Message>) -> CompletionRequest {
        CompletionRequest {
            model: None,
            preamble: Some("be brief".into()),
            chat_history: history,
            documents: vec![],
            tools: vec![ToolDefinition {
                name: "search_chunks".into(),
                description: "search".into(),
                parameters: json!({ "type": "object" }),
            }],
            temperature: None,
            max_tokens: None,
            tool_choice: None,
            additional_params: None,
            output_schema: None,
            record_telemetry_content: false,
        }
    }

    #[test]
    fn preamble_leads_and_tools_follow_the_protocol() {
        let w = wire(&req(vec![Message::user("hi")]));
        assert_eq!(w.messages[0]["role"], "system");
        assert_eq!(w.messages[0]["content"], "be brief");
        assert_eq!(w.messages[1], json!({ "role": "user", "content": "hi" }));
        let tools = w.tools.expect("工具在");
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "search_chunks");
        assert!(w.tool_choice.is_none(), "没说就不写 tool_choice");
    }

    /// 实体清单从前假扮成 user 发言，模型会回复它。现在是 system，且紧挨当前问题
    #[test]
    fn documents_become_a_system_message_right_before_the_question() {
        let mut r = req(vec![
            Message::user("first"),
            Message::assistant("a1"),
            Message::user("second"),
        ]);
        r.documents.push(Document {
            id: "known".into(),
            text: "Entities already identified".into(),
            additional_props: Default::default(),
        });
        let w = wire(&r);
        let roles: Vec<_> = w
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["system", "user", "assistant", "system", "user"]);
        assert_eq!(w.messages[3]["content"], "Entities already identified");
        assert_eq!(w.messages[4]["content"], "second");
    }

    /// 工具往返回灌：assistant 带 tool_calls（参数是字符串），tool 消息紧随其后
    #[test]
    fn a_tool_exchange_round_trips_in_protocol_shape() {
        let w = wire(&req(vec![
            Message::user("q"),
            Message::Assistant {
                id: None,
                content: vec![
                    AssistantContent::text("looking"),
                    AssistantContent::tool_call("c1", "search_chunks", json!({ "query": "x" })),
                ],
            },
            Message::tool_result("c1", "search_chunks", "found"),
        ]));
        assert_eq!(w.messages[2]["role"], "assistant");
        assert_eq!(w.messages[2]["content"], "looking");
        assert_eq!(w.messages[2]["tool_calls"][0]["id"], "c1");
        assert_eq!(
            w.messages[2]["tool_calls"][0]["function"]["arguments"],
            "{\"query\":\"x\"}"
        );
        assert_eq!(
            w.messages[3],
            json!({ "role": "tool", "tool_call_id": "c1", "content": "found" })
        );
    }

    #[test]
    fn tool_choice_maps_and_none_drops_the_tools_entirely() {
        let mut r = req(vec![Message::user("q")]);
        r.tool_choice = Some(ToolChoice::Required);
        assert_eq!(wire(&r).tool_choice, Some(json!("required")));
        r.tool_choice = Some(ToolChoice::Specific {
            function_names: vec!["search_chunks".into()],
        });
        assert_eq!(
            wire(&r).tool_choice,
            Some(json!({ "type": "function", "function": { "name": "search_chunks" } }))
        );
        r.tool_choice = Some(ToolChoice::None);
        let w = wire(&r);
        assert!(w.tools.is_none(), "None = 请求里根本没有工具字段");
        assert!(w.tool_choice.is_none());
    }

    /// 半路断掉的参数不能变成空对象（那会变成一次拿原话检索的正常调用）
    #[test]
    fn cut_off_arguments_stay_a_string_and_come_back_verbatim() {
        let v = args_value("{\"query\": \"Acme reven");
        assert_eq!(v, Value::String("{\"query\": \"Acme reven".into()));
        assert_eq!(args_string(&v), "{\"query\": \"Acme reven");
        assert_eq!(args_string(&json!({ "a": 1 })), "{\"a\":1}");
    }

    #[test]
    fn a_missing_call_id_is_minted_and_a_given_one_kept() {
        assert_eq!(call_id("call_abc"), "call_abc");
        let minted = call_id("");
        assert!(minted.starts_with("call_") && minted.len() > 10);
    }

    #[test]
    fn a_turn_becomes_text_then_tool_calls() {
        let turn = AssistantTurn {
            content: Some("hm".into()),
            tool_calls: vec![utopia_llm::ToolCall {
                id: "c9".into(),
                name: "find_entities".into(),
                arguments: "{\"name\":\"Acme\"}".into(),
            }],
        };
        let choice = choice_of(&turn);
        assert_eq!(choice.len(), 2);
        assert!(matches!(&choice[0], AssistantContent::Text(t) if t.text == "hm"));
        match &choice[1] {
            AssistantContent::ToolCall(tc) => {
                assert_eq!(tc.id.as_str(), "c9");
                assert_eq!(tc.function.name, "find_entities");
                assert_eq!(tc.function.arguments["name"], "Acme");
            }
            other => panic!("expected a tool call, got {other:?}"),
        }
    }
}
