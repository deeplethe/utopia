//! Agentic 对话：模型自主调用工具（文档检索 / 实体查找 / 时态事实）收集证据后作答。
//! 事件序列：step*（行动轨迹）| sources（引用清单，随检索增量更新）| delta*（增量文本）→ done | error。
//! 模型不支持 tool-calling 时自动降级为一次性 RAG 注入。

use super::tools;
use crate::live::Frame;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use utopia_core::models::{ChunkView, Role};
use utopia_core::AppError;
use utopia_llm::tool_result_message;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::llm_util;
use crate::retrieval;
use crate::state::AppState;

/// 回放几个已认下的实体。上限是因为长会话会攒出几十个，全贴回去就把
/// 省下来的上下文又花掉了；按首次出现排序，早认下的通常是这场对话的主角。
const KNOWN_ENTITY_LIMIT: usize = 20;

const MAX_HISTORY: usize = 20;
const MAX_ROUNDS: usize = 6;

/// 模型说「我去查」却一个工具都没调就结束了一轮时，追问的那句话（#509）。
///
/// 实测 DeepSeek-V3 会答「请稍等，我将进行相关搜索」然后什么都不做；再问一次，
/// 还是「稍等，我正在搜索」。提示词第 1、2 条早就禁了这种叙述，所以这不是缺一句
/// 指令，是模型没听、而循环把它当成了最终答案。守卫放在循环里，不放在提示词里。
///
/// 追问只给一次、不流式、只认三种回复：调工具（照常执行）、一个词 DONE（上一句
/// 本来就是答案：打招呼、问这场对话、拒答，原样收尾）、其他任何文字（还在说空话，
/// 明说没查到证据）。给它 DONE 这条出口，是为了不让「把那句话说短一点」这种
/// 本就不需要工具的回答被追问成第二个答案。
///
/// 措辞把 DONE 的门开得窄：只有问题**不是关于用户数据**时才许说 DONE。实测还有
/// 一种更坏的停法——不说「稍等」，直接写「以下是我找到的内容」然后凭记忆作答，
/// 库里 steps、sources 全是 0。它没停住，它在撒谎。对它，「若已答完就说 DONE」
/// 是一条太宽的出口，所以这里明说：关于数据的事实性回答没有工具就不算答。
const STALL_NUDGE: &str = "(system) Your last message ended the turn without calling any tool, \
    and it cites nothing. An answer about the user's data that was not gathered with a tool is \
    not an answer, whatever the message says it found: call the tool now. Do not describe a \
    plan. Reply with the single word DONE only if the question was not about the user's data \
    at all: a greeting, a question about this transcript, or a refusal.";

/// 模型回了一个**空回复**（没有正文、也没调工具）时重问的那句。
///
/// 空回复不是一个结论，只是没说话。从前它直接变成一帧 `error`：台子上 48 轮里出过
/// 2 次，都在工具已经跑完之后——用户看见三四步查证，接着一句「Model returned an
/// empty answer」，查到的东西全白费。重问一次，第二次还空才报错：只给一次，所以一个
/// 始终不说话的端点不会让循环空转。
///
/// 措辞两头都留门：工具跑过的，就着查到的答；一步没查的（首轮就回空），缺证据
/// 就去调工具。它不说「答完了就回 DONE」——#509 那个出口正是被模型滥用的那一个。
const EMPTY_REPLY_RETRY: &str = "(system) Your previous reply was empty. Reply to the user now: \
    answer from the evidence gathered above, or call a tool if you still need evidence.";

/// 追问后仍不查时补在答案末尾的话。承诺已经流给用户了，收不回来；能做的是
/// 让文字和空白的轨迹不再互相矛盾——对一个把「每个回答可追溯」当卖点的产品，
/// 一句叙述了从未发生的查证的回答比「不知道」更糟。
const NO_EVIDENCE_NOTE: &str =
    "\n\n(No evidence was gathered for this answer: the model announced a search it did not perform.)";

/// 一轮结束、正文非空、整场没调过工具也没有引用：这个答案什么都不站在上面。
/// 「问这场对话」的消息也满足这两条，所以追问必须便宜、安静，且留有 DONE 出口。
fn answer_rests_on_nothing(steps: &[serde_json::Value], sources: &[serde_json::Value]) -> bool {
    steps.is_empty() && sources.is_empty()
}

/// 追问之后模型的回复算哪种
#[derive(Debug, PartialEq, Eq)]
enum AfterNudge {
    /// 调了工具：照常执行，接着走
    Tools,
    /// 说上一句已经是答案，或者什么都没说：原样收尾
    Done,
    /// 又是一段文字：还在说空话
    Stalled,
}

fn after_nudge(turn: &utopia_llm::AssistantTurn) -> AfterNudge {
    if !turn.tool_calls.is_empty() {
        return AfterNudge::Tools;
    }
    let said = turn
        .content
        .as_deref()
        .map(|t| t.trim().trim_matches(|c: char| !c.is_alphanumeric()))
        .unwrap_or_default();
    if said.is_empty() || said.eq_ignore_ascii_case("done") {
        AfterNudge::Done
    } else {
        AfterNudge::Stalled
    }
}

/// `remember` 曾整个停用过一段（见 `docs/decisions/0015`）：它那时会把一句话直接
/// 变成图上一条活边，实测里「记住 Acme 把总部搬到了深圳」落成的是一条**空谓词、
/// 0.9 置信**的边，而助手宣称的和图里得到的不是一回事。
///
/// 现在抽取侧接上了 `pending_facts`：记忆抽出的事实先等人点头，再进账本。
/// 这个开关留着，是为了下次再发现「工具会悄悄改图」时有地方立刻拉闸——
/// 宁可没有这个工具，也不要一个会悄悄改图的工具。
pub(super) const REMEMBER_ENABLED: bool = true;

#[derive(Deserialize)]
pub struct ChatReq {
    /// 缺省 = 新建会话（SSE 首个 `conversation` 事件回传 id）
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    pub message: String,
}

/// 工具清单。**MCP 也用这一份**（`mcp.rs`）：名字、描述、参数 schema 抄成
/// 两套迟早分叉，而它们是与 `tools.rs` 执行侧的契约
pub(super) fn tools_schema(can_write: bool, data_source_names: &[String]) -> serde_json::Value {
    let mut tools = base_tools();
    if !data_source_names.is_empty() {
        if let Some(arr) = tools.as_array_mut() {
            arr.push(json!({
                "type": "function",
                "function": {
                    "name": "query_data",
                    "description": format!(
                        "Run a read-only SQL query against a mounted database. Available sources: \
                         {}. Search the source's schema document first (search_chunks) if unsure \
                         of tables/columns. Only a single SELECT/WITH statement is allowed; a \
                         LIMIT is enforced server-side; results come back as JSON lines. If the \
                         query errors, fix the SQL and retry once.",
                        data_source_names.join(", ")
                    ),
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "data_source": {
                                "type": "string",
                                "description": "Name of the mounted data source to query."
                            },
                            "sql": {
                                "type": "string",
                                "description": "One SELECT/WITH statement in the source's own SQL dialect (PostgreSQL, Trino, Databricks or Snowflake; the schema document names the engine)."
                            },
                            "purpose": {
                                "type": "string",
                                "description": "One short phrase: what this query answers (shown to the user)."
                            }
                        },
                        "required": ["data_source", "sql"]
                    }
                }
            }));
        }
    }
    if can_write && REMEMBER_ENABLED {
        if let Some(arr) = tools.as_array_mut() {
            arr.push(json!({
                "type": "function",
                "function": {
                    "name": "remember",
                    "description": "Record one memory episode into the knowledge base's temporal \
                        memory. Use ONLY when the user explicitly asks to remember/record \
                        something, or clearly states a decision or fact to keep. The sentence \
                        is stored immediately; facts extracted from it are PROPOSED and shown \
                        to the user for confirmation before they enter the knowledge graph. \
                        Never claim a fact was added to the graph. Do not use for casual \
                        conversation.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "The episode to remember, one self-contained \
                                    statement (who/what, with names spelled out)."
                            },
                            "occurred_at": {
                                "type": "string",
                                "description": "Optional date the stated fact took effect \
                                    (YYYY, YYYY-MM, YYYY-MM-DD or RFC3339). Omit to use today."
                            }
                        },
                        "required": ["text"]
                    }
                }
            }));
        }
    }
    tools
}

