//! 对话循环交给 rig 的 runner（#546）。
//!
//! 从前 `chat.rs` 里手写着一个 1,100 行的循环，每个策略问题都落成它里面的又一个
//! 分支；#509（模型说「我去查」然后就结束了一轮）的守卫（#543）是启发式上再叠
//! 启发式——实测追问四次，模型四次都答 DONE。这里把循环换成 rig 的：工具是
//! `DynamicTool`，策略是钩子，终止是**结构性的**——
//!
//! **一轮对话在调过至少一个工具之前不能结束。** 首个请求带 `tool_choice: required`，
//! 直到某个工具跑过为止；`no_evidence_needed` 是给「你好」「说短一点」这类不需要
//! 库的问题准备的那个工具。于是「请稍等，我将调用工具」不再是一个可能的终态：
//! 模型要么真的调了，要么明说这题不用查。
//!
//! 端点不认 `required`（400 时 `RigModel` 去掉它重发；或者干脆无视）的那一层，
//! 由 `on_model_turn_finished` 兜一次：首轮只有文字、一个工具没跑，退回去要求它调；
//! 再不调就认了——那是端点的事，日志里能看见。

use super::tools::{self, ToolCtx, ToolSink};
use crate::state::AppState;
use rig_agent::agent::{
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, ModelTurnAction,
    ModelTurnFinished, RequestPatch, RetryRequest, ToolCall as ToolCallEvent, ToolCallAction,
    ToolResultAction, ToolResultEvent,
};
use rig_agent::tool::{DynamicTool, ToolContext, ToolOutput};
use rig_core::message::{AssistantContent, Message, ToolChoice};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use utopia_core::models::DataSourceView;
use uuid::Uuid;

/// 「这题不用查」的那个工具。它存在是为了让首轮的 `required` 有一个诚实的出口：
/// 不是每个问题都关于库里的数据，而强迫模型对「你好」跑一次检索是另一种错
pub const NO_EVIDENCE_TOOL: &str = "no_evidence_needed";

const NO_EVIDENCE_DESCRIPTION: &str = "Declare that this question does not need the knowledge \
    base: a greeting, a question about this conversation itself, or a request to reword or \
    shorten a previous answer. Never use it for a question about facts, entities, documents, \
    dates or data; for those, call the tool that gathers the evidence. After calling it, answer \
    directly.";

/// 端点无视了 `required`、首轮只有文字时退回去的那句话。**没有 DONE 出口**：
/// 不查的正当理由只有一个，就是调 `no_evidence_needed`
const MUST_CALL: &str = "(system) Every turn starts with a tool call. Call the tool that gathers \
    the evidence for this question, or call no_evidence_needed if the question is not about the \
    knowledge base at all. Do not describe a plan.";

/// 弹药耗尽那一轮的系统提示补语；工具同时被撤走，模型只能作答
const BUDGET_EXHAUSTED: &str =
    "\n\n(system) Tool budget exhausted. Answer now from the evidence gathered above.";

/// 一场对话里工具共用的东西：库、权限、引用清单，以及给界面的轨迹。
///
/// rig 并发跑同一轮的多个工具，而引用编号是有状态的（`[3]` 取决于之前引过几个），
/// 所以 `sink` 是一把异步锁，一个工具跑完另一个再进
pub struct Shared {
    pub state: AppState,
    pub kb_id: Uuid,
    pub workspace_id: Uuid,
    pub mounted_sources: Vec<DataSourceView>,
    pub can_write: bool,
    pub actor: Uuid,
    /// 工具清单（与 MCP 共用的那一份），`check_call` 的判据从这里取
    pub schema: Value,
    /// 日志里写模型名，好按模型统计
    pub model: String,
    /// 这一轮用户问的那句话。工具拿它给同名实体排序：「张伟的雇主」和「张伟的论文」
    /// 该落到不同的张伟上（dev 上的手写循环早就传了；MCP 那边没有这句话，传 None）
    pub question: String,
    pub sink: tokio::sync::Mutex<ToolSink>,
    /// 工具跑完留给界面的一步，按 rig 的 internal_call_id 取；
    /// `check_call` 拒掉的调用也在这里留一步
    steps: Mutex<HashMap<String, Value>>,
    /// 任何工具（含 `no_evidence_needed`）跑过一次：`required` 的闸门就过了
    gate_passed: AtomicBool,
    /// 端点无视 `required` 时的退回只给一次
    nudged: AtomicBool,
}

