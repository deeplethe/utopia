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
    AgentHook, CompletionCallAction, CompletionCallEvent, HookContext, InvalidToolCallAction,
    InvalidToolCallContext, ModelTurnAction, ModelTurnFinished, RequestPatch, RetryRequest,
    ToolCall as ToolCallEvent, ToolCallAction, ToolResultAction, ToolResultEvent,
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

/// 模型回了一个**空回复**（没有正文、也没调工具）时重问的那句（#631）。
///
/// 空回复不是一个结论，只是没说话。从前它直接变成一帧 `error`：台子上 48 轮里出过
/// 2 次，都在工具已经跑完之后——用户看见三四步查证，接着一句「Model returned an
/// empty answer」，查到的东西全白费。重问一次，第二次还空才报错：只给一次，所以一个
/// 始终不说话的端点不会让循环空转。
///
/// 措辞两头都留门：工具跑过的，就着查到的答；一步没查的（首轮就回空），缺证据
/// 就去调工具。
pub(crate) const EMPTY_REPLY_RETRY: &str = "(system) Your previous reply was empty. Reply to \
    the user now: answer from the evidence gathered above, or call a tool if you still need \
    evidence.";

/// 模型把工具调用写进了正文时退回去的那句（#937）。
///
/// 端点偶尔不走协议里的 `tool_calls`，而是把调用的标记原样写成正文：DeepSeek 的
/// `<｜tool▁call▁begin｜>function…`（台子上对话那一路 35 题里错了 2 题），或者 DSML。
/// 什么都没执行，那段也不是答案。退回时保留那一回合，模型看得见自己想调什么；
/// 只给一次，再写一次就以 [`CONTROL_TEXT`] 报错
pub(crate) const MARKUP_RETRY: &str = "(system) Your previous reply wrote a tool call out as \
    text, so nothing ran. Make the call as a tool call, or answer the user in plain text.";

/// Bounded buffering applies only to the tool-free terminal call. This is a byte
/// limit, independent of the provider's token accounting.
pub(crate) const MAX_FINAL_ANSWER_BYTES: usize = 1024 * 1024;
const FINAL_TOOL_CALL: &str = "Model attempted a tool call after the tool budget was exhausted";
pub(crate) const CONTROL_TEXT: &str = "Model returned tool-control text instead of a final answer";

/// Line starts that open a tool call written as text, as [`control_key`] spells
/// them: DSML, and DeepSeek's native markup (#937).
const CONTROL_MARKERS: [&str; 5] = [
    "<DSMLcalls>",
    "<DSMLtool_calls>",
    "<DSMLinvokename=",
    "<tool▁calls▁begin>",
    "<tool▁call▁begin>",
];

/// Accommodate the known ASCII/full-width and doubled-pipe spellings.
fn control_key(candidate: &str) -> String {
    candidate
        .chars()
        .take(80)
        .filter(|c| !c.is_whitespace() && *c != '|' && *c != '｜')
        .collect()
}

fn is_control(candidate: &str) -> bool {
    let key = control_key(candidate);
    CONTROL_MARKERS.iter().any(|marker| key.starts_with(marker))
}

/// A line still being written that could yet turn out to be a marker.
fn may_become_control(candidate: &str) -> bool {
    let key = control_key(candidate);
    CONTROL_MARKERS
        .iter()
        .any(|marker| marker.starts_with(key.as_str()))
}

/// Whether a reply is a tool call written out as text (#845, #937). Explanations
/// and fenced examples are prose, and an explicit request about the encoding may
/// legitimately ask for the raw markup. This is a publication guard: never
/// interpret this text as an executable tool call.
pub(crate) fn tool_call_text(text: &str, question: &str) -> bool {
    let request = question.to_ascii_lowercase();
    let asks_for_encoding = (request.contains("dsml") || request.contains("tool▁call"))
        && ["example", "verbatim", "示例", "原样"]
            .iter()
            .any(|term| request.contains(term))
        && !request.contains("business")
        && !request.contains("业务");
    if asks_for_encoding {
        return false;
    }
    // Inspect the assembled turn, so SSE chunk boundaries do not matter.
    let text = text.trim();
    if is_control(text) {
        return true;
    }
    // A real endpoint prefixed its final control block with "Let me examine
    // it." in the SAME turn. Check bare line starts too, without treating
    // inline mentions, block quotes, or Markdown fenced examples as calls.
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        let line = line.trim_start();
        if let Some(marker @ ('`' | '~')) = line.chars().next() {
            let len = line.chars().take_while(|c| *c == marker).count();
            if len >= 3 {
                match fence {
                    None => fence = Some((marker, len)),
                    Some((open, size))
                        if marker == open && len >= size && line[len..].trim().is_empty() =>
                    {
                        fence = None;
                    }
                    _ => {}
                }
                continue;
            }
        }
        if fence.is_none() && is_control(line) {
            return true;
        }
    }
    false
}