/// 执行之前先看这次调用说清楚了没有。
///
/// **两种「没说清」以前都会安静地变成一次正常调用。**
///
/// 一是参数解析不出来。模型的输出撞上 token 上限时，`arguments` 会在半路断掉，
/// 那串 JSON 不完整。从前这里是 `unwrap_or_else(|_| json!({}))`——空对象，
/// 接着 `search_chunks` 里 `args["query"].as_str().unwrap_or(&query)` 回落到
/// **用户那句原话**，于是一次被截断的调用变成「拿用户的原问题去检索」，
/// 而轨迹上显示的是一条完全正常的 `search · 6 sources`。
///
/// 二是必填参数干脆没给。同一个回落，同一个结果。
///
/// 两种都不该猜。**回落产出的是一个看起来没问题的错误答案**，那比报错坏得多——
/// 报错模型会重试，猜出来的答案没有人会去核。
///
/// 判据直接取自工具表里的 `required`：加一个必填参数，这里自动跟上，
/// 不必记得来改第二处。
pub(super) fn check_call(
    tools: &serde_json::Value,
    name: &str,
    raw_args: &str,
) -> Result<serde_json::Value, (String, serde_json::Value)> {
    let refuse = |detail: &str, message: String| {
        (
            message,
            json!({ "kind": "tool", "label": name, "detail": detail }),
        )
    };
    let Ok(args) = serde_json::from_str::<serde_json::Value>(raw_args) else {
        return Err(refuse(
            "bad arguments",
            format!(
                "The arguments for {name} were not valid JSON, so the call was not run. \
                 They were probably cut off. Call it again with complete arguments."
            ),
        ));
    };
    let function = tools
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["function"]["name"] == name)
        .map(|t| &t["function"]);
    let required = function.and_then(|f| f["parameters"]["required"].as_array());
    for key in required.into_iter().flatten().filter_map(|k| k.as_str()) {
        // 空串与 null 都算没给：`{"query": ""}` 检索出来的东西与问题无关，
        // 而它同样会显示成一条正常的轨迹
        let missing = match args.get(key) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(s)) => s.trim().is_empty(),
            Some(_) => false,
        };
        if missing {
            return Err(refuse(
                &format!("missing {key}"),
                format!(
                    "{name} needs `{key}`, and it was missing or empty, so the call was not \
                     run. Call it again with `{key}` set."
                ),
            ));
        }
        // 判据同样取自工具表：schema 里写了 `format: uuid` 的参数，格式也在这一关挡。
        // 编出来的 id 到了工具里只能回「本库没有这篇文档」，模型会把它读成
        // 「库里真的没有」而放弃——那是又一个看起来没问题的错误答案
        let is_uuid =
            function.is_some_and(|f| f["parameters"]["properties"][key]["format"] == "uuid");
        if is_uuid
            && args[key]
                .as_str()
                .is_none_or(|s| s.trim().parse::<Uuid>().is_err())
        {
            return Err(refuse(
                &format!("invalid {key}"),
                format!(
                    "{name} needs `{key}` to be a uuid returned by another tool, and it was \
                     not, so the call was not run. Look the id up first, then call it again."
                ),
            ));
        }
    }
    Ok(args)
}

const MEMORY_PROMPT: &str = "\
    Memory: you can persist knowledge with the remember tool. Use it when the user says \
    \"remember/record this\" or states a decision meant to last. In your reply, say that the \
    sentence was recorded and that the facts extracted from it will be shown for the user's \
    confirmation before entering the graph; never say a fact is already in the graph. \
    Never invent memories, and never call it for small talk.";