impl Shared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        state: AppState,
        kb_id: Uuid,
        workspace_id: Uuid,
        mounted_sources: Vec<DataSourceView>,
        can_write: bool,
        actor: Uuid,
        schema: Value,
        model: String,
        question: String,
    ) -> Arc<Self> {
        Arc::new(Self {
            state,
            kb_id,
            workspace_id,
            mounted_sources,
            can_write,
            actor,
            schema,
            model,
            question,
            sink: tokio::sync::Mutex::new(ToolSink::default()),
            steps: Mutex::new(HashMap::new()),
            gate_passed: AtomicBool::new(false),
            nudged: AtomicBool::new(false),
        })
    }

    fn keep_step(&self, internal_call_id: &str, step: Value) {
        self.steps
            .lock()
            .expect("steps lock")
            .insert(internal_call_id.to_string(), step);
    }

    /// 取走这次调用留给界面的那一步（没有 = 未知工具，或不留痕的闸门工具）
    pub fn take_step(&self, internal_call_id: &str) -> Option<Value> {
        self.steps
            .lock()
            .expect("steps lock")
            .remove(internal_call_id)
    }

    fn tool_ctx(&self) -> ToolCtx<'_> {
        ToolCtx {
            state: &self.state,
            kb_id: self.kb_id,
            workspace_id: self.workspace_id,
            mounted_sources: &self.mounted_sources,
            can_write: self.can_write,
            actor: Some(self.actor),
            // 网页端对话不经令牌：说话的就是这个人本人
            via_token: None,
            question: Some(&self.question),
        }
    }
}

/// 工具跑完留在 rig 工具上下文里的那一步，`on_tool_result` 从那里取
#[derive(Clone)]
struct Step(Value);

/// 工具清单变成 rig 的动态工具：名字、描述、参数 schema 都来自 `tools_schema`，
/// 执行还是 `tools::dispatch`。**清单是唯一的真相**，这里不抄第二份
pub fn dynamic_tools(shared: &Arc<Shared>) -> Vec<DynamicTool> {
    let mut out = Vec::new();
    for t in shared.schema.as_array().into_iter().flatten() {
        let f = &t["function"];
        let Some(name) = f["name"].as_str() else {
            continue;
        };
        let owned = shared.clone();
        let tool_name = name.to_string();
        out.push(DynamicTool::new(
            name,
            f["description"].as_str().unwrap_or_default(),
            f["parameters"].clone(),
            move |ctx: &mut ToolContext, args: Value| {
                let shared = owned.clone();
                let name = tool_name.clone();
                Box::pin(async move {
                    let tool_ctx = shared.tool_ctx();
                    // dispatch 现在回一个结构体（#601 给 MCP 加了 structuredContent 与
                    // is_error）。网页端对话只要正文与界面那一步，与 dev 上手写循环取的一样
                    let tools::ToolResult {
                        text: result, step, ..
                    } = {
                        let mut sink = shared.sink.lock().await;
                        tools::dispatch(&tool_ctx, &mut sink, &name, &args).await
                    };
                    shared.gate_passed.store(true, Ordering::Relaxed);
                    ctx.insert_result(Step(step));
                    Ok(ToolOutput::text(result))
                })
            },
        ));
    }
    let owned = shared.clone();
    out.push(DynamicTool::new(
        NO_EVIDENCE_TOOL,
        NO_EVIDENCE_DESCRIPTION,
        json!({
            "type": "object",
            "properties": {
                "reason": {
                    "type": "string",
                    "description": "One short phrase: why no evidence is needed."
                }
            },
            "required": ["reason"]
        }),
        move |_ctx: &mut ToolContext, args: Value| {
            let shared = owned.clone();
            Box::pin(async move {
                tracing::info!(
                    model = shared.model,
                    reason = args["reason"].as_str().unwrap_or(""),
                    "模型声明这题不用查"
                );
                shared.gate_passed.store(true, Ordering::Relaxed);
                Ok(ToolOutput::text(
                    "Understood. Answer the user directly now.",
                ))
            })
        },
    ));
    out
}

