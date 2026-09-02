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

#[derive(Deserialize)]
pub struct ChatReq {
    /// 缺省 = 新建会话（SSE 首个 `conversation` 事件回传 id）
    #[serde(default)]
    pub conversation_id: Option<Uuid>,
    pub message: String,
}

fn tools_schema(can_write: bool, data_source_names: &[String]) -> serde_json::Value {
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
                                "description": "One SELECT/WITH statement (PostgreSQL dialect)."
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
    if can_write {
        if let Some(arr) = tools.as_array_mut() {
            arr.push(json!({
                "type": "function",
                "function": {
                    "name": "remember",
                    "description": "Record one memory episode into the knowledge base's temporal \
                        memory. Use ONLY when the user explicitly asks to remember/record \
                        something, or clearly states a decision or fact to keep. The episode is \
                        extracted into the knowledge graph; if it contradicts an existing \
                        single-valued fact, the old fact's validity is closed automatically \
                        (never deleted). Do not use for casual conversation.",
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
                                    (YYYY-MM-DD). Omit to use today."
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
fn check_call(
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
    let required = tools
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["function"]["name"] == name)
        .and_then(|t| t["function"]["parameters"]["required"].as_array());
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
    }
    Ok(args)
}

const MEMORY_PROMPT: &str = "\
    Memory: you can persist knowledge with the remember tool. Use it when the user says \
    \"remember/record this\" or states a decision meant to last. Confirm in your reply what \
    was recorded. Never invent memories, and never call it for small talk.";

fn base_tools() -> serde_json::Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": "search_chunks",
                "description": "Full-text + semantic search over the knowledge base documents. \
                    Returns numbered source excerpts you can cite as [n].",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query, phrased in the corpus language." }
                    },
                    "required": ["query"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "search_docs",
                "description": "Search Utopia's own user manual (the Charter): how the platform \
                    itself works — sources and ingestion, sync semantics (missing markers, \
                    tombstones, versions), the knowledge graph and review flow, roles and \
                    permissions, settings. Use ONLY for questions about using or understanding \
                    Utopia itself, or to explain platform concepts that appear in other tool \
                    results (e.g. why a document is marked missing). NEVER use it to answer \
                    questions about the content stored in the knowledge base.",
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
                    always do this for \"who was X in <year/month>\" questions.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "entity_id": { "type": "string", "description": "Entity id (uuid) from find_entities." },
                        "at": {
                            "type": "string",
                            "description": "Optional as-of date (YYYY-MM-DD). Only facts valid on \
                                this date are returned. Omit for the full history."
                        }
                    },
                    "required": ["entity_id"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "changes",
                "description": "What the graph LEARNED or REVISED in a window of record time —                     the belief axis. Answers \"what changed since X\", \"what did we get wrong\",                     \"what is new this quarter\", and needs no entity, so use it when the                     question names a period rather than a subject.                     Events: asserted (new claim), corrected (a claim replaced by a revised one),                     rejected (a claim withdrawn), merged (folded into another claim) — each with                     the document it came from.                     NOT the same axis as entity_facts(at): that asks \"what was true on date D\";                     this asks \"what did we change our mind about between D1 and D2\". A fact                     about 2019 can be recorded in 2026 — this windows on when we recorded it.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "since": {
                            "type": "string",
                            "description": "Start of the window (YYYY-MM-DD), inclusive."
                        },
                        "until": {
                            "type": "string",
                            "description": "End of the window (YYYY-MM-DD), inclusive of that                                 whole day. Omit for 'up to now'."
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
    You have tools: search_chunks (document search), find_entities, entity_facts and \
    changes (a bi-temporal knowledge graph), and search_docs (Utopia's own manual, the \
    Charter).\n\
    The graph has TWO independent time axes, and each graph tool reads exactly one:\n\
    - World time — when something was true. Read with entity_facts (`at` = as of that date).\n\
    - Record time — when we came to believe it, and when we revised it. Read with changes.\n\
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
       conversation — ALWAYS gather evidence with tools before answering. Prefer the \
       graph tools for questions about people/organizations/projects and time (\"who was X \
       when\", \"what changed\"), search_chunks for content and detail questions. Combine both \
       when useful.\n\
    2. Facts carry validity ranges (from → to). For \"as of <date>\" questions pass `at` to \
       entity_facts and the server filters to that moment; for history questions omit `at` \
       to see the full timeline. State dates in the answer.\n\
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
    let mappings = if mounted_sources.is_empty() {
        Vec::new()
    } else {
        utopia_store::mappings::confirmed(&state.pool, kb_id, 30).await?
    };
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
        let mut system_prompt = if can_write {
            format!("{SYSTEM_PROMPT}\n{MEMORY_PROMPT}")
        } else {
            SYSTEM_PROMPT.to_string()
        };
        if !ds_names.is_empty() {
            system_prompt.push_str(&format!(
                "\nData: query_data runs read-only SQL (PostgreSQL dialect) against: {}. \
                 For questions about numbers/metrics, search for the source's schema document \
                 first, then query. State units and the time range you used in the answer.",
                ds_names.join(", ")
            ));
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
                            retrieval::hybrid(&state, kb_id, workspace_id, &query, 8)
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
            let Some(turn) = turn else {
                yield error_event("LLM stream ended unexpectedly");
                return;
            };

            if turn.tool_calls.is_empty() {
                if answer_acc.is_empty() {
                    yield error_event("Model returned an empty answer");
                } else {
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
                return;
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
                };
                let (result, step) = tools::dispatch(&ctx, &mut sink, &call.name, &args).await;
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