/// 工具的 JSON schema——**给模型看的那一份**。
///
/// 留在 `chat.rs` 而不是 `tools.rs`：它是提示词的一部分，随对话策略走；
/// `tools.rs` 只管拿到参数之后干什么（见那个模块顶上的说明）。
/// MCP 也读它，所以对同模块开放——两边描述同一批工具，各写一份必然分叉。
pub(super) fn base_tools() -> serde_json::Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "search_chunks",
                "description": "Full-text + semantic search over the knowledge base documents. \
                    Returns numbered source excerpts you can cite as [n], each with the \
                    document_id it came from. Excerpts are cut short and only the best-matching \
                    sections come back, so when the answer may sit elsewhere in a hit, call \
                    get_document with that document_id to read the whole document.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query, phrased in the corpus language." },
                        "as_of": {
                            "type": "string",
                            "description": "Optional RECORD-time moment (YYYY-MM-DD or RFC3339): search only what the knowledge base held at that moment — earlier versions of documents, documents deleted since. Full-text recall stays current, so hits are correct but may be incomplete. Omit for the current base."
                        }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_document",
                "description": "Read the full text of ONE knowledge base document, all sections \
                    in order, by the document_id shown in search_chunks results. Use it whenever \
                    a search hit looks like the right document but the excerpt does not contain \
                    the answer — action items, decisions and lists usually sit past the excerpt \
                    or in a section that did not match the query.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "document_id": {
                            "type": "string",
                            "format": "uuid",
                            "description": "Document id (uuid) from a search_chunks result line."
                        }
                    },
                    "required": ["document_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_docs",
                "description": "Search the Utopia product manual (the \"Utopia Charter\") — how \
                    the platform itself works (ingestion and sync, missing markers and versions, \
                    the graph and review flow, roles, settings) — and never the user's own \
                    documents, which live in search_chunks.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to look up in the manual." }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "find_entities",
                "description": "Look up entities in the knowledge graph by (partial) name. \
                    Returns id, name, type and a disambiguator when several entities share a name.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Entity name or a fragment of it." }
                    },
                    "required": ["name"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "entity_facts",
                "description": "Facts about one entity from the bi-temporal knowledge graph: \
                    relations with validity ranges (from → to; 'now' = still ongoing). \
                    The best tool for who/when/history questions. Use after find_entities. \
                    Pass `at` to see the world as of that date (server-side filter) — \
                    always do this for \"who was X in <year/month>\" questions. \
                    Conclusions a business rule reached about this entity come back too, \
                    marked `[rule: <name>]` with the readings that made them true — take \
                    those as given rather than re-deriving them from the readings yourself. \
                    Output is grouped by predicate and cut at `limit`; narrow with predicate, \
                    object_type, since / until, or use timeline / neighbors / paths_between.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "entity_id": { "type": "string", "description": "Entity id (uuid) from find_entities, or an entity name (the server picks the best match and says which)." },
                        "predicate": { "type": "string", "description": "Optional: only facts whose relation name contains this (e.g. 'founder')." },
                        "object_type": { "type": "string", "description": "Optional: only facts whose other end is of this type (e.g. 'Person')." },
                        "since": { "type": "string", "description": "Optional WORLD-time window start: facts still holding after it." },
                        "until": { "type": "string", "description": "Optional WORLD-time window end: facts that had started by it." },
                        "limit": { "type": "integer", "description": "How many facts to return (default 80, max 300); the reply says when it is cut." },
                        "at": {
                            "type": "string",
                            "description": "Optional as-of moment on the WORLD axis (YYYY, YYYY-MM, \
                                YYYY-MM-DD, or a zoned time). Only facts valid at that moment are \
                                returned. Omit for the full history."
                        },
                        "as_of": {
                            "type": "string",
                            "description": "Optional RECORD-time moment (YYYY-MM-DD or RFC3339): the facts as the knowledge base held them at that moment, before later corrections, retractions and merges. Use for 'what did we think / know / have on record as of <date>'. Independent of `at`: `at` is when something was true, `as_of` is when we believed it. Omit for today's understanding."
                        },
                        "before": {
                            "type": "string",
                            "description": "Optional RECORD-time instant copied exactly from a changes event: the facts as the knowledge base held them strictly before that change landed. Use it for 'before <correction / memo> arrived' — paste the event's timestamp, do not compute an earlier as_of yourself. Overrides as_of."
                        }
                    },
                    "required": ["entity_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "paths_between",
                "description": "How two entities are connected in the knowledge graph: the chains of facts that join them, up to 3 hops, shortest first. THE tool for 'what is the relation between A and B', 'how is X linked to Y', 'who connects A and B'. Both ends take an entity name or an id; with a name the server picks the best match and says which. Pass `at` to require every edge to hold at that moment (world time), `as_of` to read the base as recorded then. Each edge comes with its validity range.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string", "description": "One end: entity name, or an id from find_entities." },
                        "to": { "type": "string", "description": "The other end: entity name, or an id." },
                        "max_hops": { "type": "integer", "description": "Longest chain to consider, 1-3 (default 3)." },
                        "at": { "type": "string", "description": "Optional WORLD-time moment (YYYY, YYYY-MM, YYYY-MM-DD): every edge on a path must hold then." },
                        "as_of": { "type": "string", "description": "Optional RECORD-time moment: the paths as the base held them then." }
                    },
                    "required": ["from", "to"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "neighbors",
                "description": "The entities linked to one entity, grouped by predicate, one hop. Use it to see what surrounds an entity before deciding where to look next (each further hop is another call); narrow with `predicate` (e.g. 'founder', 'employee') or `object_type` (e.g. 'Person'). Attribute values are not neighbors; entity_facts has them.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "entity": { "type": "string", "description": "Entity name, or an id from find_entities." },
                        "predicate": { "type": "string", "description": "Optional: only relations whose name contains this." },
                        "object_type": { "type": "string", "description": "Optional: only neighbors of this type (name contains)." },
                        "at": { "type": "string", "description": "Optional WORLD-time moment: only links valid then." },
                        "as_of": { "type": "string", "description": "Optional RECORD-time moment." },
                        "limit": { "type": "integer", "description": "How many to return (default 40, max 300)." }
                    },
                    "required": ["entity"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "timeline",
                "description": "One entity's dated facts in world-time order: THE tool for 'the timeline of X', 'the history of X', 'what happened to X between <year> and <year>'. Only facts with a stated date appear; narrow with `since` / `until` (world time) or `predicate`.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "entity": { "type": "string", "description": "Entity name, or an id from find_entities." },
                        "since": { "type": "string", "description": "Optional start of the window (YYYY, YYYY-MM, YYYY-MM-DD)." },
                        "until": { "type": "string", "description": "Optional end of the window." },
                        "predicate": { "type": "string", "description": "Optional: only relations whose name contains this." },
                        "as_of": { "type": "string", "description": "Optional RECORD-time moment: the timeline as the base held it then." },
                        "limit": { "type": "integer", "description": "How many to return (default 60, max 300)." }
                    },
                    "required": ["entity"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "list_rules",
                "description": "The business rules this base runs: what each one concludes and                     the exact conditions it tests, thresholds included. A rule is written by a                     person, and its conclusions are already in the graph — read the rule to                     explain WHY something was concluded, or to answer \"what counts as X here\".                     Do not re-implement a rule's comparison yourself; ask entity_facts or                     rule_matches for what it actually concluded.",
                "parameters": { "type": "object", "properties": {} }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "rule_matches",
                "description": "Which entities a business rule currently marks, with the                     readings that made each one true. Use it for \"which wells are gas-bearing\"                     style questions — one call instead of checking every entity.                     Get the rule id from list_rules.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "rule_id": { "type": "string", "description": "Rule id (uuid) from list_rules." },
                        "limit": { "type": "integer", "description": "How many to return (default 50, max 200)." }
                    },
                    "required": ["rule_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "changes",
                "description": "What the graph LEARNED or REVISED in a window of record time —                     the belief axis. Answers \"what changed since X\", \"what did we get wrong\",                     \"what is new this quarter\", and needs no entity, so use it when the                     question names a period rather than a subject.                     Events: asserted (new claim), corrected (a claim replaced by a revised one),                     rejected (a claim withdrawn), merged (folded into another claim) — each with                     the document it came from.                     NOT the same axis as entity_facts(at): that asks \"what was true on date D\";                     this asks \"what did we change our mind about between D1 and D2\". A fact                     about 2019 can be recorded in 2026 — this windows on when we recorded it.                     Each event starts with its exact UTC RFC3339 record timestamp, including fractional seconds.                     For 'before a correction arrived', find that event here and pass its timestamp, exactly as printed, to entity_facts as `before`. Keep at for the world date asked about.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "since": {
                            "type": "string",
                            "description": "Start of the window (YYYY, YYYY-MM or YYYY-MM-DD), inclusive — a year or month means its first day."
                        },
                        "until": {
                            "type": "string",
                            "description": "End of the window (YYYY, YYYY-MM or YYYY-MM-DD), inclusive of that                                 whole day, month or year. Omit for 'up to now'."
                        },
                        "entity_id": {
                            "type": "string",
                            "description": "Optional entity id from find_entities, to narrow the                                 window to changes touching that one entity."
                        },
                        "kinds": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": ["asserted", "corrected", "rejected", "merged"]
                            },
                            "description": "Optional filter. A freshly ingested corpus is nearly                                 all 'asserted'; pass [\"corrected\", \"rejected\"] to isolate                                 the places we actually changed our mind."
                        }
                    },
                    "required": ["since"]
                }
            }
        }
    ])
}