/// 循环的策略，作为 rig 的钩子。
#[derive(Clone)]
pub struct Policy {
    pub shared: Arc<Shared>,
    /// 系统提示原文：弹药耗尽那一轮要在它后面补一句
    pub preamble: String,
    /// 允许的工具轮数；第 `max_rounds + 1` 次请求撤走工具、命令作答
    pub max_rounds: usize,
}

impl AgentHook for Policy {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> impl std::future::Future<Output = CompletionCallAction> + Send {
        let turn = event.turn;
        let action = if turn > self.max_rounds {
            // 弹药耗尽：撤走工具（`RigModel` 对 None 的处理是根本不带工具字段），
            // 系统提示末尾命令它就现有证据作答
            CompletionCallAction::Patch(
                RequestPatch::new()
                    .tool_choice(ToolChoice::None)
                    .preamble(format!("{}{BUDGET_EXHAUSTED}", self.preamble)),
            )
        } else if !self.shared.gate_passed.load(Ordering::Relaxed) {
            // 一个工具都还没跑：这一轮必须调一个
            CompletionCallAction::Patch(RequestPatch::new().tool_choice(ToolChoice::Required))
        } else {
            CompletionCallAction::Continue
        };
        async move { action }
    }

    fn on_model_turn_finished(
        &self,
        _ctx: &HookContext,
        event: ModelTurnFinished<'_>,
    ) -> impl std::future::Future<Output = ModelTurnAction> + Send {
        let has_tool_call = event
            .content
            .iter()
            .any(|c| matches!(c, AssistantContent::ToolCall(_)));
        let turn = event.turn;
        let shared = self.shared.clone();
        let max_rounds = self.max_rounds;
        async move {
            if has_tool_call || shared.gate_passed.load(Ordering::Relaxed) || turn > max_rounds {
                return ModelTurnAction::Continue;
            }
            // 只有文字、闸门没过：端点无视了 `required`。退回去一次
            if !shared.nudged.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    model = shared.model,
                    turn,
                    "首轮没有工具调用（端点未执行 tool_choice=required），退回要求调用"
                );
                return ModelTurnAction::Retry(RetryRequest::Feedback(MUST_CALL.into()));
            }
            tracing::warn!(
                model = shared.model,
                turn,
                "退回一次后仍无工具调用，按原文收尾（sources 为空）"
            );
            ModelTurnAction::Continue
        }
    }

    fn on_tool_call(
        &self,
        _ctx: &HookContext,
        event: ToolCallEvent<'_>,
    ) -> impl std::future::Future<Output = ToolCallAction> + Send {
        // **说不清自己要做什么的调用不执行。** 把话回给模型，让它重来；
        // 界面上照样显示成一次没做成的调用
        let action = match super::chat::check_call(&self.shared.schema, event.tool_name, event.args)
        {
            Ok(_) => ToolCallAction::Run,
            Err((message, step)) => {
                self.shared.keep_step(event.internal_call_id, step);
                ToolCallAction::Skip(message)
            }
        };
        async move { action }
    }

    fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> impl std::future::Future<Output = ToolResultAction> + Send {
        if let Some(Step(step)) = event.tool_context.result::<Step>() {
            self.shared.keep_step(event.internal_call_id, step.clone());
        }
        async { ToolResultAction::Keep }
    }
}

/// 落库的历史（OpenAI 协议的 JSON）变回 rig 的消息。
///
/// 最后那一轮的工具往返放在它的结论之前——顺序就是真实顺序，模型读起来
/// 是「我问了、我查了、我答了」。tool 消息的工具名在协议里没有，从前面那条
/// 带 tool_calls 的 assistant 消息里按 id 找回来
pub fn history_messages(turns: &[(String, String)], last_tool_exchange: &[Value]) -> Vec<Message> {
    let last_assistant = turns.iter().rposition(|(role, _)| role == "assistant");
    let mut out = Vec::new();
    for (i, (role, content)) in turns.iter().enumerate() {
        if Some(i) == last_assistant {
            out.extend(exchange_messages(last_tool_exchange));
        }
        match role.as_str() {
            "assistant" => out.push(Message::assistant(content.clone())),
            _ => out.push(Message::user(content.clone())),
        }
    }
    out
}

