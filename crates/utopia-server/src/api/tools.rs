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

/// 一次工具调用的产出：给模型的文本 + 给界面的一步。
pub type ToolResult = (String, serde_json::Value);

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
        "find_entities" => find_entities(ctx, sink, args).await,
        "entity_facts" => entity_facts(ctx, args).await,
        "changes" => changes(ctx, args).await,
        "query_data" if !ctx.mounted_sources.is_empty() => query_data(ctx, args).await,
        "remember" if ctx.can_write => remember(ctx, args).await,
        other => (
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
    let chunks = retrieval::hybrid(ctx.state, ctx.kb_id, ctx.workspace_id, &q, SEARCH_TOP_K)
        .await
        .unwrap_or_default();
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
    (
        text,
        json!({ "kind": "search", "label": q, "detail": format!("{} sources", chunks.len()) }),
    )
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
        (
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
        .map(|t| t.format("%Y-%m-%d").to_string())
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
    (
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
    let hits = ctx.state.docs.search(&q, 4).unwrap_or_default();
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
    (
        text,
        json!({ "kind": "docs", "label": q, "detail": format!("{} sections", hits.len()) }),
    )
}

pub async fn find_entities(
    ctx: &ToolCtx<'_>,
    sink: &mut ToolSink,
    args: &serde_json::Value,
) -> ToolResult {
    let name = args["name"].as_str().unwrap_or("").to_string();
    let (hits, _) = utopia_store::graph::search_entities(&ctx.state.pool, ctx.kb_id, &name, 8, 0)
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
        sink.resolved.push(json!({
            "id": n.id.to_string(), "name": n.name, "type": n.type_label
        }));
    }
    (
        text,
        json!({ "kind": "entity", "label": name, "detail": format!("{} matches", hits.len()) }),
    )
}

pub async fn entity_facts(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    let id = args["entity_id"]
        .as_str()
        .and_then(|s| s.parse::<Uuid>().ok());
    // as-of 过滤：T 时刻有效 = 起点不晚于 T（或未知）且终点晚于 T（或开放）
    let at = args["at"]
        .as_str()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc());
    let Some(id) = id else {
        return (
            "Invalid entity_id (expected the uuid returned by find_entities).".to_string(),
            json!({ "kind": "facts", "label": "?", "detail": "invalid id" }),
        );
    };
    match utopia_store::graph::entity_detail(&ctx.state.pool, ctx.kb_id, id, None).await {
        Ok((node, mut facts)) => {
            if let Some(t) = at {
                facts.retain(|f| {
                    f.valid_from.is_none_or(|from| from <= t) && f.valid_to.is_none_or(|to| to > t)
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
                Some(t) => format!("{} facts as of {}", facts.len(), t.format("%Y-%m-%d")),
                None => format!("{} facts", facts.len()),
            };
            (
                text,
                json!({ "kind": "facts", "label": node.name, "detail": detail }),
            )
        }
        Err(_) => (
            "Entity not found.".to_string(),
            json!({ "kind": "facts", "label": "?", "detail": "not found" }),
        ),
    }
}

pub async fn changes(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    let day = |k: &str| {
        args[k]
            .as_str()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
    };
    let Some((since, until, window)) =
        changes_window(day("since"), day("until"), chrono::Utc::now())
    else {
        return (
            "Invalid or missing `since` (expected YYYY-MM-DD).".to_string(),
            json!({ "kind": "changes", "label": "?", "detail": "invalid since" }),
        );
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
    let rows = utopia_store::graph::graph_changes(
        &ctx.state.pool,
        ctx.kb_id,
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
    (
        text,
        json!({ "kind": "changes", "label": window, "detail": detail }),
    )
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
    (
        text,
        json!({ "kind": "query", "label": ds_name, "detail": detail }),
    )
}

pub async fn remember(ctx: &ToolCtx<'_>, args: &serde_json::Value) -> ToolResult {
    let text = args["text"].as_str().map(str::trim).unwrap_or("");
    let occurred_at = args["occurred_at"]
        .as_str()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
        .map(|d| d.and_hms_opt(12, 0, 0).unwrap().and_utc())
        .unwrap_or_else(chrono::Utc::now);
    if text.is_empty() {
        return (
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
            (
                format!(
                    "Recorded the sentence (effective {}): {text}\n\
                     Facts extracted from it will be shown to the user for confirmation \
                     before entering the graph. Tell the user exactly that: the sentence is \
                     recorded, and the extracted facts await their confirmation. Do not claim \
                     any fact has been added to the knowledge graph.",
                    occurred_at.format("%Y-%m-%d")
                ),
                json!({
                    "kind": "tool", "label": "remember",
                    "detail": text.chars().take(60).collect::<String>(),
                    // 对话里那张确认卡按它取待确认项；回放时也据此重画
                    "chunk_id": chunk_id,
                }),
            )
        }
        Err(e) => (
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
async fn run_query(state: &AppState, ds_id: Uuid, sql: &str) -> anyhow::Result<String> {
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
}