const SYSTEM_PROMPT: &str = "You are the assistant of Utopia, a temporal knowledge platform. \
    You have tools: search_chunks (document search) and get_document (the full text of one \
    document found by search), find_entities, entity_facts, neighbors, timeline, \
    paths_between and changes (a bi-temporal knowledge graph), and search_docs (Utopia's \
    own manual, the Charter).\n\
    search_chunks returns short excerpts of the best-matching sections only. When a hit is \
    clearly the right document but the excerpt does not carry the answer, read the whole \
    document with get_document before saying the knowledge base does not have it.\n\
    The graph has TWO independent time axes:\n\
    - World time — when something was true. entity_facts(`at` = as of that date).\n\
    - Record time — when we came to believe it, and when we revised it. entity_facts and search_chunks take `as_of` = the base as it stood at that moment, before later corrections, retractions and merges; changes lists what moved in a window.\n\
    \"Who was CTO in 2019\" is world time; \"what did we learn last month\" and \"what did \
    we get wrong\" are record time. The same fact has a position on both.\n\
    Boundary: search_docs answers questions about Utopia itself (features, ingestion, \
    permissions, what fields like 'missing' or validity ranges mean); the other tools answer \
    questions about the knowledge stored in it. Never mix the manual into answers about the \
    user's data unless they asked about Utopia's behavior.\n\
    \n\
    Method:\n\
    First decide what the message is about. A message about THIS CONVERSATION — translate it, \
    say it shorter, rephrase it, \"what did you just say\", \"why\" — is answered from the \
    transcript above with NO tool calls: the evidence is already in it. Gathering it again is \
    not merely wasted work — with several entities sharing a name the second pass can land on \
    a different one, and the \"translation\" then says something else. Just deliver it — no \
    preamble about what you are or are not looking up. Everything below is for messages about \
    the user's data.\n\
    1. For factual questions — questions about the user's data, never one about this \
       conversation — ALWAYS gather evidence with tools before answering. For how two \
       things are related, call paths_between (names are fine); for the history of one \
       thing, timeline; to see what is linked to it, neighbors; for its facts, entity_facts, \
       narrowed with predicate / object_type / since / until. search_chunks answers content \
       and detail questions. Combine both when useful.\n\
    2. Facts carry validity ranges (from → to). For \"as of <date>\" questions pass `at` to \
       entity_facts and the server filters to that moment; for history questions omit `at` \
       to see the full timeline. For 'what did we know / have on record / believe as of <date>' or 'before <memo> arrived' pass `as_of` — that is the record axis and the ONLY way to answer such a question; do not narrate a plan, call the tool. The two combine: `at` for the date asked about, `as_of` for when. State dates in the answer. Dates in tool output carry their own precision: \
       `2023` means the year and `2023-06` the month — never turn them into a specific day; \
       `undated` marks a fact with no stated start; \
       `ended, date unknown` marks one the text says is over, date not given.\n\
    2a. For 'before a correction or memo arrived', call changes to find its exact record timestamp, \
       then call entity_facts with `before` set to that timestamp, copied exactly as printed — the \
       server reads the base as it stood strictly before that change. Never compute an earlier \
       `as_of` yourself. Keep `at` for the world date asked about.\n\
    2b. For \"what changed / what is new / what did we get wrong since <date>\", call changes — \
       it needs no entity. Name the document a correction came from in plain prose. Graph tools \
       return no [n] numbers and no URLs, so never write a bracketed citation or a placeholder \
       like [Link] after one — the document's name IS the attribution.\n\
    3. Several entities can share one name — check the disambiguator and pick the right one; \
       if genuinely ambiguous, ask the user which one they mean.\n\
    4. Stop calling tools as soon as you have enough evidence. Then answer concisely: cite \
       document sources with [n] (numbers from search results) at the end of supported \
       sentences. If the evidence is insufficient, say so explicitly — never fabricate.\n\
    5. Always respond in the same language as the user's question.";

