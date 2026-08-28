//! Agentic 对话：模型自主调用工具（文档检索 / 实体查找 / 时态事实）收集证据后作答。
//! 事件序列：step*（行动轨迹）| sources（引用清单，随检索增量更新）| delta*（增量文本）→ done | error。
//! 模型不支持 tool-calling 时自动降级为一次性 RAG 注入。

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use utopia_core::models::{ChunkView, EntityFact, Role};
use utopia_core::AppError;
use utopia_llm::tool_result_message;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::llm_util;
use crate::retrieval;
use crate::state::AppState;

const MAX_HISTORY: usize = 20;
const MAX_ROUNDS: usize = 6;
const SEARCH_TOP_K: usize = 6;
const TOOL_CHUNK_CHARS: usize = 800;

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
        }
    ])
}

const SYSTEM_PROMPT: &str = "You are the assistant of Utopia, a temporal knowledge platform. \
    You have tools: search_chunks (document search), find_entities and entity_facts (a \
    bi-temporal knowledge graph where every relation carries a validity range), and \
    search_docs (Utopia's own manual, the Charter).\n\
    Boundary: search_docs answers questions about Utopia itself (features, ingestion, \
    permissions, what fields like 'missing' or validity ranges mean); the other tools answer \
    questions about the knowledge stored in it. Never mix the manual into answers about the \
    user's data unless they asked about Utopia's behavior.\n\
    \n\
    Method:\n\
    1. For factual questions, ALWAYS gather evidence with tools before answering. Prefer the \
       graph tools for questions about people/organizations/projects and time (\"who was X \
       when\", \"what changed\"), search_chunks for content and detail questions. Combine both \
       when useful.\n\
    2. Facts carry validity ranges (from → to). For \"as of <date>\" questions pass `at` to \
       entity_facts and the server filters to that moment; for history questions omit `at` \
       to see the full timeline. State dates in the answer.\n\
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
    // 语义层：人确认过的 指标/维度 → 数据资产 映射（置信 ≥0.75，与 Review 阈值互补），
    // 直接进 system prompt——问数优先用确认口径，而不是每次从 schema 猜
    let mappings = if mounted_sources.is_empty() {
        Vec::new()
    } else {
        utopia_store::graph::confirmed_mappings(&state.pool, kb_id, 0.75, 30).await?
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
    )
    .await?;
    let history: Vec<(String, String)> = utopia_store::conversations::recent_context(
        &state.pool,
        conversation_id,
        MAX_HISTORY as i64,
    )
    .await?;
    let workspace_id = kb.workspace_id;

    let stream = async_stream::stream! {
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
                for (name, def) in &mappings {
                    system_prompt.push_str(&format!("\n- {name}: {def}"));
                }
            }
        }
        let mut msgs: Vec<serde_json::Value> =
            vec![json!({ "role": "system", "content": system_prompt })];
        for (role, content) in &history {
            msgs.push(json!({ "role": role, "content": content }));
        }

        // 会话 id 先行下发（新会话由此告知前端）
        yield Ok(Event::default()
            .event("conversation")
            .data(json!({ "id": conversation_id }).to_string()));

        // 引用清单：跨轮累积、去重、编号稳定。键 = chunk uuid 或 "charter:{slug}#{anchor}"
        let mut source_ids: Vec<String> = Vec::new();
        let mut sources: Vec<serde_json::Value> = Vec::new();
        // 落库累积：assistant 全文与行动轨迹（历史回放用）
        let mut answer_acc = String::new();
        let mut steps_acc: Vec<serde_json::Value> = Vec::new();

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
                        yield Ok(Event::default().event("sources").data(
                            serde_json::to_string(&legacy_sources)
                                .unwrap_or_else(|_| "[]".into()),
                        ));
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
                let args: serde_json::Value =
                    serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
                let (result, step) = match call.name.as_str() {
                    "search_chunks" => {
                        let q = args["query"].as_str().unwrap_or(&query).to_string();
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
                        let q = args["query"].as_str().unwrap_or(&query).to_string();
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
                        let hits = utopia_store::graph::search_entities(&state.pool, kb_id, &name, 8)
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
                                        n.id, n.name, dis, n.type_label, n.degree
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        };
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
                steps_acc.push(step.clone());
                yield Ok(Event::default()
                    .event("step")
                    .data(serde_json::to_string(&step).unwrap_or_default()));
                if step["kind"] == "search" || step["kind"] == "docs" {
                    yield Ok(Event::default()
                        .event("sources")
                        .data(serde_json::to_string(&sources).unwrap_or_else(|_| "[]".into())));
                }
                msgs.push(tool_result_message(&call.id, &result));
            }
            rounds += 1;
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn delta_event(text: &str) -> Result<Event, Infallible> {
    Ok(Event::default()
        .event("delta")
        .data(serde_json::to_string(&json!({ "text": text })).unwrap_or_default()))
}

fn done_event() -> Result<Event, Infallible> {
    Ok(Event::default().event("done").data("{}"))
}

fn error_event(message: &str) -> Result<Event, Infallible> {
    Ok(Event::default().event("error").data(message))
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
    let core = if f.direction == "out" {
        format!("{} → {}", f.predicate_label, other)
    } else {
        format!("{} ← {}", f.predicate_label, other)
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

pub async fn list_conversations(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    let conversations = utopia_store::conversations::list(&state.pool, kb_id, user.id).await?;
    Ok(Json(json!({ "conversations": conversations })))
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