/// How much of a tool round's text can go out while it is still streaming (#937).
///
/// Text goes out line by line. A line is held while it could still become a
/// tool-call marker; once one has, the rest of the turn is held with it, to be
/// sent back or dropped. `from` is how much has gone out already, and the line
/// it ends in was judged then. Fences are not tracked here: a marker in a fenced
/// example is held until the turn ends, when [`tool_call_text`] releases it.
pub(crate) fn publishable(text: &str, from: usize, holding: &mut bool) -> usize {
    if *holding {
        return from;
    }
    let mut at = if from == 0 || text[..from].ends_with('\n') {
        from
    } else {
        match text[from..].find('\n') {
            Some(i) => from + i + 1,
            None => return text.len(),
        }
    };
    while at < text.len() {
        let rest = &text[at..];
        let (line, finished) = match rest.find('\n') {
            Some(i) => (&rest[..i], true),
            None => (rest, false),
        };
        if is_control(line) {
            *holding = true;
            return at;
        }
        if !finished {
            return if may_become_control(line) {
                at
            } else {
                text.len()
            };
        }
        at += line.len() + 1;
    }
    at
}

/// What keeps a budget-boundary candidate from being published: a structured
/// call, blank text, or a tool call written as text.
pub(crate) fn finalization_error(
    text: &str,
    has_calls: bool,
    question: &str,
) -> Option<&'static str> {
    if has_calls {
        return Some(FINAL_TOOL_CALL);
    }
    if text.trim().is_empty() {
        return Some("Model returned an empty answer");
    }
    tool_call_text(text, question).then_some(CONTROL_TEXT)
}

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
    steps: Mutex<HashMap<String, (Value, bool)>>,
    /// 任何工具（含 `no_evidence_needed`）跑过一次：`required` 的闸门就过了
    gate_passed: AtomicBool,
    /// 端点无视 `required` 时的退回只给一次
    nudged: AtomicBool,
    /// 空回复的重问也只给一次（见 `EMPTY_REPLY_RETRY`）
    asked_again: AtomicBool,
    /// 把工具调用写成正文的退回也只给一次（见 `MARKUP_RETRY`）
    markup_retried: AtomicBool,
    /// Set before the final request so the route can withhold unvalidated text.
    finalizing: AtomicBool,
    /// Only a rejected model candidate authorizes the one-shot recovery, not an
    /// authentication, credit, transport, or database error.
    answer_requested: AtomicBool,
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
            asked_again: AtomicBool::new(false),
            markup_retried: AtomicBool::new(false),
            finalizing: AtomicBool::new(false),
            answer_requested: AtomicBool::new(false),
        })
    }

    pub fn finalizing(&self) -> bool {
        self.finalizing.load(Ordering::Relaxed)
    }

    pub fn take_answer_request(&self) -> bool {
        self.answer_requested.swap(false, Ordering::Relaxed)
    }

    fn keep_step(&self, internal_call_id: &str, step: Value, is_error: bool) {
        self.steps
            .lock()
            .expect("steps lock")
            .insert(internal_call_id.to_string(), (step, is_error));
    }

    /// 取走这次调用留给界面的那一步（没有 = 未知工具，或不留痕的闸门工具）
    pub fn take_step(&self, internal_call_id: &str) -> Option<(Value, bool)> {
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
struct Step(Value, bool);

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
                        text: result,
                        step,
                        is_error,
                        ..
                    } = {
                        let mut sink = shared.sink.lock().await;
                        tools::dispatch(&tool_ctx, &mut sink, &name, &args).await
                    };
                    shared.gate_passed.store(true, Ordering::Relaxed);
                    ctx.insert_result(Step(step, is_error));
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
    /// The next logical call hands off before provider I/O; it cannot run tools.
    pub max_rounds: usize,
}