pub async fn chat(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<ChatReq>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let kb = utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    // 写工具跟人走：editor 及以上的对话才带 remember，viewer 纯只读
    // 挂载的数据源决定 query_data 是否入列（问数是读，viewer 亦可用）
    let mounted_sources = utopia_store::datasources::mounted(&state.pool, kb_id).await?;
    // 语义层：人确认过的 指标/维度 → 数据资产 映射，直接进 system prompt——
    // 问数优先用确认口径，而不是每次从 schema 猜。
    //
    // 从前这里按 `confidence >= 0.75` 捞事实,而那个阈值是拿浮点数编码一个
    // 二值状态(提议 0.6 / 确认 1.0)。现在读 `status = confirmed`(0011)
    // 口径在下面按问题挑（`mapping_index::relevant`）——从前这里 `confirmed(kb, 30)`
    // 按字典序取前三十条，一百条口径的库有七十条永远进不了提示词（#574）
    let can_write = utopia_store::access::kb_role(&state.pool, &user, &kb)
        .await?
        .is_some_and(|r| r >= Role::Editor);

    const NO_MODEL: &str = "Chat model not configured. Go to Settings → Models.";
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| AppError::invalid("no_chat_model", NO_MODEL))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| AppError::invalid("no_chat_model", NO_MODEL))?;

    let query = req.message.trim().to_string();
    if query.is_empty() {
        return Err(AppError::Validation("Missing user message".into()).into());
    }
    // 语义层：跟这个问题有关的那几条确认口径进 system prompt——问数优先用确认口径，
    // 而不是每次从 schema 猜。按问题挑而不是全塞：二十七条的上界 17/18 是在
    // 三十条的上限之下量的，一百条口径靠字典序截断就不成立了（#574）
    let mappings = if mounted_sources.is_empty() {
        Vec::new()
    } else {
        crate::mapping_index::relevant(
            &state,
            kb_id,
            kb.workspace_id,
            &query,
            crate::mapping_index::DEFINITIONS_IN_PROMPT,
        )
        .await
        .map_err(AppError::Other)?
    };

    // 会话持久化：有 id 则校验归属，无则以首句为题新建；用户消息即刻落库,
    // 上下文由服务端从库里拼——前端只送新消息
    let conversation_id = match req.conversation_id {
        Some(id) => {
            utopia_store::conversations::require_owned(&state.pool, kb_id, user.id, id).await?;
            id
        }
        None => utopia_store::conversations::create(&state.pool, kb_id, user.id, &query).await?,
    };
    utopia_store::conversations::append_message(
        &state.pool,
        conversation_id,
        "user",
        &query,
        &utopia_store::conversations::TurnRecord::empty(),
    )
    .await?;
    let history = utopia_store::conversations::recent_context(
        &state.pool,
        conversation_id,
        MAX_HISTORY as i64,
    )
    .await?;
    let workspace_id = kb.workspace_id;
    // 数据描述（探索从 schema 写的）与约定（人写的）跟着进 system prompt。
    // **每次都在，不靠检索碰运气**：约定写成一页文档只靠检索也到过 14/18，
    // 但那是因为这批问题都在问指标才每题命中（#520）
    let data_description = kb.data_description.clone().filter(|s| !s.trim().is_empty());
    let data_conventions = kb.data_conventions.clone().filter(|s| !s.trim().is_empty());

    // 注册表在生成器之前取出来：下面那个 `async_stream!` 会把 `state` 整个搬走
    let live = state.live.clone();

    // 生成过程不挂在这条连接上。
    //
    // **切走一次就丢一个回答**，而且丢得比看上去彻底：整段生成住在下面这个
    // 生成器里，助手消息只在走完时落库；浏览器一导航，axum 丢掉响应体、
    // 生成器 future 被丢弃，于是 LLM 调用当场取消，那句 `append_message`
    // 永远不执行。实测掐断连接时已经收到 1219 字节的正文，二十秒后库里
    // 只剩用户那一行——**那个回答不是存在但没显示，是根本没被生成完**。
    //
    // 所以把生成器交给一个独立任务去驱动，这条连接降级成一个订阅者。
    // 任务不随连接消失，答案照常写完、照常落库，人回来就在。
    //
    // 代价说清楚：**没人看的时候仍然在花钱**。这是有意的——丢答案比多跑一轮贵，
    // 而 `MAX_ROUNDS` 已经给了上限。send 失败（接收端没了）不中断，那正是要点。
    let producer = async_stream::stream! {
        let ds_names: Vec<String> = mounted_sources.iter().map(|d| d.name.clone()).collect();
        let tools = tools_schema(can_write, &ds_names);
        let mut system_prompt = if can_write && REMEMBER_ENABLED {
            format!("{SYSTEM_PROMPT}\n{MEMORY_PROMPT}")
        } else {
            SYSTEM_PROMPT.to_string()
        };
        if !ds_names.is_empty() {
            system_prompt.push_str(&format!(
                "\nData: query_data runs read-only SQL (in each source's own dialect) against: {}. \
                 For questions about numbers/metrics, search for the source's schema document \
                 first, then query. State units and the time range you used in the answer.",
                ds_names.join(", ")
            ));
            // 描述说的是 schema 里有的（粒度、单位、码值、时间轴），约定说的是 schema 里
            // 没有的（哪些行算数、哪列才是那个数）。后者是问数从 2/18 到 14/18 的那一半
            if let Some(d) = &data_description {
                system_prompt.push_str(&format!(
                    "\nAbout the data (written from the schema; states only what the schema says):\n{d}"
                ));
            }
            if let Some(c) = &data_conventions {
                system_prompt.push_str(&format!(
                    "\nConventions stated by the owner of this base — apply them in every query \
                     and every answer (filters, units, which column is the figure):\n{c}"
                ));
            }
            if !mappings.is_empty() {
                system_prompt.push_str(
                    "\nSemantic layer (confirmed definitions — use these instead of guessing from schema):",
                );
                // 从前这里直接把整份 JSON 打进去。现在字段是列，只挑问数用得上的
                // 那几样铺开——`sql` 与 `expr` 是「怎么算」，`unit` 是答里必须带的
                // 量纲，`summary` 是给模型的一句人话
                for m in &mappings {
                    let how = m
                        .sql
                        .as_deref()
                        .or(m.expr.as_deref())
                        .or(m.table_name.as_deref())
                        .unwrap_or("-");
                    let unit = m
                        .unit
                        .as_deref()
                        .map(|u| format!(" [{u}]"))
                        .unwrap_or_default();
                    let note = m
                        .summary
                        .as_deref()
                        .map(|s| format!(" — {s}"))
                        .unwrap_or_default();
                    system_prompt.push_str(&format!(
                        "\n- {} ({}){unit}: {how}{note}",
                        m.concept_name, m.source
                    ));
                }
            }
        }
        let mut msgs: Vec<serde_json::Value> =
            vec![json!({ "role": "system", "content": system_prompt })];
        /* **上一轮做过什么，按它当时发生的位置放回去。**
           最后那条助手消息是它的结论；带 `tool_calls` 的消息与 tool 结果
           发生在它之前，所以插在它前面——顺序就是真实顺序，模型读起来
           就是「我问了、我查了、我答了」。
           少了这一段，跨轮之后它只看得见自己写的散文，于是接着说「翻译」
           时重查一遍（还可能落到另一批同名实体上）。 */
        let last_assistant = history
            .turns
            .iter()
            .rposition(|(role, _)| role == "assistant");
        for (i, (role, content)) in history.turns.iter().enumerate() {
            if Some(i) == last_assistant {
                for m in &history.last_tool_exchange {
                    msgs.push(m.clone());
                }
            }
            msgs.push(json!({ "role": role, "content": content }));
        }
        // **前几轮已经认下的实体，连 id 一起交回去。**
        //
        // 少了这一段，模型只看得见上一轮的最终答案文字，不知道自己搜过什么、
        // 拿到过哪些 id，于是从名字重搜一遍。更隐蔽的是同名歧义时两轮可能落到
        // **不同的实体**上，前后两个答案讲的不是同一个节点。
        //
        // 贴在历史之后、当前问题之前——位置就是服从性，跟抽取里 known_block
        // 紧挨正文是同一条理由。
        if !history.entities.is_empty() {
            let lines: Vec<String> = history.entities
                .iter()
                .take(KNOWN_ENTITY_LIMIT)
                .map(|e| {
                    format!(
                        "{} | {} | {}",
                        e["id"].as_str().unwrap_or("?"),
                        e["name"].as_str().unwrap_or("?"),
                        e["type"].as_str().unwrap_or("?")
                    )
                })
                .collect();
            msgs.push(json!({
                "role": "user",
                "content": format!(
                    "Entities already identified earlier in this conversation                      (id | name | type). Call entity_facts with these ids directly;                      do not look them up by name again:
    {}",
                    lines.join("
    ")
                )
            }));
        }

        // 会话 id 先行下发（新会话由此告知前端）
        yield Frame::new("conversation", json!({ "id": conversation_id }).to_string());

        // 引用清单与这一轮认下的实体。**攒在工具外面**——`[3]` 里的 3 取决于
        // 之前已经引过几个，各个工具各算各的会让同一个 chunk 拿到两个号
        let mut sink = tools::ToolSink::default();
        // 落库累积：assistant 全文与行动轨迹（历史回放用）
        let mut answer_acc = String::new();
        let mut steps_acc: Vec<serde_json::Value> = Vec::new();
        // 这一轮的工具往返，原样留一份落库：下一轮回放它，模型才知道自己做过什么
        let mut exchange_acc: Vec<serde_json::Value> = Vec::new();

        let mut rounds = 0usize;
        // #509 的追问只给一次
        let mut nudged = false;
        // 空回复的重问也只给一次（见 EMPTY_REPLY_RETRY）
        let mut asked_again = false;
        loop {
            if rounds >= MAX_ROUNDS {
                // 弹药耗尽：命令模型就现有证据作答（流式）
                msgs.push(json!({
                    "role": "user",
                    "content": "(system) Tool budget exhausted. Answer now from the evidence gathered above.",
                }));
                match client.chat_stream_raw(&msgs).await {
                    Ok(deltas) => {
                        let mut deltas = std::pin::pin!(deltas);
                        while let Some(item) = deltas.next().await {
                            match item {
                                Ok(text) => { answer_acc.push_str(&text); yield delta_event(&text); }
                                Err(e) => { yield error_event(&e.to_string()); return; }
                            }
                        }
                        let _ = utopia_store::conversations::append_message(
                            &state.pool, conversation_id, "assistant", &answer_acc,
                            &utopia_store::conversations::TurnRecord {
                                steps: serde_json::Value::Array(steps_acc.clone()),
                                sources: serde_json::Value::Array(sink.sources.clone()),
                                resolved: serde_json::Value::Array(sink.resolved.clone()),
                                tool_exchange: serde_json::Value::Array(exchange_acc.clone()),
                            },
                        ).await;
                        yield done_event();
                    }
                    Err(e) => yield error_event(&e.to_string()),
                }
                return;
            }

            // 主链路全程流式：正文增量即时转发，工具调用在流末归并到达
            let deltas = match client.chat_tools_stream(&msgs, &tools).await {
                Ok(s) => s,
                Err(e) => {
                    if rounds == 0 {
                        // 模型可能不支持 tool-calling：降级为一次性 RAG 注入
                        tracing::warn!(error = %e, "tool-calling 不可用，降级为一次性 RAG");
                        let chunks =
                            retrieval::hybrid(&state, kb_id, workspace_id, &query, 8, None)
                                .await
                                .unwrap_or_default();
                        let legacy_sources: Vec<serde_json::Value> = chunks
                            .iter()
                            .enumerate()
                            .map(|(i, c)| source_json(i + 1, c))
                            .collect();
                        yield Frame::new(
                            "sources",
                            serde_json::to_string(&legacy_sources).unwrap_or_else(|_| "[]".into()),
                        );
                        let mut lmsgs =
                            vec![json!({ "role": "system", "content": legacy_system_prompt(&chunks) })];
                        for (role, content) in &history.turns {
                            lmsgs.push(json!({ "role": role, "content": content }));
                        }
                        match client.chat_stream_raw(&lmsgs).await {
                            Ok(deltas) => {
                                let mut deltas = std::pin::pin!(deltas);
                                while let Some(item) = deltas.next().await {
                                    match item {
                                        Ok(text) => { answer_acc.push_str(&text); yield delta_event(&text); }
                                        Err(e2) => { yield error_event(&e2.to_string()); return; }
                                    }
                                }
                                let _ = utopia_store::conversations::append_message(
                                    &state.pool, conversation_id, "assistant", &answer_acc,
                                    &utopia_store::conversations::TurnRecord {
                                        steps: serde_json::Value::Array(steps_acc.clone()),
                                        sources: serde_json::Value::Array(legacy_sources.clone()),
                                        resolved: serde_json::Value::Array(sink.resolved.clone()),
                                        tool_exchange: serde_json::Value::Array(exchange_acc.clone()),
                                    },
                                ).await;
                                yield done_event();
                            }
                            Err(e2) => yield error_event(&e2.to_string()),
                        }
                        return;
                    }
                    yield error_event(&e.to_string());
                    return;
                }
            };
            let mut turn: Option<utopia_llm::AssistantTurn> = None;
            {
                let mut deltas = std::pin::pin!(deltas);
                while let Some(item) = deltas.next().await {
                    match item {
                        Ok(utopia_llm::ToolStreamItem::Delta(text)) => {
                            answer_acc.push_str(&text);
                            yield delta_event(&text);
                        }
                        Ok(utopia_llm::ToolStreamItem::Turn(t)) => turn = Some(t),
                        Err(e) => {
                            yield error_event(&e.to_string());
                            return;
                        }
                    }
                }
            }
            let Some(mut turn) = turn else {
                yield error_event("LLM stream ended unexpectedly");
                return;
            };

            if turn.tool_calls.is_empty() {
                if answer_acc.is_empty() {
                    if asked_again {
                        yield error_event("Model returned an empty answer");
                        return;
                    }
                    asked_again = true;
                    tracing::warn!(
                        model = settings.chat_model.as_deref().unwrap_or_default(),
                        rounds,
                        "模型回了空，重问一次"
                    );
                    // 空的那一轮本身不进 msgs：一条既无正文又无调用的 assistant 消息，
                    // 有的端点直接拒收。回到循环顶上重来，不占工具轮数
                    msgs.push(json!({ "role": "user", "content": EMPTY_REPLY_RETRY }));
                    continue;
                }
                // **没调工具的一轮不一定是答完了，也可能是停住了**（#509）：正文说
                // 「我去查」，然后轮次就结束。循环分不出这两种，靠一次追问让模型自己
                // 表态。追问不流式：模型若只是确认 DONE，用户不该看见那个词
                let mut carry_on_with_tools = false;
                if !nudged && answer_rests_on_nothing(&steps_acc, &sink.sources) {
                    nudged = true;
                    let model = settings.chat_model.clone().unwrap_or_default();
                    msgs.push(turn.to_message());
                    msgs.push(json!({ "role": "user", "content": STALL_NUDGE }));
                    match client.chat_tools(&msgs, &tools).await {
                        Ok(second) => match after_nudge(&second) {
                            AfterNudge::Tools => {
                                tracing::warn!(model, "模型只说了要查没查，追问后调了工具");
                                // 追问那轮的叙述没有流过，这里补上，接在承诺后面
                                if let Some(text) = second.content.as_deref().filter(|t| !t.trim().is_empty()) {
                                    answer_acc.push_str("\n\n");
                                    yield delta_event("\n\n");
                                    answer_acc.push_str(text);
                                    yield delta_event(text);
                                }
                                turn = second;
                                carry_on_with_tools = true;
                            }
                            // 也记一笔：DONE 说得对不对没法在这里判，只能靠日志和这条
                            // 回答的 sources 为空这个事实，事后一起看
                            AfterNudge::Done => tracing::info!(model, "模型追问后说上一句已是答案"),
                            AfterNudge::Stalled => {
                                tracing::warn!(model, "模型追问后仍只说不查，答案标注无证据");
                                answer_acc.push_str(NO_EVIDENCE_NOTE);
                                yield delta_event(NO_EVIDENCE_NOTE);
                            }
                        },
                        // 追问本身失败不能拖垮已经到手的答案
                        Err(e) => tracing::warn!(model, error = %e, "追问失败，按原答案收尾"),
                    }
                }
                if !carry_on_with_tools {
                    let _ = utopia_store::conversations::append_message(
                        &state.pool, conversation_id, "assistant", &answer_acc,
                        &utopia_store::conversations::TurnRecord {
                            steps: serde_json::Value::Array(steps_acc.clone()),
                            sources: serde_json::Value::Array(sink.sources.clone()),
                            resolved: serde_json::Value::Array(sink.resolved.clone()),
                            tool_exchange: serde_json::Value::Array(exchange_acc.clone()),
                        },
                    ).await;
                    yield done_event();
                    return;
                }
            }

            // 工具轮带了叙述文本：与后续轮次的正文之间补一个段落分隔
            if turn.content.is_some() && !answer_acc.is_empty() {
                answer_acc.push_str("\n\n");
                yield delta_event("\n\n");
            }

            let call_msg = turn.to_message();
            exchange_acc.push(call_msg.clone());
            msgs.push(call_msg);
            for call in &turn.tool_calls {
                // **说不清自己要做什么的调用不执行。** 把话回给模型，让它重来
                let args = match check_call(&tools, &call.name, &call.arguments) {
                    Ok(args) => args,
                    Err((message, step)) => {
                        steps_acc.push(step.clone());
                        yield Frame::new("step", serde_json::to_string(&step).unwrap_or_default());
                        msgs.push(tool_result_message(&call.id, &message));
                        continue;
                    }
                };
                let ctx = tools::ToolCtx {
                    state: &state,
                    kb_id,
                    workspace_id,
                    mounted_sources: &mounted_sources,
                    can_write,
                    actor: Some(user.id),
                    // 网页端对话不经令牌：说话的就是这个人本人
                    via_token: None,
                    question: Some(&query),
                };
                let tools::ToolResult { text: result, step, .. } =
                    tools::dispatch(&ctx, &mut sink, &call.name, &args).await;
                // **这一步发生在正文的哪个位置。**
                //
                // 模型是边说边调的：说一句、查一下、再说一句。SSE 上 `delta` 与
                // `step` 本来就是交替发出去的，顺序不用额外记；而**历史回放没有
                // 那条时间线**——落库的只有拼好的整段正文和一个扁平的 steps 数组，
                // 于是重新打开一场对话，所有调用都堆在正文最前面，读起来像是
                // 先查了七次再一口气说完。记下偏移，回放才能把话再断开。
                //
                // 单位是 **UTF-16 码元**，因为切分发生在浏览器里，而 JS 的
                // `String.prototype.length` 数的就是它。用字节数或 `chars()`
                // 在中文和 emoji 上都会切歪
                let mut step = step;
                if let Some(obj) = step.as_object_mut() {
                    obj.insert("at".into(), json!(answer_acc.encode_utf16().count()));
                }
                steps_acc.push(step.clone());
                yield Frame::new("step", serde_json::to_string(&step).unwrap_or_default());
                if step["kind"] == "search" || step["kind"] == "docs" {
                    yield Frame::new(
                        "sources",
                        serde_json::to_string(&sink.sources).unwrap_or_else(|_| "[]".into()),
                    );
                }
                let result_msg = tool_result_message(&call.id, &result);
                exchange_acc.push(result_msg.clone());
                msgs.push(result_msg);
            }
            rounds += 1;
        }
    };

    // 生成登记在案，然后**这条连接也只是去「接上」它**——与刷新之后
    // 那条重连走的是同一段代码。两条路分开写的话，迟早只有一条是对的
    let handle = live.begin(conversation_id).await;
    let attached = live.attach(conversation_id).await;
    tokio::spawn(async move {
        let mut producer = std::pin::pin!(producer);
        while let Some(frame) = producer.next().await {
            // 没有订阅者是常态（人走了）。**照发不误**：这里中断就等于
            // 把「切走一次丢一个回答」原样搬回来
            handle.emit(frame).await;
        }
        // 注销之后再接上的人得到「没有在跑的」，那时答案已经落库
        handle.finish().await;
    });

    Ok(sse_from(attached))
}

