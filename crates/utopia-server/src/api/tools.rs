//! 七个工具的**执行**，与谁在调用它们无关。
//!
//! 从前这些是 `chat.rs` 里一个 `match` 的七条分支，每条 20–70 行，闭包捕获着
//! 流式循环的局部变量。那样写在只有对话一个调用方时没问题——**而 MCP 是第二个**。
//! 抽出来之后两边共用同一份实现，不会出现「对话里的 entity_facts 和 MCP 里的
//! 不是同一个东西」。
//!
//! 工具定义（给模型看的 JSON schema）仍在 `chat.rs`：那是提示词的一部分，
//! 随对话策略走；这里只管拿到参数之后干什么。

use serde_json::json;
use utopia_core::models::{ChunkView, DataSourceView, EntityFact, GraphChange};
use uuid::Uuid;

use crate::retrieval;
use crate::state::AppState;

const SEARCH_TOP_K: usize = 6;
const TOOL_CHUNK_CHARS: usize = 800;
/// `get_document` 一次最多回多少字。检索那边的 800 字是为了让六条命中都装得下，
/// 这边只有一篇文档，而问的人已经知道要读哪一篇——截在答案前面才是这个工具
/// 要消灭的毛病
const DOCUMENT_CHARS: usize = 24_000;
/// 一次 changes 最多回多少条。刚灌完的库里 asserted 是成百上千条，全发出去
/// 只会把上下文填满而不增加信息——有信息量的是 corrected/rejected，那类事件
/// 本来就稀少。截断时 detail 写 "40+"，模型据此知道该收窄窗口
const CHANGES_LIMIT: i64 = 40;

/// 一次工具调用看得见的世界。**只读**——工具改不了它。
pub struct ToolCtx<'a> {
    pub state: &'a AppState,
    pub kb_id: Uuid,
    pub workspace_id: Uuid,
    /// 本库挂载的数据源。**`query_data` 的安全边界就是这个列表**：
    /// 凭据不出服务端，模型只能按名字点单
    pub mounted_sources: &'a [DataSourceView],
    /// editor 及以上才有 `remember`
    pub can_write: bool,
    /// 在说话的人。`remember` 把它记成「谁说的」，一路跟到待确认队列（0015）。
    /// 对话里恒有值；MCP 也有——令牌以人的身份行事（0014）
    pub actor: Option<Uuid>,
    /// 经 MCP 时，是哪一枚令牌在说话。**人之外还要记它**：一个人可以同时挂
    /// 三个 agent，审核卡上只写人名分不出是哪一个记的（0026）。对话里为 None
    pub via_token: Option<Uuid>,
    /// 用户这一轮的原话。图谱工具拿它消歧：同名候选按「谁的上下文画像离这个问题近」排。
    /// MCP 没有它（agent 的意图不在请求里），那边按事实数排
    pub question: Option<&'a str>,
}

impl ToolCtx<'_> {
    /// 一段文字的向量，用这个工作区配的嵌入模型。没配、或调用失败时 None：
    /// 图谱工具照常按子串走，向量只是第二阶段
    pub async fn embed(&self, text: &str) -> Option<Vec<f32>> {
        let settings = utopia_store::settings::get(&self.state.pool, self.workspace_id)
            .await
            .ok()
            .flatten()?;
        let client = crate::llm_util::embed_client(&settings)?;
        match client.embed(&[text.to_string()]).await {
            Ok(mut v) if !v.is_empty() => Some(v.remove(0)),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(error = %e, "嵌入失败，图谱工具按子串匹配");
                None
            }
        }
    }
}

/// 工具执行过程中往外攒的东西。
///
/// **引用编号是有状态的**：`[3]` 里的 3 取决于这一轮之前已经引过几个，
/// 所以不能让每个工具各算各的再合并——那样同一个 chunk 会拿到两个号。
#[derive(Default)]
pub struct ToolSink {
    /// 去重键（chunk uuid，或 `charter:{slug}#{anchor}`），下标 +1 就是引用号
    pub source_ids: Vec<String>,
    /// 发给前端的引用清单，与 `source_ids` 同序
    pub sources: Vec<serde_json::Value>,
    /// 这一轮认下的实体，落进会话供下一轮回放
    pub resolved: Vec<serde_json::Value>,
}

/// 一次调用的文本、界面步骤与可选机器读取结果；不放进跨调用累计的 ToolSink。
pub struct ToolResult {
    pub text: String,
    pub step: serde_json::Value,
    pub structured_content: Option<serde_json::Value>,
    pub is_error: bool,
}

impl ToolResult {
    pub fn new(text: String, step: serde_json::Value) -> Self {
        Self {
            text,
            step,
            structured_content: None,
            is_error: false,
        }
    }

    pub fn structured(mut self, content: serde_json::Value) -> Self {
        self.structured_content = Some(content);
        self
    }

    pub fn error(mut self) -> Self {
        self.is_error = true;
        self
    }
}

