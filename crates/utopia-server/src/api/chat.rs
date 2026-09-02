//! Agentic 对话：模型自主调用工具（文档检索 / 实体查找 / 时态事实）收集证据后作答。
//! 事件序列：step*（行动轨迹）| sources（引用清单，随检索增量更新）| delta*（增量文本）→ done | error。
//! 模型不支持 tool-calling 时自动降级为一次性 RAG 注入。

use crate::live::Frame;
use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use utopia_core::models::{ChunkView, EntityFact, GraphChange, Role};
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
const SEARCH_TOP_K: usize = 6;
const TOOL_CHUNK_CHARS: usize = 800;
/// 一次 changes 最多回多少条。刚灌完的库里 asserted 是成百上千条，全发出去
/// 只会把上下文填满而不增加信息——有信息量的是 corrected/rejected，那类事件
/// 本来就稀少。截断时 detail 写 "40+"，模型据此知道该收窄窗口
const CHANGES_LIMIT: i64 = 40;

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
        &json!([]),
        &json!([]),
        &json!([]),
    )
    .await?;
    let (history, known_entities) = utopia_store::conversations::recent_context(
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
        for (role, content) in &history {
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
        if !known_entities.is_empty() {
            let lines: Vec<String> = known_entities
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

        // 引用清单：跨轮累积、去重、编号稳定。键 = chunk uuid 或 "charter:{slug}#{anchor}"
        let mut source_ids: Vec<String> = Vec::new();
        let mut sources: Vec<serde_json::Value> = Vec::new();
        // 落库累积：assistant 全文与行动轨迹（历史回放用）
        let mut answer_acc = String::new();
        let mut steps_acc: Vec<serde_json::Value> = Vec::new();
        // 这一轮认下的实体，落库供下一轮回放。**只记身份不记正文**——
        // 工具结果里是 chunk 全文，每轮重复堆进上下文几轮就把窗口吃光
        let mut resolved_acc: Vec<serde_json::Value> = Vec::new();

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
                            &serde_json::Value::Array(steps_acc.clone()),
                            &serde_json::Value::Array(sources.clone()),
                            &serde_json::Value::Array(resolved_acc.clone()),
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
                        for (role, content) in &history {
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
                                    &serde_json::Value::Array(steps_acc.clone()),
                                    &serde_json::Value::Array(legacy_sources.clone()),
                                    &serde_json::Value::Array(resolved_acc.clone()),
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
                        &serde_json::Value::Array(steps_acc.clone()),
                        &serde_json::Value::Array(sources.clone()),
                        &serde_json::Value::Array(resolved_acc.clone()),
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

            msgs.push(turn.to_message());
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
                let (result, step) = match call.name.as_str() {
                    "search_chunks" => {
                        // check_call 已经保证 query 在且非空——这里不再有回落
                        let q = args["query"].as_str().unwrap_or_default().to_string();
                        let chunks = retrieval::hybrid(&state, kb_id, workspace_id, &q, SEARCH_TOP_K)
                            .await
                            .unwrap_or_default();
                        let mut lines = Vec::new();
                        for c in &chunks {
                            let key = c.id.to_string();
                            let n = match source_ids.iter().position(|id| *id == key) {
                                Some(i) => i + 1,
                                None => {
                                    source_ids.push(key);
                                    sources.push(source_json(source_ids.len(), c));
                                    source_ids.len()
                                }
                            };
                            lines.push(format!(
                                "[{n}] \"{}\" section {}:\n{}",
                                c.filename,
                                c.seq + 1,
                                truncate(&c.text, TOOL_CHUNK_CHARS)
                            ));
                        }
                        let text = if lines.is_empty() {
                            "No results.".to_string()
                        } else {
                            lines.join("\n\n")
                        };
                        (text, json!({ "kind": "search", "label": q, "detail": format!("{} sources", chunks.len()) }))
                    }
                    "search_docs" => {
                        // 同上：check_call 已经拦过，这里不再回落到用户原话
                        let q = args["query"].as_str().unwrap_or_default().to_string();
                        let hits = state.docs.search(&q, 4).unwrap_or_default();
                        let mut lines = Vec::new();
                        for h in &hits {
                            let key = format!("charter:{}#{}", h.slug, h.anchor);
                            let n = match source_ids.iter().position(|id| *id == key) {
                                Some(i) => i + 1,
                                None => {
                                    source_ids.push(key);
                                    sources.push(charter_source_json(source_ids.len(), h));
                                    source_ids.len()
                                }
                            };
                            lines.push(format!(
                                "[{n}] Utopia Charter — {} › {}:\n{}",
                                h.title,
                                h.heading,
                                truncate(&h.body, 1600)
                            ));
                        }
                        let text = if lines.is_empty() {
                            "No matching manual sections.".to_string()
                        } else {
                            lines.join("\n\n")
                        };
                        (text, json!({ "kind": "docs", "label": q, "detail": format!("{} sections", hits.len()) }))
                    }
                    "find_entities" => {
                        let name = args["name"].as_str().unwrap_or("").to_string();
                        let (hits, _) =
                            utopia_store::graph::search_entities(&state.pool, kb_id, &name, 8, 0)
                            .await
                            .unwrap_or_default();
                        let text = if hits.is_empty() {
                            "No matching entities.".to_string()
                        } else {
                            hits.iter()
                                .map(|n| {
                                    let dis = n
                                        .disambiguator
                                        .as_deref()
                                        .map(|d| format!(" ({d})"))
                                        .unwrap_or_default();
                                    format!(
                                        "{} | {}{} | {} | {} facts",
                                        n.id,
                                        n.name,
                                        dis,
                                        // 没判出类型的实体照样能被搜到、被引用（0009）
                                        n.type_label.as_deref().unwrap_or("untyped"),
                                        n.degree
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        };
                        for n in &hits {
                            resolved_acc.push(json!({
                                "id": n.id.to_string(), "name": n.name, "type": n.type_label
                            }));
                        }
                        (text, json!({ "kind": "entity", "label": name, "detail": format!("{} matches", hits.len()) }))
                    }
                    "entity_facts" => {
                        let id = args["entity_id"].as_str().and_then(|s| s.parse::<Uuid>().ok());
                        // as-of 过滤：T 时刻有效 = 起点不晚于 T（或未知）且终点晚于 T（或开放）
                        let at = args["at"]
                            .as_str()
                            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
                            .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc());
                        match id {
                            Some(id) => match utopia_store::graph::entity_detail(&state.pool, kb_id, id).await {
                                Ok((node, mut facts)) => {
                                    if let Some(t) = at {
                                        facts.retain(|f| {
                                            f.valid_from.is_none_or(|from| from <= t)
                                                && f.valid_to.is_none_or(|to| to > t)
                                        });
                                    }
                                    let text = if facts.is_empty() {
                                        match at {
                                            Some(t) => format!(
                                                "{}: no facts valid as of {}.",
                                                node.name,
                                                t.format("%Y-%m-%d")
                                            ),
                                            None => format!("{}: no recorded facts.", node.name),
                                        }
                                    } else {
                                        facts.iter().map(fact_line).collect::<Vec<_>>().join("\n")
                                    };
                                    let detail = match at {
                                        Some(t) => format!(
                                            "{} facts as of {}",
                                            facts.len(),
                                            t.format("%Y-%m-%d")
                                        ),
                                        None => format!("{} facts", facts.len()),
                                    };
                                    (text, json!({ "kind": "facts", "label": node.name, "detail": detail }))
                                }
                                Err(_) => (
                                    "Entity not found.".to_string(),
                                    json!({ "kind": "facts", "label": "?", "detail": "not found" }),
                                ),
                            },
                            None => (
                                "Invalid entity_id (expected the uuid returned by find_entities)."
                                    .to_string(),
                                json!({ "kind": "facts", "label": "?", "detail": "invalid id" }),
                            ),
                        }
                    }
                    "changes" => {
                        let day = |k: &str| {
                            args[k]
                                .as_str()
                                .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
                        };
                        match changes_window(day("since"), day("until"), chrono::Utc::now()) {
                            None => (
                                "Invalid or missing `since` (expected YYYY-MM-DD).".to_string(),
                                json!({ "kind": "changes", "label": "?", "detail": "invalid since" }),
                            ),
                            Some((since, until, window)) => {
                                let entity = args["entity_id"]
                                    .as_str()
                                    .and_then(|s| s.parse::<Uuid>().ok());
                                let kinds: Option<Vec<String>> = args["kinds"].as_array().map(|a| {
                                    a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
                                });
                                let kinds = kinds.filter(|k: &Vec<String>| !k.is_empty());
                                let rows = utopia_store::graph::graph_changes(
                                    &state.pool,
                                    kb_id,
                                    since,
                                    until,
                                    entity,
                                    kinds.as_deref(),
                                    CHANGES_LIMIT,
                                )
                                .await
                                .unwrap_or_default();
                                let text = if rows.is_empty() {
                                    format!("No recorded changes in {window}.")
                                } else {
                                    rows.iter().map(change_line).collect::<Vec<_>>().join("\n")
                                };
                                let detail = if rows.len() as i64 == CHANGES_LIMIT {
                                    format!("{CHANGES_LIMIT}+ changes")
                                } else {
                                    format!("{} changes", rows.len())
                                };
                                (text, json!({ "kind": "changes", "label": window, "detail": detail }))
                            }
                        }
                    }
                    "query_data" if !mounted_sources.is_empty() => {
                        let ds_name = args["data_source"].as_str().map(str::trim).unwrap_or("");
                        let sql = args["sql"].as_str().map(str::trim).unwrap_or("");
                        let purpose = args["purpose"].as_str().map(str::trim).unwrap_or("");
                        // 安全边界：只允许本 KB 挂载的源（凭据不出服务端）
                        let found = mounted_sources
                            .iter()
                            .find(|d| d.name.eq_ignore_ascii_case(ds_name));
                        let text = match found {
                            None => format!(
                                "Unknown data source '{ds_name}'. Mounted sources: {}",
                                mounted_sources
                                    .iter()
                                    .map(|d| d.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            Some(ds) => {
                                match run_query(&state, ds.id, sql).await {
                                    Ok(out) => out,
                                    // 错误透传：模型可据此修正 SQL 重试
                                    Err(e) => format!("Query failed: {e}"),
                                }
                            }
                        };
                        let detail = if purpose.is_empty() {
                            sql.chars().take(60).collect::<String>()
                        } else {
                            purpose.to_string()
                        };
                        (text, json!({ "kind": "query", "label": ds_name, "detail": detail }))
                    }
                    "remember" if can_write => {
                        let text = args["text"].as_str().map(str::trim).unwrap_or("");
                        let occurred_at = args["occurred_at"]
                            .as_str()
                            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
                            .map(|d| d.and_hms_opt(12, 0, 0).unwrap().and_utc())
                            .unwrap_or_else(chrono::Utc::now);
                        if text.is_empty() {
                            (
                                "remember requires non-empty text.".to_string(),
                                json!({ "kind": "tool", "label": "remember", "detail": "empty" }),
                            )
                        } else {
                            match utopia_store::memory::append_episode(
                                &state.pool, kb_id, text, occurred_at,
                            )
                            .await
                            {
                                Ok((doc_id, _chunk)) => {
                                    // 摄入(embedding/索引/增量抽取)异步走队列，不阻塞对话
                                    let _ = utopia_store::jobs::enqueue(
                                        &state.pool,
                                        "memory_ingest",
                                        json!({ "document_id": doc_id }),
                                    )
                                    .await;
                                    state.emit_document(kb_id, doc_id);
                                    (
                                        format!(
                                            "Recorded (effective {}): {text}",
                                            occurred_at.format("%Y-%m-%d")
                                        ),
                                        json!({
                                            "kind": "tool", "label": "remember",
                                            "detail": text.chars().take(60).collect::<String>()
                                        }),
                                    )
                                }
                                Err(e) => (
                                    format!("Failed to record: {e}"),
                                    json!({ "kind": "tool", "label": "remember", "detail": "failed" }),
                                ),
                            }
                        }
                    }
                    other => (
                        format!("Unknown tool: {other}"),
                        json!({ "kind": "tool", "label": other, "detail": "unknown" }),
                    ),
                };
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
                        serde_json::to_string(&sources).unwrap_or_else(|_| "[]".into()),
                    );
                }
                msgs.push(tool_result_message(&call.id, &result));
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

/// Charter 引用：前端渲染成手册行，链到 /docs/{slug}#{anchor}。
fn charter_source_json(n: usize, h: &utopia_search::DocsSection) -> serde_json::Value {
    json!({
        "n": n,
        "kind": "charter",
        "slug": h.slug,
        "anchor": h.anchor,
        "heading": h.heading,
        "filename": h.title,
        "excerpt": truncate(&h.body, 160),
    })
}

/// 事实行："works at → 星云科技 (2023-08 → now) [90%]"，in 方向用 ←。
/// 问数执行：安全闸（解析白名单）→ 引擎执行（只读会话 + 强制 LIMIT + 超时）→ JSON 行。
async fn run_query(state: &AppState, ds_id: Uuid, sql: &str) -> anyhow::Result<String> {
    let guarded = crate::query_engine::guard_sql(sql)?;
    let (engine, conn) = utopia_store::datasources::engine_and_conn(&state.pool, ds_id).await?;
    let result = crate::query_engine::engine_for(&engine, &conn)?
        .execute(&guarded)
        .await?;
    let mut out = String::new();
    if result.rows.is_empty() {
        out.push_str("(no rows)");
    } else {
        out.push_str(&result.rows.join("\n"));
        out.push_str(&format!("\n({} rows", result.rows.len()));
        if result.truncated {
            out.push_str(&format!(
                ", truncated at {} — aggregate in SQL for totals",
                crate::query_engine::ROW_CAP
            ));
        }
        out.push(')');
    }
    Ok(out)
}

fn fact_line(f: &EntityFact) -> String {
    let other = f.other_name.as_deref().unwrap_or("?");
    // 本体没认下、原文说法也没留下时用 "?"——与 other 同一个约定。
    // 不编一个"相关"出来：那正是删掉 related_to 要消灭的东西
    let pred = f.predicate_label.as_deref().unwrap_or("?");
    let core = if f.direction == "out" {
        format!("{pred} → {other}")
    } else {
        format!("{pred} ← {other}")
    };
    let range = match (&f.valid_from, &f.valid_to) {
        (Some(from), Some(to)) => {
            format!(" ({} → {})", from.format("%Y-%m-%d"), to.format("%Y-%m-%d"))
        }
        (Some(from), None) => format!(" ({} → now)", from.format("%Y-%m-%d")),
        (None, Some(to)) => format!(" (→ {})", to.format("%Y-%m-%d")),
        (None, None) => String::new(),
    };
    format!("{core}{range} [{}%]", (f.confidence * 100.0).round() as i32)
}

/// changes 的时间窗：把两个可选日期变成 (SQL 用的半开区间, 展示用的窗口串)。
///
/// **抽成纯函数是因为这里出过一次错。** `until` 进 SQL 前要加一天（说"到 3 月 31 日
/// 为止"的人要的是含 31 日，而 SQL 那头是 `< $3`），第一版把加过一天的值也印进了
/// 展示串，模型于是照着答"截至 8 月 30 日"——问的是 29 日。两个值必须一起算、
/// 一起被测住；分在两处写，迟早再次分叉。
///
/// `now` 从外面传进来而不是在里面取，纯粹是为了这个函数测得动。
fn changes_window(
    since: Option<chrono::NaiveDate>,
    until: Option<chrono::NaiveDate>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<(
    chrono::DateTime<chrono::Utc>,
    chrono::DateTime<chrono::Utc>,
    String,
)> {
    let from = since?;
    let start = from.and_hms_opt(0, 0, 0).unwrap().and_utc();
    let end = until
        .and_then(|d| d.succ_opt())
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
        .unwrap_or(now);
    // 展示用的是**问的那天**，没问就写 now——绝不是 end
    let label = format!(
        "{} → {}",
        from.format("%Y-%m-%d"),
        match until {
            Some(d) => d.format("%Y-%m-%d").to_string(),
            None => "now".to_string(),
        }
    );
    Some((start, end, label))
}

/// 认知轴上的一行。
///
/// 排版把两根轴**分开写**：`at` 前面标事件类型，世界轴的区间夹在括号里跟在断言后。
/// 若把它们排成一串日期，模型会把"2026 年记下的"读成"2026 年发生的"——那正是
/// 这个工具要防的误读。
fn change_line(c: &GraphChange) -> String {
    let object = match (&c.object_name, &c.object_value) {
        (Some(name), _) => name.clone(),
        (None, Some(v)) => match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        },
        (None, None) => "?".to_string(),
    };
    let range = match (&c.valid_from, &c.valid_to) {
        (Some(from), Some(to)) => {
            format!(
                " [valid {} → {}]",
                from.format("%Y-%m-%d"),
                to.format("%Y-%m-%d")
            )
        }
        (Some(from), None) => format!(" [valid {} → now]", from.format("%Y-%m-%d")),
        (None, Some(to)) => format!(" [valid → {}]", to.format("%Y-%m-%d")),
        (None, None) => String::new(),
    };
    // 文件名不带 [n]：引证编号是 chunk 的，这里只有 document，发一个编号出去
    // 会在界面上落成一条指不到东西的引证
    let src = match (&c.filename, &c.quote) {
        (Some(f), Some(q)) => format!(" — from \"{f}\": \"{}\"", truncate(q, 160)),
        (Some(f), None) => format!(" — from \"{f}\""),
        _ => String::new(),
    };
    format!(
        "{} {}: {} {} {}{}{}",
        c.at.format("%Y-%m-%d"),
        c.kind,
        c.subject_name,
        c.predicate_label.as_deref().unwrap_or("?"),
        object,
        range,
        src
    )
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

    fn d(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }
    fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
        s.parse().unwrap()
    }

    // --- changes_window -----------------------------------------------------

    /// 说"到 3 月 31 日为止"的人要的是**含 31 日**。SQL 那头是 `< end`，
    /// 所以 end 必须落在 4 月 1 日零点——差这一天，问 31 日会安静地丢掉 31 日
    #[test]
    fn until_names_a_day_the_window_must_contain() {
        let (start, end, label) = changes_window(
            Some(d("2026-03-01")),
            Some(d("2026-03-31")),
            t("2026-06-01T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(start, t("2026-03-01T00:00:00Z"));
        assert_eq!(end, t("2026-04-01T00:00:00Z"));
        // 但**展示串里绝不能出现 04-01**：那是半开区间的内部细节，
        // 印出去等于告诉模型窗口比它要的宽一天，模型会照着答（这条真的发生过）
        assert_eq!(label, "2026-03-01 → 2026-03-31");
    }

    /// 没给 until 时，展示串写 now。写成 end 的格式化结果就等于把服务器时钟
    /// 当成用户问的边界——看着像个精确答案，其实只是"现在几点"
    #[test]
    fn an_open_window_says_now_rather_than_the_clock() {
        let now = t("2026-06-01T13:45:00Z");
        let (_, end, label) = changes_window(Some(d("2026-03-01")), None, now).unwrap();
        assert_eq!(end, now);
        assert_eq!(label, "2026-03-01 → now");
    }

    #[test]
    fn without_since_there_is_no_window() {
        assert!(changes_window(None, Some(d("2026-03-31")), t("2026-06-01T00:00:00Z")).is_none());
    }

    // --- change_line --------------------------------------------------------

    fn change(kind: &str) -> GraphChange {
        GraphChange {
            fact_id: Uuid::nil(),
            at: t("2026-08-28T10:00:00Z"),
            kind: kind.to_string(),
            subject_id: Uuid::nil(),
            subject_name: "Acme".to_string(),
            predicate_label: Some("founded in".to_string()),
            object_name: None,
            object_value: None,
            valid_from: None,
            valid_to: None,
            // 两端都没日期就没有精度——夹具也得守这条不变量（见 `facts.valid_from_precision`）
            // 两端都没日期，所以两端都没有精度（见 facts 的两个精度列）
            valid_from_precision: None,
            valid_to_precision: None,
            confidence: 0.9,
            document_id: None,
            filename: None,
            quote: None,
        }
    }

    /// 字面值要读成它自己。走 `Value::to_string()` 会把字符串连引号一起印出来，
    /// 模型于是把 `"1993"` 当成答案的一部分
    #[test]
    fn a_literal_value_reads_as_itself_not_as_json() {
        let mut c = change("asserted");
        c.object_value = Some(serde_json::json!("1993"));
        assert!(
            change_line(&c).ends_with("Acme founded in 1993"),
            "{}",
            change_line(&c)
        );
    }

    /// 两根轴必须在同一行里**看得出是两样东西**：记录时刻在前、无标签，
    /// 世界轴区间在后、带 `valid` 字样。排成一串裸日期，模型会把
    /// "2026 年记下的"读成"2026 年发生的"——这个工具存在的理由就是防这个
    #[test]
    fn the_record_time_and_the_valid_range_do_not_read_as_one_date_run() {
        let mut c = change("corrected");
        c.object_name = Some("Berlin".to_string());
        c.predicate_label = Some("headquartered in".to_string());
        c.valid_from = Some(t("2019-01-01T00:00:00Z"));
        let line = change_line(&c);
        assert!(line.starts_with("2026-08-28 corrected: "), "{line}");
        assert!(line.contains("[valid 2019-01-01 → now]"), "{line}");
    }

    /// 没有证据就什么都别说。凭空补一句 from "…" 会让一条无出处的断言
    /// 看起来有出处
    #[test]
    fn a_fact_with_no_evidence_claims_no_document() {
        let mut c = change("rejected");
        c.object_name = Some("Berlin".to_string());
        assert!(!change_line(&c).contains("from"), "{}", change_line(&c));
    }

    #[test]
    fn evidence_carries_the_filename_and_the_quote() {
        let mut c = change("corrected");
        c.object_name = Some("Berlin".to_string());
        c.filename = Some("annual-report.pdf".to_string());
        c.quote = Some("moved its head office to Berlin".to_string());
        let line = change_line(&c);
        assert!(line.contains("from \"annual-report.pdf\""), "{line}");
        assert!(
            line.contains("\"moved its head office to Berlin\""),
            "{line}"
        );
    }

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