/// 把一次「接上」变成 SSE：先补一份快照，再照常收增量。
///
/// `None` = 这个会话没有在跑的生成。回一条 `idle` 而不是 404——**客户端
/// 每次打开会话都会问一次**，而「没有在跑」是最常见的答案，不是错误
fn sse_from(
    attached: Option<(
        crate::live::Snapshot,
        tokio::sync::broadcast::Receiver<Frame>,
    )>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = async_stream::stream! {
        let Some((snapshot, mut rx)) = attached else {
            yield to_event(&Frame::new("idle", "{}".into()));
            return;
        };
        yield to_event(&snapshot.to_frame());
        loop {
            match rx.recv().await {
                Ok(frame) => {
                    let done = frame.event == "done" || frame.event == "error";
                    yield to_event(&frame);
                    if done { return; }
                }
                // 生成结束、发送端销毁：正常收尾
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                // 这个客户端读得太慢，被广播缓冲甩下了。**说出来**——
                // 静默继续会让它少掉中间一段而毫不知情
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    yield to_event(&Frame::new(
                        "error",
                        format!("Fell behind the stream by {n} messages; reopen the conversation"),
                    ));
                    return;
                }
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn to_event(frame: &Frame) -> Result<Event, Infallible> {
    Ok(Event::default().event(frame.event).data(&frame.data))
}

/// 接上一次正在跑的生成（刷新页面之后走这里）。
pub async fn reattach(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, conversation_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    // **归属要查。** 会话 id 是可以猜的，而这条流会把别人的回答一字不差地念出来
    utopia_store::conversations::require_owned(&state.pool, kb_id, user.id, conversation_id)
        .await?;
    Ok(sse_from(state.live.attach(conversation_id).await))
}

/// 生成器产出的是 `Frame`，不是 `axum` 的 `Event`。
/// **广播与快照都要读回事件的内容**，而 `Event` 读不回来（见 `live`）
fn delta_event(text: &str) -> Frame {
    Frame::new(
        "delta",
        serde_json::to_string(&json!({ "text": text })).unwrap_or_default(),
    )
}

fn done_event() -> Frame {
    Frame::new("done", "{}".into())
}

fn error_event(message: &str) -> Frame {
    Frame::new("error", message.into())
}

fn source_json(n: usize, c: &ChunkView) -> serde_json::Value {
    json!({
        "n": n,
        "chunk_id": c.id,
        "document_id": c.document_id,
        "filename": c.filename,
        "excerpt": truncate(&c.text, 160),
    })
}

/// 降级路径的系统提示（tool-calling 不可用时的一次性注入）。
fn legacy_system_prompt(chunks: &[ChunkView]) -> String {
    if chunks.is_empty() {
        return "You are an enterprise knowledge base assistant. No relevant sources were \
                retrieved for this question. Tell the user the knowledge base lacks material \
                on this topic, answer cautiously from general knowledge, and clearly separate \
                sourced statements from speculation. Always respond in the same language as \
                the user's question."
            .to_string();
    }
    let mut prompt = String::from(
        "You are an enterprise knowledge base assistant. Answer strictly based on the \
         numbered sources below. When a source supports a statement, append its citation \
         number, e.g. [1] or [2], at the end of the sentence. If the sources are \
         insufficient, say so explicitly — never fabricate. Always respond in the same \
         language as the user's question.\n\n### Sources\n",
    );
    for (i, c) in chunks.iter().enumerate() {
        prompt.push_str(&format!(
            "\n[{}] \"{}\" section {}:\n{}\n",
            i + 1,
            c.filename,
            c.seq + 1,
            c.text
        ));
    }
    prompt
}

fn truncate(text: &str, max_chars: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        let cut: String = t.chars().take(max_chars).collect();
        format!("{cut}…")
    }
}

// ---------------------------------------------------------------------------
// 会话管理（列表 / 回放 / 删除）
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub struct ConversationsQuery {
    /// 搜标题与消息正文两处：人记得住的往往是问过的那句话，不是标题
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

pub async fn list_conversations(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<ConversationsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    let (conversations, total) = utopia_store::conversations::list(
        &state.pool,
        kb_id,
        user.id,
        q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        q.limit.unwrap_or(30).clamp(1, 100),
        q.offset.unwrap_or(0).max(0),
    )
    .await?;
    Ok(Json(
        json!({ "conversations": conversations, "total": total }),
    ))
}

#[derive(serde::Deserialize)]
pub struct RenameConversationReq {
    pub title: String,
}

/// 改会话标题。
///
/// **标题本来是从第一句话自动取的**，而一段对话跑偏是常态——改名让人能按
/// 自己记得的方式找回它。
pub async fn rename_conversation(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, conversation_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<RenameConversationReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    utopia_store::conversations::rename(&state.pool, kb_id, user.id, conversation_id, &req.title)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// 历史回放：消息含落库的行动轨迹（steps）与引用（sources）。
pub async fn conversation_detail(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, conversation_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    utopia_store::conversations::require_owned(&state.pool, kb_id, user.id, conversation_id)
        .await?;
    let messages = utopia_store::conversations::messages(&state.pool, conversation_id).await?;
    Ok(Json(json!({ "messages": messages })))
}

pub async fn delete_conversation(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((kb_id, conversation_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    utopia_store::conversations::delete(&state.pool, kb_id, user.id, conversation_id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- check_call ---------------------------------------------------------

    /// 参数在半路断掉——模型撞上 token 上限时就长这样。
    ///
    /// **从前这里回落成空对象，然后 `search_chunks` 拿用户那句原话去检索。**
    /// 得到的是一条看起来完全正常的 `search · 6 sources`，和一个基于错误输入
    /// 的答案。没人会去核一个看起来正常的答案，所以这一条必须是拒绝
    #[test]
    fn arguments_cut_off_mid_json_do_not_become_a_search() {
        let tools = tools_schema(false, &[]);
        let err = check_call(&tools, "search_chunks", "{\"query\": \"Acme reven")
            .expect_err("残缺 JSON 必须拒绝，而不是回落");
        assert_eq!(err.1["kind"], "tool", "轨迹上要显示成一次没做成的调用");
        assert_eq!(err.1["detail"], "bad arguments");
        assert!(
            err.0.contains("not valid JSON") && err.0.contains("again"),
            "回给模型的话要说清没执行、并让它重来：{}",
            err.0
        );
    }

    /// 空串与缺字段是同一件事：`{"query": ""}` 检索回来的东西与问题无关，
    /// 而它同样会显示成一条正常轨迹
    #[test]
    fn an_empty_required_argument_counts_as_missing() {
        let tools = tools_schema(false, &[]);
        for raw in [
            "{}",
            "{\"query\": \"\"}",
            "{\"query\": \"   \"}",
            "{\"query\": null}",
        ] {
            let Err(err) = check_call(&tools, "search_chunks", raw) else {
                panic!("{raw} 应当被拒");
            };
            assert_eq!(err.1["detail"], "missing query", "{raw}");
        }
    }

    /// **判据取自工具表本身。** 这条守的是「加了必填参数却忘了改校验」——
    /// query_data 只在挂了数据源时才出现在表里，它的两个必填参数
    /// 从没在别处被单独写过一遍
    #[test]
    fn the_schema_is_the_only_place_required_is_written_down() {
        let tools = tools_schema(false, &["warehouse".into()]);
        let err = check_call(&tools, "query_data", "{\"data_source\": \"warehouse\"}")
            .expect_err("缺 sql 必须拒绝");
        assert_eq!(err.1["detail"], "missing sql");
        check_call(
            &tools,
            "query_data",
            "{\"data_source\": \"warehouse\", \"sql\": \"SELECT 1\"}",
        )
        .expect("两个都给了就该放行");
    }

    /// 合规的调用原样通过，**可选参数一个不少**。
    ///
    /// 这一关只判「说清了没有」，不做过滤——把 args 重新组装一遍的话，
    /// 加一个可选参数就得记得来这里加一次，而忘记的后果是它安静地失效
    #[test]
    fn a_well_formed_call_passes_through_untouched() {
        let tools = tools_schema(false, &[]);
        let args = check_call(
            &tools,
            "entity_facts",
            "{\"entity_id\": \"1f8ac10b-58cc-4372-a567-0e02b2c3d479\", \"at\": \"2026-03-15\"}",
        )
        .expect("必填给了就该放行");
        assert_eq!(args["at"], "2026-03-15", "可选参数不能在这一关被吃掉");
    }

    /// 全文只能按 id 读，而 id 是模型从检索结果里抄来的——抄错、抄漏
    /// 都得在这里停住
    #[test]
    fn get_document_without_an_id_does_not_run() {
        let tools = tools_schema(false, &[]);
        let err = check_call(&tools, "get_document", "{}").expect_err("缺 document_id 必须拒绝");
        assert_eq!(err.1["detail"], "missing document_id");
    }

    /// 不是 uuid 的 id 到了工具里只能回「本库没有这篇文档」——模型会把它读成
    /// 「库里真的没有」而收手，于是一次抄错的 id 变成一个否定的答案
    #[test]
    fn a_document_id_that_is_not_a_uuid_does_not_run() {
        let tools = tools_schema(false, &[]);
        for raw in [
            "{\"document_id\": \"Notes- 'Scrum' 3 Sept 2026.txt\"}",
            "{\"document_id\": \"1f8ac10b-58cc-4372\"}",
        ] {
            let Err(err) = check_call(&tools, "get_document", raw) else {
                panic!("{raw} 应当被拒");
            };
            assert_eq!(err.1["detail"], "invalid document_id", "{raw}");
        }
        check_call(
            &tools,
            "get_document",
            "{\"document_id\": \"1f8ac10b-58cc-4372-a567-0e02b2c3d479\"}",
        )
        .expect("真的 uuid 就该放行");
    }

    /// `changes` 早就在自己那一支里拒绝缺失的 `since`——**它是唯一做对的一个**。
    /// 这条钉住两件事：新的统一关卡与它一致，而它自己那道对日期格式的检查
    /// （`2026-13-45` 这种）仍然要留着，因为 check_call 只看有没有、不看对不对
    #[test]
    fn the_one_tool_that_already_refused_still_refuses() {
        let tools = tools_schema(false, &[]);
        assert!(check_call(&tools, "changes", "{}").is_err());
        check_call(&tools, "changes", "{\"since\": \"2026-13-45\"}")
            .expect("格式错的日期不归这一关管，交给 changes_window");
    }
}

#[cfg(test)]
mod stall_tests {
    use super::{after_nudge, answer_rests_on_nothing, AfterNudge};
    use utopia_llm::{AssistantTurn, ToolCall};

    fn says(text: Option<&str>) -> AssistantTurn {
        AssistantTurn {
            content: text.map(String::from),
            tool_calls: Vec::new(),
        }
    }

    /// 整场没调过工具、没有引用，才算「什么都不站在上面」；任一边有东西都不追问
    #[test]
    fn an_answer_with_a_step_or_a_source_is_not_questioned() {
        let step = serde_json::json!({ "kind": "search" });
        let source = serde_json::json!({ "n": 1 });
        assert!(answer_rests_on_nothing(&[], &[]));
        assert!(!answer_rests_on_nothing(std::slice::from_ref(&step), &[]));
        assert!(!answer_rests_on_nothing(&[], std::slice::from_ref(&source)));
    }

    /// 追问后调了工具就接着走，不管它顺带说了什么
    #[test]
    fn a_tool_call_after_the_nudge_carries_on() {
        let turn = AssistantTurn {
            content: Some("Searching now.".into()),
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "search_chunks".into(),
                arguments: "{}".into(),
            }],
        };
        assert_eq!(after_nudge(&turn), AfterNudge::Tools);
    }

    /// DONE 怎么写都算：大小写、句号、前后空白；什么都没说也算——没有可补的
    #[test]
    fn done_in_any_dress_keeps_the_answer() {
        for text in ["DONE", "done", " Done. ", "DONE!", ""] {
            assert_eq!(after_nudge(&says(Some(text))), AfterNudge::Done, "{text:?}");
        }
        assert_eq!(after_nudge(&says(None)), AfterNudge::Done);
    }

    /// 再来一段文字，不管哪种语言、说得多客气，都是又停住了
    #[test]
    fn more_prose_after_the_nudge_is_a_second_stall() {
        for text in [
            "稍等，我正在搜索OpenAI的时间线信息。",
            "Let me search for that now.",
            "Done searching, here is the timeline: ...",
        ] {
            assert_eq!(
                after_nudge(&says(Some(text))),
                AfterNudge::Stalled,
                "{text:?}"
            );
        }
    }
}

#[cfg(test)]
#[path = "chat_empty_reply_tests.rs"]
mod chat_empty_reply_tests;