/// 按名字派发。**未知工具不是错误**——模型偶尔会编一个名字出来，
/// 告诉它没有这个工具，它下一轮就换一个，比中断整场对话好。
pub async fn dispatch(
    ctx: &ToolCtx<'_>,
    sink: &mut ToolSink,
    name: &str,
    args: &serde_json::Value,
) -> ToolResult {
    match name {
        "search_chunks" => search_chunks(ctx, sink, args).await,
        "get_document" => get_document(ctx, sink, args).await,
        "search_docs" => search_docs(ctx, sink, args).await,
        "find_entities" => super::tools_graph::find_entities(ctx, sink, args).await,
        "entity_facts" => super::tools_graph::entity_facts(ctx, sink, args).await,
        "neighbors" => super::tools_graph::neighbors(ctx, sink, args).await,
        "timeline" => super::tools_graph::timeline(ctx, sink, args).await,
        "paths_between" => super::tools_graph::paths_between(ctx, sink, args).await,
        "changes" => changes(ctx, args).await,
        // 业务规则只读（0021）：判据要看得见，但**写规则不开给模型**——
        // 「推理的判据由人写」是 0002 与 0021 共同的那条线，而一个工具调用
        // 分不出「人口述、agent 代打」与「模型自己编了一条」
        "list_rules" => list_rules(ctx).await,
        "rule_matches" => rule_matches(ctx, args).await,
        "query_data" if !ctx.mounted_sources.is_empty() => query_data(ctx, args).await,
        "remember" if ctx.can_write => remember(ctx, args).await,
        other => ToolResult::new(
            format!("Unknown tool: {other}"),
            json!({ "kind": "tool", "label": other, "detail": "unknown" }),
        ),
    }
}

/// 已经引过的给回原号，没引过的落一个新号。**同一个 chunk 在一轮对话里
/// 只能有一个号**，否则模型引 `[2]` 而界面上有两个 `[2]`。
fn cite(sink: &mut ToolSink, key: String, make: impl FnOnce(usize) -> serde_json::Value) -> usize {
    match sink.source_ids.iter().position(|id| *id == key) {
        Some(i) => i + 1,
        None => {
            sink.source_ids.push(key);
            sink.sources.push(make(sink.source_ids.len()));
            sink.source_ids.len()
        }
    }
}

pub async fn search_chunks(
    ctx: &ToolCtx<'_>,
    sink: &mut ToolSink,
    args: &serde_json::Value,
) -> ToolResult {
    // 必填参数由 `chat::check_call` 在派发之前挡下，所以这里不再回落到
    // 用户那句原话——回落产出的是一个看起来没问题的错误答案
    let q = args["query"].as_str().unwrap_or_default().to_string();
    // 记录轴（0019 / #347）：只搜那一刻库里有的东西。全文那一路仍是"现在"，
    // 命中不会错但会缺——retrieval.rs 的头上写了
    let as_of = args["as_of"].as_str().and_then(parse_when);
    let chunks = match retrieval::hybrid(
        ctx.state,
        ctx.kb_id,
        ctx.workspace_id,
        &q,
        SEARCH_TOP_K,
        as_of,
    )
    .await
    {
        Ok(chunks) => chunks,
        Err(e) => {
            tracing::warn!(error = %e, "MCP chunk search failed");
            return ToolResult::new(
                "Could not search the documents.".into(),
                json!({"kind": "search", "label": q, "detail": "failed"}),
            )
            .error();
        }
    };
    let mut lines = Vec::new();
    for c in &chunks {
        let n = cite(sink, c.id.to_string(), |n| source_json(n, c));
        // 行里带 document_id：命中被切在 800 字上时，模型得有办法把整篇要回去
        lines.push(format!(
            "[{n}] \"{}\" section {} (document_id: {}):\n{}",
            c.filename,
            c.seq + 1,
            c.document_id,
            truncate(&c.text, TOOL_CHUNK_CHARS)
        ));
    }
    let text = if lines.is_empty() {
        "No results.".to_string()
    } else {
        lines.join("\n\n")
    };
    ToolResult::new(
        text,
        json!({ "kind": "search", "label": q, "detail": format!("{} sources", chunks.len()) }),
    )
    .structured(json!({
        "kb_id": ctx.kb_id, "as_of": as_of,
        "limit": SEARCH_TOP_K, "limit_reached": chunks.len() == SEARCH_TOP_K,
        "chunks": chunks.iter().map(|c| json!({
            "chunk_id": c.id, "document_id": c.document_id,
            "seq": c.seq, "filename": c.filename,
            "text": truncate(&c.text, TOOL_CHUNK_CHARS),
            "truncated": c.text.trim().chars().count() > TOOL_CHUNK_CHARS,
        })).collect::<Vec<_>>()
    }))
}