fn exchange_messages(exchange: &[Value]) -> Vec<Message> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for m in exchange {
        match m["role"].as_str() {
            Some("assistant") => {
                let mut content = Vec::new();
                if let Some(text) = m["content"].as_str().filter(|t| !t.is_empty()) {
                    content.push(AssistantContent::text(text));
                }
                for c in m["tool_calls"].as_array().into_iter().flatten() {
                    let id = c["id"].as_str().unwrap_or_default();
                    let name = c["function"]["name"].as_str().unwrap_or_default();
                    names.insert(id.to_string(), name.to_string());
                    content.push(AssistantContent::tool_call(
                        id,
                        name,
                        super::rig_model::args_value(
                            c["function"]["arguments"].as_str().unwrap_or("{}"),
                        ),
                    ));
                }
                if !content.is_empty() {
                    out.push(Message::Assistant { id: None, content });
                }
            }
            Some("tool") => {
                let id = m["tool_call_id"].as_str().unwrap_or_default();
                let name = names.get(id).cloned().unwrap_or_default();
                out.push(Message::tool_result(
                    id,
                    name,
                    m["content"].as_str().unwrap_or_default(),
                ));
            }
            _ => {}
        }
    }
    out
}

/// 前几轮已经认下的实体，连 id 一起交回去（作为本轮的上下文文档，见 `rig_model::wire`
/// 里它落在哪、以什么角色）。少了它，模型只看得见上一轮的最终答案文字，不知道
/// 自己拿到过哪些 id，于是从名字重搜一遍；同名歧义时两轮可能落到不同的实体上
pub fn known_entities_block(entities: &[Value], limit: usize) -> Option<String> {
    if entities.is_empty() {
        return None;
    }
    let lines: Vec<String> = entities
        .iter()
        .take(limit)
        .map(|e| {
            format!(
                "{} | {} | {}",
                e["id"].as_str().unwrap_or("?"),
                e["name"].as_str().unwrap_or("?"),
                e["type"].as_str().unwrap_or("?")
            )
        })
        .collect();
    Some(format!(
        "Entities already identified earlier in this conversation (id | name | type). \
         Call entity_facts with these ids directly; do not look them up by name again:\n{}",
        lines.join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上一轮的工具往返插在它的结论之前，tool 消息找回自己的工具名
    #[test]
    fn the_last_exchange_sits_before_its_conclusion() {
        let turns = vec![
            ("user".to_string(), "q1".to_string()),
            ("assistant".to_string(), "a1".to_string()),
            ("user".to_string(), "q2".to_string()),
        ];
        let exchange = vec![
            json!({ "role": "assistant", "content": null, "tool_calls": [
                { "id": "c1", "type": "function",
                  "function": { "name": "search_chunks", "arguments": "{\"query\":\"x\"}" } }
            ]}),
            json!({ "role": "tool", "tool_call_id": "c1", "content": "found" }),
        ];
        let msgs = history_messages(&turns, &exchange);
        assert_eq!(msgs.len(), 5);
        assert!(matches!(&msgs[0], Message::User { .. }));
        match &msgs[1] {
            Message::Assistant { content, .. } => match &content[0] {
                AssistantContent::ToolCall(tc) => {
                    assert_eq!(tc.id.as_str(), "c1");
                    assert_eq!(tc.function.arguments["query"], "x");
                }
                other => panic!("expected the tool call, got {other:?}"),
            },
            other => panic!("expected the exchange first, got {other:?}"),
        }
        match &msgs[2] {
            Message::User { content } => match &content[0] {
                rig_core::message::UserContent::ToolResult(r) => {
                    assert_eq!(r.call.as_str(), "c1");
                    assert_eq!(r.name, "search_chunks");
                }
                other => panic!("expected the tool result, got {other:?}"),
            },
            other => panic!("expected the tool result, got {other:?}"),
        }
        assert!(matches!(&msgs[3], Message::Assistant { .. }));
        assert!(matches!(&msgs[4], Message::User { .. }));
    }

    #[test]
    fn the_entity_block_lists_id_name_type_and_stops_at_the_limit() {
        assert!(known_entities_block(&[], 20).is_none());
        let entities = vec![
            json!({ "id": "e1", "name": "Acme", "type": "Organization" }),
            json!({ "id": "e2", "name": "Bob", "type": "Person" }),
        ];
        let block = known_entities_block(&entities, 1).unwrap();
        assert!(block.contains("e1 | Acme | Organization"));
        assert!(!block.contains("e2"));
        assert!(block.starts_with("Entities already identified"));
    }
}