impl AgentHook for Policy {
    fn on_completion_call(
        &self,
        _ctx: &HookContext,
        event: CompletionCallEvent<'_>,
    ) -> impl std::future::Future<Output = CompletionCallAction> + Send {
        let turn = event.turn;
        self.shared
            .finalizing
            .store(turn > self.max_rounds, Ordering::Relaxed);
        let action = if turn > self.max_rounds {
            // Rig 0.42 resolves this hook before model selection or provider I/O.
            // The route consumes this per-run state only with PromptCancelled.
            self.shared.answer_requested.store(true, Ordering::Relaxed);
            CompletionCallAction::Stop("Evidence gathering complete".into())
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
        // 只有推理、或者正文全是空白，对用户来说都是没说话
        let has_text = event
            .content
            .iter()
            .any(|c| matches!(c, AssistantContent::Text(t) if !t.text.trim().is_empty()));
        let written_call = !has_tool_call && {
            let text: String = event
                .content
                .iter()
                .filter_map(|c| match c {
                    AssistantContent::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect();
            tool_call_text(&text, &self.shared.question)
        };
        let turn = event.turn;
        let shared = self.shared.clone();
        let max_rounds = self.max_rounds;
        async move {
            // 空回复重问一次；再空就放它结束，`chat` 那边以「Model returned an empty
            // answer」收尾。**不再叠加下面那次退回**：一个始终不说话的端点只多问一次
            if !has_tool_call && !has_text {
                if !shared.asked_again.swap(true, Ordering::Relaxed) {
                    tracing::warn!(model = shared.model, turn, "模型回了空，重问一次");
                    return ModelTurnAction::Retry(RetryRequest::Feedback(
                        EMPTY_REPLY_RETRY.into(),
                    ));
                }
                return ModelTurnAction::Continue;
            }
            // 工具调用写成了正文：什么都没执行，这一回合也不是答案（#937）。哪一轮都
            // 一样，不只是首轮。退回一次；再这样就放它结束，`chat` 那边扣着没发的
            // 标记，以 `CONTROL_TEXT` 收尾，不存
            if written_call {
                if !shared.markup_retried.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        model = shared.model,
                        turn,
                        "模型把工具调用写成了正文，退回一次"
                    );
                    return ModelTurnAction::Retry(RetryRequest::Feedback(MARKUP_RETRY.into()));
                }
                tracing::warn!(
                    model = shared.model,
                    turn,
                    "退回一次后仍把工具调用写成正文，按错误收尾"
                );
                return ModelTurnAction::Continue;
            }
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

    fn on_invalid_tool_call(
        &self,
        _ctx: &HookContext,
        _event: &InvalidToolCallContext,
    ) -> impl std::future::Future<Output = Option<InvalidToolCallAction>> + Send {
        // Rig rejects calls disallowed by ToolChoice::None before on_tool_call.
        // Mark that candidate for the same answer-only recovery; never ask the
        // runner to retry a forbidden tool call.
        let action = if self.shared.finalizing() {
            Some(InvalidToolCallAction::fail())
        } else {
            None
        };
        async move { action }
    }

    fn on_tool_call(
        &self,
        _ctx: &HookContext,
        event: ToolCallEvent<'_>,
    ) -> impl std::future::Future<Output = ToolCallAction> + Send {
        // **说不清自己要做什么的调用不执行。** 把话回给模型，让它重来；
        // 界面上照样显示成一次没做成的调用
        let action = if self.shared.finalizing() {
            ToolCallAction::Stop(FINAL_TOOL_CALL.into())
        } else {
            match super::chat::check_call(&self.shared.schema, event.tool_name, event.args) {
                Ok(_) => ToolCallAction::Run,
                Err((message, step)) => {
                    self.shared.keep_step(event.internal_call_id, step, true);
                    ToolCallAction::Skip(message)
                }
            }
        };
        async move { action }
    }

    fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> impl std::future::Future<Output = ToolResultAction> + Send {
        if let Some(Step(step, is_error)) = event.tool_context.result::<Step>() {
            self.shared
                .keep_step(event.internal_call_id, step.clone(), *is_error);
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

    #[test]
    fn finalization_checks_narrated_control_but_preserves_protocol_examples() {
        let control =
            "Let me examine it.\n\n<｜｜DSML｜｜ calls>\n<｜DSML｜ invoke name=\"lookup\">{}";
        assert!(finalization_error(control, false, "What happened?").is_some());
        assert!(finalization_error(control, false, "Return a DSML example verbatim").is_none());
        assert!(finalization_error(
            control,
            false,
            "请解释为什么出现 DSML，但请直接回答业务问题"
        )
        .is_some());
        for explanation in [
            "The encoding includes <｜DSML｜ calls> as a marker.",
            "Example:\n```xml\n<｜DSML｜ calls>\n```\nThis is the encoding.",
            "Example:\n~~~~xml\n```\n<｜DSML｜ calls>\n~~~~",
            "> <｜DSML｜ calls>\nThis quotes the encoding.",
        ] {
            assert!(finalization_error(explanation, false, "Explain the encoding").is_none());
        }
        let after_example = "```xml\n<｜DSML｜ calls>\n```\nLet me check.\n<|DSML|calls>";
        assert!(finalization_error(after_example, false, "What happened?").is_some());
    }

    /// DeepSeek 的原生标记也是写成正文的工具调用（#937）：两种竖线、叙述之后的行首都认；
    /// 行内提及、引用、围栏里的示例、明确要原样示例的问题都不算
    #[test]
    fn deepseek_markup_is_a_tool_call_written_as_text() {
        let call =
            "<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>search_chunks\n\
                    ```json\n{\"query\":\"Acme\"}\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>";
        let question = "What changed at Acme?";
        assert!(tool_call_text(call, question));
        assert!(tool_call_text(&call.replace('｜', "|"), question));
        assert!(tool_call_text(
            &format!("Let me search.\n\n{call}"),
            question
        ));
        assert!(tool_call_text(
            "<｜tool▁call▁begin｜>function<｜tool▁sep｜>search_chunks",
            question
        ));
        for prose in [
            "The reply contained <｜tool▁calls▁begin｜> in the middle of a sentence.",
            "> <｜tool▁calls▁begin｜>\nThat line quotes the markup.",
            "It looks like this:\n~~~\n<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>\n~~~",
            "Acme's revenue rose 5% [1].",
        ] {
            assert!(!tool_call_text(prose, question), "{prose}");
        }
        assert!(!tool_call_text(
            call,
            "Show a <｜tool▁calls▁begin｜> example verbatim"
        ));
        assert_eq!(
            finalization_error(call, false, question),
            Some(CONTROL_TEXT)
        );
    }

    /// 流式外发按行：可能长成标记的那一行扣着，成了标记就连同这一回合剩下的都扣着；
    /// 其余照常外发，已经发出去一半的那一行不再判断
    #[test]
    fn a_line_is_held_while_it_may_become_a_tool_call() {
        // (本回合正文, 已发出的字节数, 应当发到哪里为止, 之后是否整回合扣住)
        for (text, from, out, held) in [
            ("Let me search.\n<｜to", 0, "Let me search.\n", false),
            (
                "Let me search.\n<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>",
                15,
                "Let me search.\n",
                true,
            ),
            ("One.\n\nTwo.\n  ", 0, "One.\n\nTwo.\n", false),
            ("<", 0, "", false),
            ("<b>Acme</b> grew", 0, "<b>Acme</b> grew", false),
            ("Acme grew\n<b>", 9, "Acme grew\n<b>", false),
            (
                "Acme grew <｜tool▁calls▁begin｜> inline",
                9,
                "Acme grew <｜tool▁calls▁begin｜> inline",
                false,
            ),
            ("Acme grew\n<|tool▁call▁begin|>", 9, "Acme grew\n", true),
            ("<|DSML|calls>\nMore.", 0, "", true),
        ] {
            let mut holding = false;
            let upto = publishable(text, from, &mut holding);
            assert_eq!(&text[..upto], out, "{text:?} from {from}");
            assert_eq!(holding, held, "{text:?} from {from}");
        }
        let mut holding = true;
        assert_eq!(publishable("Held.\nStill held.", 0, &mut holding), 0);
    }

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