/// 一篇文档的全文。**search_chunks 够不到的东西全在这里**：它只回前六条命中、
/// 每条切在 800 字上，答案落在第 801 字或落在没排上的那一块时，检索本身没错，
/// 错在没有第二步。
pub async fn get_document(
    ctx: &ToolCtx<'_>,
    sink: &mut ToolSink,
    args: &serde_json::Value,
) -> ToolResult {
    let refuse = |detail: &str| {
        ToolResult::new(
            "No document with that id in this knowledge base.".to_string(),
            json!({ "kind": "document", "label": "?", "detail": detail }),
        )
    };
    let Some(id) = args["document_id"]
        .as_str()
        .and_then(|s| s.trim().parse::<Uuid>().ok())
    else {
        return refuse("invalid id");
    };
    // 本库之外的 id 一律当作不存在——分不出「没有」和「不给你看」才是对的
    let Ok(Some(doc)) = utopia_store::documents::find_in_kb(&ctx.state.pool, ctx.kb_id, id).await
    else {
        return refuse("not found");
    };
    let chunks = utopia_store::documents::chunks_in_document(&ctx.state.pool, ctx.kb_id, id)
        .await
        .unwrap_or_default();

    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut omitted = 0usize;
    for c in &chunks {
        let body = c.text.trim();
        let len = body.chars().count();
        let room = DOCUMENT_CHARS.saturating_sub(used);
        // 装不下的分块整块不发，也不引——发一个引证编号出去而正文不在，
        // 界面上落成一条指不到东西的引证
        if room == 0 {
            omitted += len;
            continue;
        }
        let body: String = if len > room {
            omitted += len - room;
            body.chars().take(room).collect()
        } else {
            body.to_string()
        };
        used += body.chars().count();
        let n = cite(sink, c.id.to_string(), |n| source_json(n, c));
        lines.push(format!(
            "[{n}] \"{}\" section {}:\n{body}",
            c.filename,
            c.seq + 1
        ));
    }
    if omitted > 0 {
        lines.push(format!("… truncated, {omitted} chars omitted"));
    }

    let when = doc
        .doc_time
        .map(crate::time_text::instant)
        .unwrap_or_else(|| "no date".to_string());
    let header = format!(
        "\"{}\" ({when}) — {} section(s):",
        doc.filename,
        chunks.len()
    );
    let text = if lines.is_empty() {
        format!("{header}\n(no text)")
    } else {
        format!("{header}\n\n{}", lines.join("\n\n"))
    };
    ToolResult::new(
        text,
        json!({
            "kind": "document", "label": doc.filename,
            "detail": format!("{} sections", chunks.len()),
        }),
    )
}

pub async fn search_docs(
    ctx: &ToolCtx<'_>,
    sink: &mut ToolSink,
    args: &serde_json::Value,
) -> ToolResult {
    // 必填参数由 `chat::check_call` 在派发之前挡下，所以这里不再回落到
    // 用户那句原话——回落产出的是一个看起来没问题的错误答案
    let q = args["query"].as_str().unwrap_or_default().to_string();
    // Tantivy 是同步的，放到阻塞线程池上，别占 runtime 线程（#515，与 retrieval.rs 同理）
    let hits = {
        let docs = ctx.state.docs.clone();
        let q = q.clone();
        tokio::task::spawn_blocking(move || docs.search(&q, 4))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
    };
    let mut lines = Vec::new();
    for h in &hits {
        let key = format!("charter:{}#{}", h.slug, h.anchor);
        let n = cite(sink, key, |n| charter_source_json(n, h));
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
    ToolResult::new(
        text,
        json!({ "kind": "docs", "label": q, "detail": format!("{} sections", hits.len()) }),
    )
}

/// 这个库的判据。**把阈值原样给出来**——模型要能解释「凭什么算含气井」，
/// 而不是猜一个听起来合理的门槛。
pub async fn list_rules(ctx: &ToolCtx<'_>) -> ToolResult {
    let Ok(rules) = utopia_store::business_rules::list(&ctx.state.pool, ctx.kb_id).await else {
        return ToolResult::new(
            "Could not read the rules.".to_string(),
            json!({ "kind": "tool", "label": "list_rules", "detail": "failed" }),
        );
    };
    if rules.is_empty() {
        return ToolResult::new(
            "This base has no business rules.".to_string(),
            json!({ "kind": "tool", "label": "list_rules", "detail": "none" }),
        );
    }
    let text = rules
        .iter()
        .map(|r| {
            let conditions = r["conditions"]
                .as_array()
                .map(|cs| {
                    cs.iter()
                        .map(|c| {
                            format!(
                                "{} {} {}",
                                c["predicate_label"].as_str().unwrap_or("?"),
                                c["op"].as_str().unwrap_or("?"),
                                c["operand"]
                                    .as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| c["operand"].to_string()),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" AND ")
                })
                .unwrap_or_default();
            let concludes = if r["conclusion"] == "typing" {
                r["conclude_type_label"].as_str().unwrap_or("?").to_string()
            } else {
                format!(
                    "{} = {}",
                    r["conclude_predicate_label"].as_str().unwrap_or("?"),
                    r["conclude_value"]
                )
            };
            format!(
                "{} [{}] — applies to {} where {} ⇒ {} · marks {} now · id {}",
                r["name"].as_str().unwrap_or("?"),
                if r["enabled"] == true { "on" } else { "off" },
                r["subject_label"].as_str().unwrap_or("?"),
                conditions,
                concludes,
                r["derived_count"],
                r["id"].as_str().unwrap_or("?"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let n = rules.len();
    ToolResult::new(
        text,
        json!({ "kind": "tool", "label": "list_rules", "detail": format!("{n} rules") }),
    )
}

/// 一条规则此刻标了谁。**前提一起给**：结论没有前提就跟一条凭空的断言没区别。
pub async fn rule_matches(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    let Some(rule_id) = args["rule_id"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok())
    else {
        return ToolResult::new(
            "Invalid rule_id (expected the uuid returned by list_rules).".to_string(),
            json!({ "kind": "tool", "label": "rule_matches", "detail": "invalid id" }),
        );
    };
    let limit = args["limit"].as_i64().unwrap_or(50).clamp(1, 200);
    let Ok((rows, total)) =
        utopia_store::business_rules::matches(&ctx.state.pool, ctx.kb_id, rule_id, limit, 0).await
    else {
        return ToolResult::new(
            "Could not read what that rule marks.".to_string(),
            json!({ "kind": "tool", "label": "rule_matches", "detail": "failed" }),
        );
    };
    if rows.is_empty() {
        return ToolResult::new(
            "That rule marks nothing right now.".to_string(),
            json!({ "kind": "tool", "label": "rule_matches", "detail": "0" }),
        );
    }
    let text = rows
        .iter()
        .map(|m| {
            let premises = m["premises"]
                .as_array()
                .map(|p| {
                    p.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            format!(
                "{} ⇒ {} (because {}) [{}]",
                m["entity"].as_str().unwrap_or("?"),
                m["concluded"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| m["concluded"].to_string()),
                premises,
                m["entity_id"].as_str().unwrap_or("?"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    // 截断要说出来：模型看到 50 条会当成全部，而库里可能有两百
    let text = if total > rows.len() as i64 {
        format!("{text}\n(showing {} of {total})", rows.len())
    } else {
        text
    };
    ToolResult::new(
        text,
        json!({ "kind": "tool", "label": "rule_matches", "detail": format!("{total} entities") }),
    )
}

pub async fn changes(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    // 两端与 `at` 同一种写法：YYYY / YYYY-MM / YYYY-MM-DD。`since` 取那一段的第一天，
    // `until` 取最后一天——「2023 年有什么变化」不该逼模型编一个 12 月 31 日
    let since = args["since"]
        .as_str()
        .and_then(utopia_extract::parse_time)
        .map(|(t, _)| t.date_naive());
    let until = args["until"]
        .as_str()
        .and_then(utopia_extract::parse_time)
        .map(|(t, p)| period_last_day(t.date_naive(), p));
    let Some((since, until, window)) = changes_window(since, until, chrono::Utc::now()) else {
        return ToolResult::new(
            "Invalid or missing `since` (expected YYYY-MM-DD).".to_string(),
            json!({ "kind": "changes", "label": "?", "detail": "invalid since" }),
        )
        .error();
    };
    let entity = args["entity_id"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok());
    let kinds: Option<Vec<String>> = args["kinds"].as_array().map(|a| {
        a.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    });
    let kinds = kinds.filter(|k: &Vec<String>| !k.is_empty());
    let rows = match utopia_store::graph::graph_changes(
        &ctx.state.pool,
        ctx.kb_id,
        since,
        until,
        entity,
        kinds.as_deref(),
        CHANGES_LIMIT,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "Graph changes lookup failed");
            return ToolResult::new(
                "Could not read the graph changes.".into(),
                json!({"kind": "changes", "label": window, "detail": "failed"}),
            )
            .error();
        }
    };
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
    ToolResult::new(
        text,
        json!({ "kind": "changes", "label": window, "detail": detail }),
    )
    .structured(json!({
        "kb_id": ctx.kb_id, "since": since, "until": until,
        "limit": CHANGES_LIMIT, "limit_reached": rows.len() as i64 == CHANGES_LIMIT,
        "changes": rows.iter().map(|r| json!({
            "fact_id": r.fact_id, "at": r.at, "kind": r.kind,
            "subject_id": r.subject_id, "subject_name": r.subject_name,
            "predicate_label": r.predicate_label, "object_name": r.object_name,
            "object_value": r.object_value, "confidence": r.confidence,
            "valid_from": r.valid_from, "valid_to": r.valid_to,
            "valid_from_precision": r.valid_from_precision,
            "valid_to_precision": r.valid_to_precision,
            "document_id": r.document_id, "filename": r.filename, "quote": r.quote,
        })).collect::<Vec<_>>()
    }))
}

pub async fn query_data(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    let ds_name = args["data_source"].as_str().map(str::trim).unwrap_or("");
    let sql = args["sql"].as_str().map(str::trim).unwrap_or("");
    let purpose = args["purpose"].as_str().map(str::trim).unwrap_or("");
    // 安全边界：只允许本 KB 挂载的源（凭据不出服务端）
    let found = ctx
        .mounted_sources
        .iter()
        .find(|d| d.name.eq_ignore_ascii_case(ds_name));
    let text = match found {
        None => format!(
            "Unknown data source '{ds_name}'. Mounted sources: {}",
            ctx.mounted_sources
                .iter()
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Some(ds) => match run_query(ctx.state, ds.id, sql).await {
            Ok(out) => out,
            // 错误透传：模型可据此修正 SQL 重试
            Err(e) => format!("Query failed: {e}"),
        },
    };
    let detail = if purpose.is_empty() {
        sql.chars().take(60).collect::<String>()
    } else {
        purpose.to_string()
    };
    ToolResult::new(
        text,
        json!({ "kind": "query", "label": ds_name, "detail": detail }),
    )
}

pub async fn remember(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    // NUL 在入口就剥（#665）：`append_episode` 自己也剥，但这里拼的回复与卡片详情
    // 用的是同一份文本——对话里它们要落进会话记录，Postgres 的 TEXT 与 JSONB 一样
    // 不收 0x00，一轮对话会因为回显了几个字节而存不下来
    let text = utopia_core::without_nul(args["text"].as_str().unwrap_or(""));
    let text = text.trim();
    // 与 `at` 同一种写法：YYYY / YYYY-MM / YYYY-MM-DD 或 RFC3339。日期落在那段第一天的
    // 正午——离两边的日界都最远；时刻照给的。回显按给的精度写，不把「2023 年」说成 1 月 1 日
    let (occurred_at, occurred_text) = match args["occurred_at"].as_str().map(str::trim) {
        Some(s) if !s.is_empty() => match utopia_extract::parse_time(s) {
            Some((d, precision)) => (
                d + chrono::Duration::hours(12),
                crate::time_text::world(d, Some(precision)),
            ),
            None => match parse_when(s) {
                Some(at) => (at, crate::time_text::instant(at)),
                None => {
                    let now = chrono::Utc::now();
                    (now, crate::time_text::instant(now))
                }
            },
        },
        _ => {
            let now = chrono::Utc::now();
            (now, crate::time_text::instant(now))
        }
    };
    if text.is_empty() {
        return ToolResult::new(
            "remember requires non-empty text.".to_string(),
            json!({ "kind": "tool", "label": "remember", "detail": "empty" }),
        );
    }
    match utopia_store::memory::append_episode(&ctx.state.pool, ctx.kb_id, text, occurred_at).await
    {
        Ok((doc_id, chunk_id)) => {
            // 摄入(embedding/索引/增量抽取)异步走队列，不阻塞对话。
            // 抽出来的事实**先等人点头**（0015）——所以这里只能如实说「记下了这句话」，
            // 说不出抽出了几条：抽取还没跑。卡片在任务完成时长到对话里
            let _ = utopia_store::jobs::enqueue(
                &ctx.state.pool,
                "memory_ingest",
                json!({
                    "document_id": doc_id,
                    "proposed_by": ctx.actor,
                    "proposed_token": ctx.via_token,
                }),
            )
            .await;
            ctx.state.emit_document(ctx.kb_id, doc_id);
            ToolResult::new(
                format!(
                    "Recorded the sentence (effective {}): {text}\n\
                     Facts extracted from it will be shown to the user for confirmation \
                     before entering the graph. Tell the user exactly that: the sentence is \
                     recorded, and the extracted facts await their confirmation. Do not claim \
                     any fact has been added to the knowledge graph.",
                    occurred_text
                ),
                json!({
                    "kind": "tool", "label": "remember",
                    "detail": text.chars().take(60).collect::<String>(),
                    // 对话里那张确认卡按它取待确认项；回放时也据此重画
                    "chunk_id": chunk_id,
                }),
            )
        }
        Err(e) => ToolResult::new(
            format!("Failed to record: {e}"),
            json!({ "kind": "tool", "label": "remember", "detail": "failed" }),
        ),
    }
}

// ---------------------------------------------------------------------------
// 排版与执行的辅助。**对话降级路径也用它们**，所以是 pub(super) 而不是私有
// ---------------------------------------------------------------------------

pub(super) fn source_json(n: usize, c: &ChunkView) -> serde_json::Value {
    json!({
        "n": n,
        "chunk_id": c.id,
        "document_id": c.document_id,
        "filename": c.filename,
        "excerpt": truncate(&c.text, 160),
    })
}

/// Charter 引用：前端渲染成手册行，链到 /docs/{slug}#{anchor}。
pub(super) fn charter_source_json(n: usize, h: &utopia_search::DocsSection) -> serde_json::Value {
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

/// 问数执行：安全闸（解析白名单）→ 引擎执行（只读会话 + 强制 LIMIT + 超时）→ JSON 行。
pub(crate) async fn run_query(state: &AppState, ds_id: Uuid, sql: &str) -> anyhow::Result<String> {
    let (engine, conn) = utopia_store::datasources::engine_and_conn(&state.pool, ds_id).await?;
    // 闸门按引擎选方言：Databricks 的反引号、Snowflake 的 :: 转型都得先过得了解析
    let guarded = crate::query_engine::guard_sql_for(&engine, sql)?;
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

/// 事实行："works at → 星云科技 (2023-08 → now) [90%]"，in 方向用 ←。
pub(super) fn fact_line(f: &EntityFact) -> String {
    // 属性事实没有对端实体，值在 `object_value` 里（0004）。从前这里只看 `other_name`，
    // 于是薪资、职位到了模型眼前是 `salary → ?`——区间和置信度都在，唯独值没到，
    // 模型只能说"没有薪资信息"（#348）。渲染规则与客户端 `fmtObjectValue` 一致
    let literal = f
        .object_value
        .as_ref()
        .and_then(literal_text)
        .filter(|_| f.other_name.is_none());
    let other = f
        .other_name
        .as_deref()
        .or(literal.as_deref())
        .unwrap_or("?");
    // 边上的属性（0037）跟在对端后面：`invested_in → Kestrel [amount: 4000000000 $]`。
    // 模型读事实行时最常问的就是"投了多少"，数不在行里它就答"没有金额信息"
    let quals: Vec<String> = f
        .qualifiers
        .iter()
        .map(|q| {
            let v = q
                .value
                .as_ref()
                .and_then(literal_text)
                .or_else(|| q.entity_name.clone())
                .unwrap_or_else(|| "?".to_string());
            format!("{}: {v}", q.key)
        })
        .collect();
    let other = if quals.is_empty() {
        other.to_string()
    } else {
        format!("{other} [{}]", quals.join(", "))
    };
    // 本体没认下、原文说法也没留下时用 "?"——与 other 同一个约定。
    // 不编一个"相关"出来：那正是删掉 related_to 要消灭的东西
    let pred = f.predicate_label.as_deref().unwrap_or("?");
    let core = if f.direction == "out" {
        format!("{pred} → {other}")
    } else {
        format!("{pred} ← {other}")
    };
    // 两端各按自己的精度写；没起点的从证据起，结束了不知哪天的到说出它的那份文档为止
    //（time_text，0022）。从前一律 %Y-%m-%d，年精度印成 1 月 1 日、结束未知印成 now
    let range = crate::time_text::span(crate::time_text::Span {
        valid_from: f.valid_from,
        from_precision: f.valid_from_precision.as_deref(),
        valid_to: f.valid_to,
        to_precision: f.valid_to_precision.as_deref(),
        holds_from: f.holds_from,
        holds_to: f.holds_to,
    });
    let range = if range.is_empty() {
        range
    } else {
        format!(" ({range})")
    };
    format!("{core}{range} [{}%]", (f.confidence * 100.0).round() as i32)
}

// 记录轴上的两次更正可以发生在同一秒；输出必须能原样交回 as_of，不能截到天或秒。
fn record_stamp(time: chrono::DateTime<chrono::Utc>) -> String {
    crate::time_text::instant(time)
}

/// 严格早于 T，按账本的分辨率（timestamptz 是微秒）：不晚于 T − 1µs。
/// `recorded_at <= T−1µs` 恰好是 `recorded_at < T`，`invalidated_at > T−1µs` 恰好是
/// `invalidated_at >= T`——0019 的 held_at 一个字不用改
pub(super) fn just_before(t: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> {
    t - chrono::Duration::microseconds(1)
}

pub(super) fn entity_facts_detail(
    count: usize,
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    before: Option<chrono::DateTime<chrono::Utc>>,
) -> String {
    // 记录轴那半句：给了 before 就说「before T」，是人问的那个时刻，不是减过一微秒的
    let record = match (before, as_of) {
        (Some(b), _) => Some(format!("as recorded before {}", record_stamp(b))),
        (None, Some(r)) => Some(format!("as recorded by {}", record_stamp(r))),
        (None, None) => None,
    };
    match (at, record) {
        (Some(t), Some(r)) => format!("{count} facts at {}, {r}", record_stamp(t)),
        (Some(t), None) => format!("{count} facts as of {}", record_stamp(t)),
        (None, Some(r)) => format!("{count} facts {r}"),
        (None, None) => format!("{count} facts"),
    }
}

/// 时刻参数：`YYYY-MM-DD` 或 RFC3339。`at` 与 `as_of` 共用一个解析——记录轴上的
/// 时刻常常是一个带时间的戳（"第一波灌完那一刻"），日期粒度装不下它
pub(super) fn parse_when(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let s = raw.trim();
    // 完整的 RFC3339 时刻原样收下，小数秒也留着——记录轴上同一秒内可以先录入再更正
    // （#351），这里截掉一位就把两次认知叠回一起。日期形式（YYYY / YYYY-MM / YYYY-MM-DD，
    // 或带时区的缩略钟点）才交给抽取端同一个解析，取那一段的开头
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    utopia_extract::parse_time(s).map(|(d, _)| d)
}

/// 字面值宾语给模型看的样子：`{value, unit}` → "28000 CNY"，布尔 → ✓/✗，
/// 映射那类 `{summary}` → 摘要本身。与 `web/src/pages/Graph.tsx::fmtObjectValue` 同一条规则，
/// 两边分叉的话，人看到的和模型看到的就不是同一个值
pub(super) fn literal_text(v: &serde_json::Value) -> Option<String> {
    // 裸标量（`changes` 那头的旧数据长这样）：字符串读成它自己，不带引号
    match v {
        serde_json::Value::Null => return None,
        serde_json::Value::String(s) => return Some(s.clone()),
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => return Some(v.to_string()),
        _ => {}
    }
    if let Some(val) = v.get("value") {
        let text = match val {
            serde_json::Value::Bool(true) => "✓".to_string(),
            serde_json::Value::Bool(false) => "✗".to_string(),
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => return None,
            other => other.to_string(),
        };
        return Some(
            match v
                .get("unit")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
            {
                Some(unit) => format!("{text} {unit}"),
                None => text,
            },
        );
    }
    if let Some(summary) = v.get("summary").and_then(|s| s.as_str()) {
        return Some(summary.to_string());
    }
    Some(v.to_string())
}

/// changes 的时间窗：把两个可选日期变成 (SQL 用的半开区间, 展示用的窗口串)。
///
/// **抽成纯函数是因为这里出过一次错。** `until` 进 SQL 前要加一天（说"到 3 月 31 日
/// 为止"的人要的是含 31 日，而 SQL 那头是 `< $3`），第一版把加过一天的值也印进了
/// 展示串，模型于是照着答"截至 8 月 30 日"——问的是 29 日。两个值必须一起算、
/// 一起被测住；分在两处写，迟早再次分叉。
///
/// `now` 从外面传进来而不是在里面取，纯粹是为了这个函数测得动。
/// 一段（年 / 月 / 日）的最后一天：`until=2023` 是到 12 月 31 日为止，含那一整天。
fn period_last_day(first: chrono::NaiveDate, precision: &str) -> chrono::NaiveDate {
    use chrono::Datelike;
    match precision {
        "year" => chrono::NaiveDate::from_ymd_opt(first.year(), 12, 31).unwrap_or(first),
        "month" => {
            let (y, m) = if first.month() == 12 {
                (first.year() + 1, 1)
            } else {
                (first.year(), first.month() + 1)
            };
            chrono::NaiveDate::from_ymd_opt(y, m, 1)
                .and_then(|d| d.pred_opt())
                .unwrap_or(first)
        }
        _ => first,
    }
}

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
    // 字面值宾语与 `fact_line` 同一条规则（`literal_text`）：两个工具看到的
    // 不能是两种写法，否则 `{value, unit}` 在这里会印成一段 JSON
    let object = match (&c.object_name, &c.object_value) {
        (Some(name), _) => name.clone(),
        (None, Some(v)) => literal_text(v).unwrap_or_else(|| "?".to_string()),
        (None, None) => "?".to_string(),
    };
    // 世界轴两端按自己的精度写（time_text）；事件行上没有锚点，结束未知只能写成话
    let range = crate::time_text::span(crate::time_text::Span {
        valid_from: c.valid_from,
        from_precision: c.valid_from_precision.as_deref(),
        valid_to: c.valid_to,
        to_precision: c.valid_to_precision.as_deref(),
        holds_from: None,
        holds_to: None,
    });
    let range = if range.is_empty() {
        range
    } else {
        format!(" [valid {range}]")
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
        record_stamp(c.at),
        c.kind,
        c.subject_name,
        c.predicate_label.as_deref().unwrap_or("?"),
        object,
        range,
        src
    )
}

pub(super) fn truncate(text: &str, max_chars: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max_chars {
        t.to_string()
    } else {
        let cut: String = t.chars().take(max_chars).collect();
        format!("{cut}…")
    }
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

    // --- parse_when -------------------------------------------------------------

    /// `at` 与 `as_of` 共用一个解析：日期按当天零点，RFC3339 原样；别的一律 None，
    /// 不猜——猜出来的时刻会安静地把问题答到另一天上
    #[test]
    fn a_moment_is_a_date_or_an_rfc3339_stamp_and_nothing_else() {
        assert_eq!(parse_when("2024-08-01"), Some(t("2024-08-01T00:00:00Z")));
        assert_eq!(
            parse_when(" 2026-09-05T02:43:24.197Z "),
            Some(t("2026-09-05T02:43:24.197Z"))
        );
        assert_eq!(parse_when("August 2024"), None);
        assert_eq!(parse_when(""), None);
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

    /// 年精度的起点写成年，不再印成 1 月 1 日；结束了不知哪天绝不写 now
    #[test]
    fn a_change_line_shows_each_end_at_its_precision() {
        let mut c = change("asserted");
        c.valid_from = Some(t("2019-01-01T00:00:00Z"));
        c.valid_from_precision = Some("year".into());
        assert!(
            change_line(&c).contains("[valid 2019 → now]"),
            "{}",
            change_line(&c)
        );
        c.valid_to_precision = Some("unknown".into());
        assert!(
            change_line(&c).contains("[valid 2019 → ended, date unknown]"),
            "{}",
            change_line(&c)
        );
    }

    #[test]
    fn a_window_end_covers_the_whole_period_named() {
        assert_eq!(period_last_day(d("2023-01-01"), "year"), d("2023-12-31"));
        assert_eq!(period_last_day(d("2024-02-01"), "month"), d("2024-02-29"));
        assert_eq!(period_last_day(d("2023-12-01"), "month"), d("2023-12-31"));
        assert_eq!(period_last_day(d("2023-06-15"), "day"), d("2023-06-15"));
        // parse_when 与 at 同一种写法
        assert_eq!(parse_when("2023"), Some(t("2023-01-01T00:00:00Z")));
        assert_eq!(parse_when("2023-06"), Some(t("2023-06-01T00:00:00Z")));
    }

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

    // 同一秒内也能先录入再更正；截到整秒会把这两次认知重新叠在一起。
    #[test]
    fn change_stamps_round_trip_without_losing_the_record_clock() {
        let mut c = change("corrected");
        for stamp in [
            "2026-09-05T02:43:53Z",
            "2026-09-05T02:43:53.382Z",
            "2026-09-05T02:43:53.382001Z",
            "2026-09-05T02:43:53.382002Z",
        ] {
            c.at = t(stamp);
            let line = change_line(&c);
            let printed = line.split_whitespace().next().unwrap();
            assert_eq!(printed, stamp);
            assert_eq!(parse_when(printed), Some(c.at));
        }
    }

    /// `before` 减的是账本分辨率的一微秒——不是一秒、不是一天；摘要里写的是人问的那个时刻
    #[test]
    fn before_is_the_microsecond_before_at_the_ledgers_resolution() {
        let t0 = t("2026-09-05T02:43:53.382Z");
        assert_eq!(just_before(t0), t("2026-09-05T02:43:53.381999Z"));
        assert_eq!(
            just_before(t("2026-09-05T02:43:53Z")),
            t("2026-09-05T02:43:52.999999Z"),
            "小数为零要向秒借位——正是不该让模型算的那一步"
        );
        assert_eq!(
            entity_facts_detail(2, None, Some(just_before(t0)), Some(t0)),
            "2 facts as recorded before 2026-09-05T02:43:53.382Z"
        );
    }

    #[test]
    fn fact_details_keep_the_record_instant_beside_the_world_date() {
        let at = Some(t("2024-08-01T00:00:00Z"));
        let as_of = Some(t("2026-09-05T02:43:53.382001Z"));
        assert_eq!(
            entity_facts_detail(2, at, as_of, None),
            "2 facts at 2024-08-01T00:00:00Z, as recorded by 2026-09-05T02:43:53.382001Z"
        );
        assert_eq!(
            entity_facts_detail(2, None, as_of, None),
            "2 facts as recorded by 2026-09-05T02:43:53.382001Z"
        );
        assert_eq!(
            entity_facts_detail(2, at, None, None),
            "2 facts as of 2024-08-01T00:00:00Z"
        );
        assert_eq!(entity_facts_detail(0, None, None, None), "0 facts");
    }

    fn attribute_fact(value: serde_json::Value) -> EntityFact {
        EntityFact {
            recorded_at: chrono::Utc::now(),
            invalidated_at: None,
            supersedes: None,
            document_ids: vec![],
            id: Uuid::nil(),
            direction: "out".into(),
            predicate_key: Some("salary".into()),
            predicate_label: Some("salary".into()),
            inferred: false,
            temporal: Some("state".into()),
            other_id: None,
            other_name: None,
            other_type: None,
            qualifiers: Vec::new(),
            object_value: Some(value),
            valid_from: Some(t("2023-06-01T00:00:00Z")),
            valid_to: Some(t("2024-02-20T00:00:00Z")),
            valid_from_precision: Some("day".into()),
            valid_to_precision: Some("day".into()),
            holds_from: Some(t("2023-06-01T00:00:00Z")),
            holds_to: Some(t("2024-02-20T00:00:00Z")),
            confidence: 0.9,
            evidence_count: 1,
            stale: false,
            corrected: false,
            last_evidence_time: None,
            contested: None,
        }
    }

    /// 属性事实的值要到模型眼前（#348）。从前这里是 `salary → ? (2023-06-01 → 2024-02-20)`：
    /// 区间和置信度都在，唯独值没到，模型只能说"没有薪资信息"——而账本里明明有
    #[test]
    fn an_attribute_fact_shows_the_model_its_value() {
        let line = fact_line(&attribute_fact(
            serde_json::json!({ "value": 28000, "unit": "CNY" }),
        ));
        assert!(line.starts_with("salary → 28000 CNY"), "{line}");
        assert!(line.contains("(2023-06-01 → 2024-02-20)"), "{line}");

        // 与客户端 fmtObjectValue 同一条规则：布尔画成 ✓/✗，映射摘要读摘要本身
        let flag = fact_line(&attribute_fact(serde_json::json!({ "value": true })));
        assert!(flag.starts_with("salary → ✓"), "{flag}");
        let mapped = fact_line(&attribute_fact(
            serde_json::json!({ "summary": "orders.total" }),
        ));
        assert!(mapped.starts_with("salary → orders.total"), "{mapped}");
    }

    /// 本体没接住的关系仍然是 "?"：那个问号是给「没有谓词」留的，不是给「有值」用的
    #[test]
    fn a_missing_object_still_reads_as_a_question_mark() {
        let mut f = attribute_fact(serde_json::json!(null));
        f.object_value = None;
        assert!(fact_line(&f).starts_with("salary → ?"), "{}", fact_line(&f));
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
        c.valid_from_precision = Some("day".into());
        let line = change_line(&c);
        assert!(
            line.starts_with("2026-08-28T10:00:00Z corrected: "),
            "{line}"
        );
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
}
