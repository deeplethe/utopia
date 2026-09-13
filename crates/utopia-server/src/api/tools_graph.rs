//! 图谱查询工具（#558）：`paths_between`、`neighbors`、`timeline`，以及
//! `find_entities` 的排序和 `entity_facts` 的过滤与分组。
//!
//! 从前模型手里的图谱只有两个动作：按名找实体、倒出一个实体的全部事实。
//! 「OpenAI 和 Anthropic 的关系」要它自己把两边几百条事实在上下文里连线，
//! 十六次里三次连上；「OpenAI 的时间线」是一份 433 行的倾倒，它自己排序。
//! 这里把查询的活收回服务端：路径是服务端搜的，邻居按谓词分好组，时间线按
//! 世界时间排好，事实可以按谓词、对象类型、时段筛。
//!
//! **实体参数收 uuid 也收名字。** 收名字是为了省掉一轮 find_entities，以及那一轮
//! 之后常见的「你说的是哪个 OpenAI」——排序规则挑一个，把挑了谁、还有谁一并
//! 写在结果最上面，模型不同意就拿 id 再来一次。

use super::tools::{
    entity_facts_detail, fact_line, just_before, literal_text, parse_when, ToolCtx, ToolResult,
    ToolSink,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use utopia_core::models::{EntityFact, GraphNode};
use utopia_store::paths::{Limits, Path, PathEdge};
use uuid::Uuid;

const NEIGHBORS_DEFAULT: usize = 40;
const TIMELINE_DEFAULT: usize = 60;
const FACTS_DEFAULT: usize = 80;
const LIST_MAX: usize = 300;

// ---- 实体参数 -------------------------------------------------------------------

/// 名字匹配的排序：**类型判出来的在前，同名精确命中在前，事实多的在前**。
///
/// 没类型的短语（"lawsuit against OpenAI"，#559）在有别的候选时不列：它们几乎全是
/// 抽取漏进来的描述，列出来只会把问题变成一次消歧。全是 untyped 时照列——一个
/// 类型都没判出来的库也得能用。**叫这个名字的照列，有没有类型都一样**：问的就是
/// "Acme"，而 Acme 本身还没判出类型时，只剩一个 "Acme lawsuit" 是把人引到错的实体上，
/// 没类型的实体也得找得到（0009）。
pub(super) fn rank(mut hits: Vec<GraphNode>, query: &str) -> Vec<GraphNode> {
    let q = query.trim().to_lowercase();
    let exact = |n: &GraphNode| n.name.trim().to_lowercase() == q;
    if hits.iter().any(|n| n.type_label.is_some()) {
        hits.retain(|n| n.type_label.is_some() || exact(n));
    }
    hits.sort_by(|a, b| {
        exact(b)
            .cmp(&exact(a))
            .then(b.degree.cmp(&a.degree))
            .then_with(|| a.name.cmp(&b.name))
    });
    hits
}

/// 第一名是不是明显的那一个：只有它，或只有它精确命中，或它的事实数是第二名的三倍
pub(super) fn dominant(ranked: &[GraphNode], query: &str) -> bool {
    let q = query.trim().to_lowercase();
    match ranked {
        [] => false,
        [_] => true,
        [first, second, ..] => {
            let exact = |n: &GraphNode| n.name.trim().to_lowercase() == q;
            (exact(first) && !exact(second)) || first.degree >= 3 * second.degree.max(1)
        }
    }
}

fn node_line(n: &GraphNode) -> String {
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
}

fn remember(sink: &mut ToolSink, n: &GraphNode) {
    sink.resolved.push(json!({
        "id": n.id.to_string(), "name": n.name, "type": n.type_label
    }));
}

/// 一个实体参数解析成 id。名字走 [`rank`]，挑了谁写进 `note` 交给结果开头
struct Resolved {
    id: Uuid,
    name: String,
    note: Option<String>,
}

enum ResolveError {
    Unresolved(String),
    ReadFailed,
}

async fn resolve(
    ctx: &ToolCtx<'_>,
    sink: &mut ToolSink,
    raw: &str,
) -> Result<Resolved, ResolveError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ResolveError::Unresolved(
            "an entity name or id is required".to_string(),
        ));
    }
    if let Ok(id) = raw.parse::<Uuid>() {
        return Ok(Resolved {
            id,
            name: raw.to_string(),
            note: None,
        });
    }
    let hits = lookup(ctx, raw).await.map_err(|e| {
        tracing::warn!(error = %e, "Entity lookup failed");
        ResolveError::ReadFailed
    })?;
    let (ranked, by_question) = rank_by_question(ctx, rank(hits, raw), raw).await;
    let Some(first) = ranked.first() else {
        return Err(ResolveError::Unresolved(format!(
            "no entity named \"{raw}\" in this base"
        )));
    };
    remember(sink, first);
    let others: Vec<String> = ranked
        .iter()
        .skip(1)
        .take(4)
        .map(|n| {
            format!(
                "{} ({}, {} facts, {})",
                n.name,
                n.type_label.as_deref().unwrap_or("untyped"),
                n.degree,
                n.id
            )
        })
        .collect();
    let mut note = format!(
        "\"{raw}\" = {} ({}, {} facts)",
        first.name,
        first.type_label.as_deref().unwrap_or("untyped"),
        first.degree
    );
    if by_question {
        note.push_str(", closest to the question");
    }
    if !others.is_empty() {
        note.push_str(&format!(
            "; other matches: {}. Pass an id to choose another.",
            others.join("; ")
        ));
    }
    Ok(Resolved {
        id: first.id,
        name: first.name.clone(),
        note: Some(note),
    })
}

/// 时刻参数三件套：`at` 世界轴，`as_of` / `before` 记录轴（before 优先，减一微秒）
struct Moments {
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    before: Option<chrono::DateTime<chrono::Utc>>,
}

fn moments(args: &Value) -> Moments {
    let at = args["at"].as_str().and_then(parse_when);
    let before = args["before"].as_str().and_then(parse_when);
    let as_of = match before {
        Some(t) => Some(just_before(t)),
        None => args["as_of"].as_str().and_then(parse_when),
    };
    Moments { at, as_of, before }
}

fn limit_of(args: &Value, default: usize) -> usize {
    args["limit"]
        .as_u64()
        .map(|n| n as usize)
        .filter(|n| *n > 0)
        .unwrap_or(default)
        .min(LIST_MAX)
}

fn contains_ci(haystack: Option<&str>, needle: &str) -> bool {
    haystack.is_some_and(|h| h.to_lowercase().contains(&needle.to_lowercase()))
}

// ---- find_entities ---------------------------------------------------------------

pub async fn find_entities(ctx: &ToolCtx<'_>, sink: &mut ToolSink, args: &Value) -> ToolResult {
    let name = args["name"].as_str().unwrap_or("").to_string();
    let hits = match lookup(ctx, &name).await {
        Ok(hits) => hits,
        Err(e) => {
            tracing::warn!(error = %e, "Entity lookup failed");
            return ToolResult::new(
                "Could not look up entities.".into(),
                json!({"kind": "entity", "label": name, "detail": "failed"}),
            )
            .error();
        }
    };
    let (ranked, by_question) = rank_by_question(ctx, rank(hits, &name), &name).await;
    let text = if ranked.is_empty() {
        "No matching entities.".to_string()
    } else if by_question || dominant(&ranked, &name) {
        let mut lines = vec![format!("Best match: {}", node_line(&ranked[0]))];
        if ranked.len() > 1 {
            lines.push("Other matches:".to_string());
            lines.extend(ranked[1..].iter().map(node_line));
        }
        lines.join("\n")
    } else {
        let mut lines = vec![format!(
            "Several entities match \"{name}\"; pick by type and fact count, or ask the user:"
        )];
        lines.extend(ranked.iter().map(node_line));
        lines.join("\n")
    };
    for n in &ranked {
        remember(sink, n);
    }
    ToolResult::new(
        text,
        json!({ "kind": "entity", "label": name, "detail": format!("{} matches", ranked.len()) }),
    )
    .structured(json!({
        "kb_id": ctx.kb_id,
        "entities": ranked.iter().map(|n| json!({
            "id": n.id, "name": n.name, "type_key": n.type_key,
            "type_label": n.type_label, "disambiguator": n.disambiguator,
            "fact_count": n.degree,
        })).collect::<Vec<_>>()
    }))
}

// ---- entity_facts ----------------------------------------------------------------

/// 事实的另一端：对端实体，或属性值
/// 边上的属性（0037）跟在对端后面：`Vega Capital [amount: 5000000000 $]`。
/// 模型读事实行时最常问的就是"投了多少"，数不在行里它就答"没有金额信息"
fn qualifiers_text(f: &EntityFact) -> String {
    if f.qualifiers.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = f
        .qualifiers
        .iter()
        .map(|q| {
            let v = q
                .value
                .as_ref()
                .and_then(|v| v.get("value"))
                .map(|v| v.to_string().trim_matches('"').to_string())
                .or_else(|| q.entity_name.clone())
                .unwrap_or_else(|| "?".to_string());
            let u = q
                .value
                .as_ref()
                .and_then(|v| v.get("unit"))
                .and_then(|u| u.as_str())
                .map(|u| format!(" {u}"))
                .unwrap_or_default();
            format!("{}: {v}{u}", q.key)
        })
        .collect();
    format!(" [{}]", parts.join(", "))
}

fn other_text(f: &EntityFact) -> String {
    let literal = f
        .object_value
        .as_ref()
        .and_then(literal_text)
        .filter(|_| f.other_name.is_none());
    f.other_name
        .as_deref()
        .or(literal.as_deref())
        .unwrap_or("?")
        .to_string()
}

fn range_text(f: &EntityFact) -> String {
    let range = crate::time_text::span(crate::time_text::Span {
        valid_from: f.valid_from,
        from_precision: f.valid_from_precision.as_deref(),
        valid_to: f.valid_to,
        to_precision: f.valid_to_precision.as_deref(),
        holds_from: f.holds_from,
        holds_to: f.holds_to,
    });
    if range.is_empty() {
        range
    } else {
        format!(" ({range})")
    }
}

fn confidence_text(f: &EntityFact) -> String {
    format!("[{}%]", (f.confidence * 100.0).round() as i32)
}

/// 谓词组的抬头：出边 `pred →`，入边 `← pred`
fn group_key(f: &EntityFact) -> String {
    let pred = f.predicate_label.as_deref().unwrap_or("?");
    if f.direction == "out" {
        format!("{pred} →")
    } else {
        format!("← {pred}")
    }
}

/// 按谓词分组，多的组在前；组内保持传入顺序（时间序）。回 (组名, 该组事实)
pub(super) fn grouped(facts: &[EntityFact]) -> Vec<(String, Vec<&EntityFact>)> {
    let mut groups: BTreeMap<String, Vec<&EntityFact>> = BTreeMap::new();
    for f in facts {
        groups.entry(group_key(f)).or_default().push(f);
    }
    let mut out: Vec<(String, Vec<&EntityFact>)> = groups.into_iter().collect();
    out.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
    out
}

/// 过滤没命中时告诉模型这个实体身上有哪些谓词：它猜的词（"board member"）和
/// 库里的词（`comprised`、`has_member`）常常对不上（#560），空手而回它只会再猜一次
pub(super) fn predicates_of(facts: &[EntityFact]) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in facts {
        *counts.entry(group_key(f)).or_default() += 1;
    }
    let mut named: Vec<(String, usize)> = counts.into_iter().collect();
    named.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    named
        .iter()
        .take(20)
        .map(|(k, n)| format!("{k} ({n})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// 子串找不到时按词找：每个词各去库里捞一把，名字里含的词数达到「全部减一、至少两个」
/// 的候选算命中（"OpenAI board members" → "OpenAI's board of directors"）。
/// 模型给的名字常带一个库里没有的词（members、公司、这个），全词命中会把它们全漏掉
async fn lookup(ctx: &ToolCtx<'_>, raw: &str) -> utopia_core::AppResult<Vec<GraphNode>> {
    let (hits, _) =
        utopia_store::graph::search_entities(&ctx.state.pool, ctx.kb_id, raw, 8, 0).await?;
    if !hits.is_empty() {
        return Ok(hits);
    }
    let words: Vec<&str> = raw.split_whitespace().filter(|w| w.len() >= 2).collect();
    if words.len() < 2 {
        return Ok(hits);
    }
    let mut pool: Vec<GraphNode> = Vec::new();
    for w in &words {
        let (found, _) =
            utopia_store::graph::search_entities(&ctx.state.pool, ctx.kb_id, w, 40, 0).await?;
        for n in found {
            if !pool.iter().any(|p| p.id == n.id) {
                pool.push(n);
            }
        }
    }
    let need = words.len().saturating_sub(1).max(2);
    let mut scored: Vec<(usize, GraphNode)> = pool
        .into_iter()
        .map(|n| (word_score(&n.name, &words), n))
        .filter(|(s, _)| *s >= need)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.degree.cmp(&a.1.degree)));
    Ok(scored.into_iter().map(|(_, n)| n).take(8).collect())
}

/// 名字里含了几个词（大小写不敏感）
pub(super) fn word_score(name: &str, words: &[&str]) -> usize {
    let lower = name.to_lowercase();
    words
        .iter()
        .filter(|w| lower.contains(&w.to_lowercase()))
        .count()
}

/// 同名候选按问题排：用户的问题嵌一次，与候选的上下文画像比，近的在前。
/// 精确命中仍在前（问 OpenAI 就先给叫 OpenAI 的），画像只在同一档里排序。
/// 没有问题（MCP）、没有嵌入模型、只有一个候选：原样返回，第二个值说明有没有用上
async fn rank_by_question(
    ctx: &ToolCtx<'_>,
    ranked: Vec<GraphNode>,
    query: &str,
) -> (Vec<GraphNode>, bool) {
    if ranked.len() < 2 {
        return (ranked, false);
    }
    let Some(question) = ctx.question else {
        return (ranked, false);
    };
    let Some(vec) = ctx.embed(question).await else {
        return (ranked, false);
    };
    let ids: Vec<Uuid> = ranked.iter().map(|n| n.id).collect();
    let Ok(dist) =
        utopia_store::graph::profile_distances(&ctx.state.pool, ctx.kb_id, &ids, &vec).await
    else {
        return (ranked, false);
    };
    let dist: HashMap<Uuid, f64> = dist.into_iter().collect();
    (reorder_by_distance(ranked, query, &dist), true)
}

/// 纯函数：精确命中的一档在前；档内按画像距离升序，没有画像的垫底；再按事实数
pub(super) fn reorder_by_distance(
    mut ranked: Vec<GraphNode>,
    query: &str,
    dist: &HashMap<Uuid, f64>,
) -> Vec<GraphNode> {
    let q = query.trim().to_lowercase();
    let exact = |n: &GraphNode| n.name.trim().to_lowercase() == q;
    ranked.sort_by(|a, b| {
        exact(b)
            .cmp(&exact(a))
            .then_with(|| {
                let da = dist.get(&a.id).copied().unwrap_or(f64::MAX);
                let db = dist.get(&b.id).copied().unwrap_or(f64::MAX);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .then(b.degree.cmp(&a.degree))
    });
    ranked
}

/// 谓词参数与库里的词对齐：模型说 "board member"，库里叫 `has_member`。子串对不上时
/// 把它嵌一次，取最近的关系类型（关系向量在 `relation_types.embedding`），距离在
/// 上限内的算它说的是那几个。自动扩本体造出的关系没有向量（#560），对不到
const PREDICATE_DISTANCE: f32 = 0.45;
const PREDICATE_CANDIDATES: i64 = 5;

async fn aligned_predicates(ctx: &ToolCtx<'_>, word: &str) -> Vec<String> {
    let Some(vec) = ctx.embed(word).await else {
        return Vec::new();
    };
    let near = utopia_store::ontology::nearest_relation_types(
        &ctx.state.pool,
        ctx.kb_id,
        &vec,
        PREDICATE_CANDIDATES,
        None,
    )
    .await
    .unwrap_or_default();
    for c in &near {
        tracing::debug!(word, key = c.key, distance = c.distance, "谓词对齐候选");
    }
    near.into_iter()
        .filter(|c| c.distance <= PREDICATE_DISTANCE)
        .map(|c| c.key)
        .collect()
}

/// 过滤；谓词按子串对不上时向量对齐一次，回一句说明给结果开头
async fn filtered<'f>(
    ctx: &ToolCtx<'_>,
    facts: &'f [EntityFact],
    filter: &FactFilter<'_>,
) -> (Vec<&'f EntityFact>, Option<String>) {
    let kept: Vec<&EntityFact> = facts.iter().filter(|f| filter.keeps(f)).collect();
    let Some(word) = filter.predicate else {
        return (kept, None);
    };
    if !kept.is_empty() || facts.is_empty() {
        return (kept, None);
    }
    let keys: HashSet<String> = aligned_predicates(ctx, word).await.into_iter().collect();
    if keys.is_empty() {
        return (kept, None);
    }
    let widened = FactFilter {
        predicate: None,
        ..*filter
    };
    let kept: Vec<&EntityFact> = facts
        .iter()
        .filter(|f| {
            widened.keeps(f) && f.predicate_key.as_deref().is_some_and(|k| keys.contains(k))
        })
        .collect();
    let mut names: Vec<&str> = keys.iter().map(String::as_str).collect();
    names.sort();
    let note = format!("predicate \"{word}\" read as {}", names.join(", "));
    (kept, Some(note))
}

#[derive(Clone, Copy)]
struct FactFilter<'a> {
    predicate: Option<&'a str>,
    object_type: Option<&'a str>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
}

impl FactFilter<'_> {
    fn from_args(args: &Value) -> FactFilter<'_> {
        FactFilter {
            predicate: args["predicate"].as_str().filter(|s| !s.trim().is_empty()),
            object_type: args["object_type"]
                .as_str()
                .filter(|s| !s.trim().is_empty()),
            since: args["since"].as_str().and_then(parse_when),
            until: args["until"].as_str().and_then(parse_when),
        }
    }

    fn narrows(&self) -> bool {
        self.predicate.is_some()
            || self.object_type.is_some()
            || self.since.is_some()
            || self.until.is_some()
    }

    /// 时段用**说出来的**区间：起点在 until 之前、终点在 since 之后（开放的终点算在后）
    fn keeps(&self, f: &EntityFact) -> bool {
        if let Some(p) = self.predicate {
            if !contains_ci(f.predicate_key.as_deref(), p)
                && !contains_ci(f.predicate_label.as_deref(), p)
            {
                return false;
            }
        }
        if let Some(t) = self.object_type {
            if !contains_ci(f.other_type.as_deref(), t) {
                return false;
            }
        }
        if let Some(until) = self.until {
            if f.valid_from.is_some_and(|from| from > until) {
                return false;
            }
        }
        if let Some(since) = self.since {
            if f.valid_to.is_some_and(|to| to < since) {
                return false;
            }
        }
        true
    }
}

pub async fn entity_facts(ctx: &ToolCtx<'_>, sink: &mut ToolSink, args: &Value) -> ToolResult {
    let raw = args["entity_id"]
        .as_str()
        .or_else(|| args["entity"].as_str())
        .unwrap_or("");
    let who = match resolve(ctx, sink, raw).await {
        Ok(r) => r,
        Err(ResolveError::ReadFailed) => {
            return ToolResult::new(
                "Could not look up entities.".into(),
                json!({ "kind": "facts", "label": "?", "detail": "failed" }),
            )
            .error();
        }
        Err(ResolveError::Unresolved(e)) => {
            return ToolResult::new(
                format!(
                    "Invalid entity: {e} (expected a name, or the uuid returned by find_entities)."
                ),
                json!({ "kind": "facts", "label": "?", "detail": "invalid id" }),
            )
            .error()
        }
    };
    let m = moments(args);
    let filter = FactFilter::from_args(args);
    let limit = limit_of(args, FACTS_DEFAULT);
    let (node, facts) =
        match utopia_store::graph::entity_detail(&ctx.state.pool, ctx.kb_id, who.id, m.at, m.as_of)
            .await
        {
            Ok(detail) => detail,
            Err(e) => {
                let text = if matches!(e, utopia_core::AppError::NotFound) {
                    "Entity not found."
                } else {
                    tracing::warn!(error = %e, "Entity facts lookup failed");
                    "Could not read the entity facts."
                };
                return ToolResult::new(
                    text.into(),
                    json!({"kind": "facts", "label": "?", "detail": "failed"}),
                )
                .error();
            }
        };
    // 规则的结论也是这个实体的一部分（0021）。**不给的话模型会拿那些读数自己再判
    // 一遍**——而阈值写在规则里，它看不见，于是两处判断迟早不一致
    let derived = match
        // 两根轴一起传（#549）：as_of 回到三月，派生也回到三月
        utopia_store::reasoning::derived_for_entity(
            &ctx.state.pool,
            ctx.kb_id,
            who.id,
            m.at,
            m.as_of,
        )
        .await {
            Ok(derived) => derived,
            Err(e) => {
                tracing::warn!(error = %e, "Derived facts lookup failed");
                return ToolResult::new("Could not read the derived facts.".into(),
                    json!({"kind": "facts", "label": node.name, "detail": "failed"})).error();
            }
        };
    let derived_lines: Vec<String> = derived
        .iter()
        .map(|d| {
            format!(
                "{} · {} · {} [rule: {}]",
                d.subject,
                d.predicate,
                d.object,
                d.rule_name.as_deref().unwrap_or(&d.rule),
            )
        })
        .collect();

    // 名字单独一行（0041）：「海探1」和「海洋探测器1号」是同一个，模型答题时要知道
    let names = utopia_store::names::for_entity(&ctx.state.pool, ctx.kb_id, who.id, m.as_of)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Entity names lookup failed");
            Vec::new()
        });
    let (kept, aligned) = filtered(ctx, &facts, &filter).await;
    let shown: Vec<&EntityFact> = kept.iter().copied().take(limit).collect();
    let mut lines: Vec<String> = Vec::new();
    if let Some(note) = &who.note {
        lines.push(note.clone());
    }
    let other_names: Vec<&str> = names
        .iter()
        .filter(|n| !n.canonical)
        .map(|n| n.name.as_str())
        .collect();
    if !other_names.is_empty() {
        lines.push(format!("Also known as: {}", other_names.join(", ")));
    }
    if let Some(a) = &aligned {
        lines.push(a.clone());
    }
    let type_label = node.type_label.as_deref().unwrap_or("untyped");
    if facts.is_empty() && derived.is_empty() {
        lines.push(match m.at {
            Some(t) => format!(
                "{}: no facts valid as of {}.",
                node.name,
                crate::time_text::instant(t)
            ),
            None => format!("{}: no recorded facts.", node.name),
        });
    } else {
        let mut head = format!("{} ({type_label}) · {} facts", node.name, facts.len());
        if filter.narrows() {
            head.push_str(&format!(", {} match the filter", kept.len()));
            if kept.is_empty() {
                head.push_str(&format!(
                    ". Predicates on this entity: {}",
                    predicates_of(&facts)
                ));
            }
        }
        if shown.len() < kept.len() {
            head.push_str(&format!(
                ", {} shown. Narrow with predicate=, object_type=, since/until; timeline for the \
                 dated ones; neighbors for the related entities",
                shown.len()
            ));
        }
        lines.push(head);
        let shown_owned: Vec<EntityFact> = shown.iter().map(|f| (*f).clone()).collect();
        for (key, group) in grouped(&shown_owned) {
            lines.push(format!("## {key} ({})", group.len()));
            for f in group {
                lines.push(format!(
                    "{}{}{} {}",
                    other_text(f),
                    qualifiers_text(f),
                    range_text(f),
                    confidence_text(f)
                ));
            }
        }
        lines.extend(derived_lines);
    }
    let detail = entity_facts_detail(shown.len(), m.at, m.as_of, m.before);
    ToolResult::new(
        lines.join("\n"),
        json!({ "kind": "facts", "label": node.name, "detail": detail }),
    )
    .structured(json!({
        "kb_id": ctx.kb_id,
        "entity": {"id": node.id, "name": node.name,
            "type_key": node.type_key, "type_label": node.type_label},
        "names": names.iter().map(|n| json!({
            "fact_id": n.fact_id, "name": n.name, "canonical": n.canonical,
            "recorded_at": n.recorded_at, "valid_from": n.valid_from, "valid_to": n.valid_to,
            "document_ids": n.document_ids,
        })).collect::<Vec<_>>(),
        "at": m.at, "as_of": m.as_of, "before": m.before,
        "total_facts": facts.len(), "matched_facts": kept.len(),
        "limit": limit, "truncated": shown.len() < kept.len(),
        "facts": shown.iter().map(|f| json!({
            "id": f.id, "direction": f.direction,
            "predicate_key": f.predicate_key, "predicate_label": f.predicate_label,
            "inferred": f.inferred, "temporal": f.temporal,
            "other_id": f.other_id, "other_name": f.other_name,
            "object_value": f.object_value, "confidence": f.confidence,
            "qualifiers": f.qualifiers.iter().map(|q| json!({
                "qualifier_type_id": q.qualifier_type_id, "key": q.key, "label": q.label,
                "value": q.value, "entity_id": q.entity_id, "entity_name": q.entity_name,
            })).collect::<Vec<_>>(),
            "valid_from": f.valid_from, "valid_to": f.valid_to,
            "valid_from_precision": f.valid_from_precision,
            "valid_to_precision": f.valid_to_precision,
            "holds_from": f.holds_from, "holds_to": f.holds_to,
            "recorded_at": f.recorded_at, "invalidated_at": f.invalidated_at,
            "supersedes": f.supersedes, "document_ids": f.document_ids,
        })).collect::<Vec<_>>(),
        "derived_facts": derived.iter().map(|d| json!({
            "id": d.id, "subject_id": d.subject_id, "subject": d.subject,
            "predicate_id": d.predicate_id, "predicate": d.predicate,
            "object_id": d.object_id, "object": d.object, "object_value": d.object_value,
            "rule": d.rule, "rule_name": d.rule_name,
            "rule_id": d.rule_id, "attribute_rule_id": d.attribute_rule_id,
            "valid_from": d.valid_from, "valid_to": d.valid_to,
            "valid_from_precision": d.valid_from_precision,
            "valid_to_precision": d.valid_to_precision,
            "derived_at": d.derived_at, "invalidated_at": d.invalidated_at,
            "confidence": d.confidence,
        })).collect::<Vec<_>>()
    }))
}

// ---- neighbors -------------------------------------------------------------------

pub async fn neighbors(ctx: &ToolCtx<'_>, sink: &mut ToolSink, args: &Value) -> ToolResult {
    let raw = args["entity"]
        .as_str()
        .or_else(|| args["entity_id"].as_str())
        .unwrap_or("");
    let who = match resolve(ctx, sink, raw).await {
        Ok(r) => r,
        Err(ResolveError::ReadFailed) => {
            return ToolResult::new(
                "Could not look up entities.".into(),
                json!({ "kind": "neighbors", "label": "?", "detail": "failed" }),
            )
            .error();
        }
        Err(ResolveError::Unresolved(e)) => {
            return ToolResult::new(
                format!("Unknown entity: {e}."),
                json!({ "kind": "neighbors", "label": "?", "detail": "unknown entity" }),
            )
        }
    };
    let m = moments(args);
    let filter = FactFilter::from_args(args);
    let limit = limit_of(args, NEIGHBORS_DEFAULT);
    let (node, facts) =
        match utopia_store::graph::entity_detail(&ctx.state.pool, ctx.kb_id, who.id, m.at, m.as_of)
            .await
        {
            Ok(detail) => detail,
            Err(utopia_core::AppError::NotFound) => {
                return ToolResult::new(
                    "Entity not found.".to_string(),
                    json!({ "kind": "neighbors", "label": "?", "detail": "not found" }),
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "Entity neighbors lookup failed");
                return ToolResult::new(
                    "Could not read the entity facts.".into(),
                    json!({ "kind": "neighbors", "label": "?", "detail": "failed" }),
                )
                .error();
            }
        };
    // 邻居是对端**实体**；属性值不算邻居，entity_facts 里有
    let (matched, aligned) = filtered(ctx, &facts, &filter).await;
    let linked: Vec<&EntityFact> = matched
        .into_iter()
        .filter(|f| f.other_id.is_some())
        .collect();
    let shown: Vec<EntityFact> = linked.iter().take(limit).map(|f| (*f).clone()).collect();
    let mut lines: Vec<String> = Vec::new();
    if let Some(note) = &who.note {
        lines.push(note.clone());
    }
    if let Some(a) = &aligned {
        lines.push(a.clone());
    }
    let type_label = node.type_label.as_deref().unwrap_or("untyped");
    if shown.is_empty() {
        let all_linked: Vec<EntityFact> = facts
            .iter()
            .filter(|f| f.other_id.is_some())
            .cloned()
            .collect();
        let hint = if filter.narrows() && !all_linked.is_empty() {
            format!(" Predicates on this entity: {}", predicates_of(&all_linked))
        } else {
            String::new()
        };
        lines.push(format!(
            "{} ({type_label}): no linked entities{}.{hint}",
            node.name,
            if filter.narrows() {
                " match the filter"
            } else {
                ""
            }
        ));
    } else {
        let groups = grouped(&shown);
        // 数的是对端实体，不是事实：同一条边常是两条事实（一条带日期一条不带）
        let entities: HashSet<Uuid> = linked.iter().filter_map(|f| f.other_id).collect();
        let mut head = format!(
            "{} ({type_label}): {} linked entities under {} predicates",
            node.name,
            entities.len(),
            groups.len()
        );
        if shown.len() < linked.len() {
            head.push_str(&format!(
                " ({} shown; narrow with predicate= or object_type=)",
                shown.len()
            ));
        }
        lines.push(head);
        for (key, group) in groups {
            let items: Vec<String> = group
                .iter()
                .map(|f| {
                    let ty = f
                        .other_type
                        .as_deref()
                        .map(|t| format!(" [{t}]"))
                        .unwrap_or_default();
                    format!(
                        "{}{}{} {}",
                        other_text(f),
                        ty,
                        range_text(f),
                        confidence_text(f)
                    )
                })
                .collect();
            lines.push(format!("{key} {}", items.join(" · ")));
        }
    }
    let detail = format!("{} of {} linked", shown.len(), linked.len());
    ToolResult::new(
        lines.join("\n"),
        json!({ "kind": "neighbors", "label": node.name, "detail": detail }),
    )
}

// ---- timeline --------------------------------------------------------------------

pub async fn timeline(ctx: &ToolCtx<'_>, sink: &mut ToolSink, args: &Value) -> ToolResult {
    let raw = args["entity"]
        .as_str()
        .or_else(|| args["entity_id"].as_str())
        .unwrap_or("");
    let who = match resolve(ctx, sink, raw).await {
        Ok(r) => r,
        Err(ResolveError::ReadFailed) => {
            return ToolResult::new(
                "Could not look up entities.".into(),
                json!({ "kind": "timeline", "label": "?", "detail": "failed" }),
            )
            .error();
        }
        Err(ResolveError::Unresolved(e)) => {
            return ToolResult::new(
                format!("Unknown entity: {e}."),
                json!({ "kind": "timeline", "label": "?", "detail": "unknown entity" }),
            )
        }
    };
    let m = moments(args);
    let filter = FactFilter::from_args(args);
    let limit = limit_of(args, TIMELINE_DEFAULT);
    let (node, facts) =
        match utopia_store::graph::entity_detail(&ctx.state.pool, ctx.kb_id, who.id, None, m.as_of)
            .await
        {
            Ok(detail) => detail,
            Err(utopia_core::AppError::NotFound) => {
                return ToolResult::new(
                    "Entity not found.".to_string(),
                    json!({ "kind": "timeline", "label": "?", "detail": "not found" }),
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "Entity timeline lookup failed");
                return ToolResult::new(
                    "Could not read the entity facts.".into(),
                    json!({ "kind": "timeline", "label": "?", "detail": "failed" }),
                )
                .error();
            }
        };
    // 只要**说出了世界时间**的事实。没日期的那些起点是摄取时刻，排进时间线只会
    // 把一篇文章的日期当成事件的日期
    let (matched, aligned) = filtered(ctx, &facts, &filter).await;
    let mut dated: Vec<&EntityFact> = matched
        .into_iter()
        .filter(|f| f.valid_from.is_some())
        .collect();
    dated.sort_by_key(|f| f.valid_from);
    let undated = facts.iter().filter(|f| f.valid_from.is_none()).count();
    let shown: Vec<&EntityFact> = dated.iter().copied().take(limit).collect();
    let mut lines: Vec<String> = Vec::new();
    if let Some(note) = &who.note {
        lines.push(note.clone());
    }
    if let Some(a) = &aligned {
        lines.push(a.clone());
    }
    let type_label = node.type_label.as_deref().unwrap_or("untyped");
    if shown.is_empty() {
        lines.push(format!(
            "{} ({type_label}): no dated facts{}; {undated} facts carry no date (entity_facts lists them).",
            node.name,
            if filter.narrows() { " in that window" } else { "" }
        ));
    } else {
        let first = shown[0].valid_from.expect("dated");
        let last = shown[shown.len() - 1].valid_from.expect("dated");
        let mut head = format!("{} ({type_label}): {} dated facts", node.name, dated.len());
        if shown.len() < dated.len() {
            head.push_str(&format!(
                ", {} shown from {} to {} (pass since/until to see the rest)",
                shown.len(),
                crate::time_text::world(first, shown[0].valid_from_precision.as_deref()),
                crate::time_text::world(
                    last,
                    shown[shown.len() - 1].valid_from_precision.as_deref()
                )
            ));
        }
        if undated > 0 {
            head.push_str(&format!(
                "; {undated} undated facts omitted (entity_facts has them)"
            ));
        }
        lines.push(head);
        for f in &shown {
            let stamp = crate::time_text::world(
                f.valid_from.expect("dated"),
                f.valid_from_precision.as_deref(),
            );
            lines.push(format!("{stamp}  {}", fact_line(f)));
        }
    }
    let detail = format!("{} of {} dated", shown.len(), dated.len());
    ToolResult::new(
        lines.join("\n"),
        json!({ "kind": "timeline", "label": node.name, "detail": detail }),
    )
}

// ---- paths_between ---------------------------------------------------------------

/// 一条边在链上的写法：顺着走 `A —pred→ B`，逆着走 `A ←pred— B`
pub(super) fn edge_text(prev: Uuid, e: &PathEdge) -> String {
    let pred = e.predicate.as_deref().unwrap_or("?");
    let range = crate::time_text::span(crate::time_text::Span {
        valid_from: e.valid_from,
        from_precision: e.valid_from_precision.as_deref(),
        valid_to: e.valid_to,
        to_precision: e.valid_to_precision.as_deref(),
        holds_from: e.holds_from,
        holds_to: e.holds_to,
    });
    let range = if range.is_empty() {
        range
    } else {
        format!(" ({range})")
    };
    let conf = (e.confidence * 100.0).round() as i32;
    if e.subject_id == prev {
        format!(
            "{} —{pred}→ {}{range} [{conf}%]",
            e.subject_name, e.object_name
        )
    } else {
        format!(
            "{} ←{pred}— {}{range} [{conf}%]",
            e.object_name, e.subject_name
        )
    }
}

pub(super) fn path_text(p: &Path) -> String {
    let mut parts = Vec::new();
    for (i, e) in p.edges.iter().enumerate() {
        parts.push(edge_text(p.nodes[i], e));
    }
    parts.join("; ")
}

pub async fn paths_between(ctx: &ToolCtx<'_>, sink: &mut ToolSink, args: &Value) -> ToolResult {
    let mut notes = Vec::new();
    let mut ends = Vec::new();
    for key in ["from", "to"] {
        match resolve(ctx, sink, args[key].as_str().unwrap_or("")).await {
            Ok(r) => {
                if let Some(n) = &r.note {
                    notes.push(n.clone());
                }
                ends.push(r);
            }
            Err(ResolveError::ReadFailed) => {
                return ToolResult::new(
                    "Could not look up entities.".into(),
                    json!({ "kind": "path", "label": "?", "detail": "failed" }),
                )
                .error();
            }
            Err(ResolveError::Unresolved(e)) => {
                return ToolResult::new(
                    format!("Unknown `{key}`: {e}."),
                    json!({ "kind": "path", "label": "?", "detail": "unknown entity" }),
                )
            }
        }
    }
    let (from, to) = (&ends[0], &ends[1]);
    let m = moments(args);
    let max_hops = args["max_hops"]
        .as_u64()
        .map(|n| n as usize)
        .unwrap_or(3)
        .clamp(1, 3);
    let limits = Limits {
        max_hops,
        ..Limits::default()
    };
    let paths = match utopia_store::paths::paths_between(
        &ctx.state.pool,
        ctx.kb_id,
        from.id,
        to.id,
        m.at,
        m.as_of,
        limits,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "Path search failed");
            return ToolResult::new(
                "Could not search paths.".into(),
                json!({ "kind": "path", "label": "?", "detail": "failed" }),
            )
            .error();
        }
    };
    let label = format!("{} ↔ {}", from.name, to.name);
    let when = match (m.at, m.before, m.as_of) {
        (Some(t), _, _) => format!(" at {}", crate::time_text::world(t, Some("day"))),
        (None, Some(b), _) => format!(" as recorded before {}", crate::time_text::instant(b)),
        (None, None, Some(r)) => format!(" as recorded by {}", crate::time_text::instant(r)),
        (None, None, None) => String::new(),
    };
    let mut lines = notes;
    if paths.is_empty() {
        lines.push(format!(
            "No path of up to {max_hops} hop{} between {} and {}{when}. They may be unrelated in \
             this base, or connected only through a hub; try neighbors on each.",
            if max_hops == 1 { "" } else { "s" },
            from.name,
            to.name
        ));
        return ToolResult::new(
            lines.join("\n"),
            json!({ "kind": "path", "label": label, "detail": "no path" }),
        );
    }
    let shortest = paths[0].hops();
    lines.push(format!(
        "{} path{} between {} and {}{when} (up to {max_hops} hops, shortest first):",
        paths.len(),
        if paths.len() == 1 { "" } else { "s" },
        from.name,
        to.name
    ));
    for (i, p) in paths.iter().enumerate() {
        lines.push(format!("{}. {}", i + 1, path_text(p)));
    }
    let detail = format!(
        "{} path{}, shortest {shortest} hop{}",
        paths.len(),
        if paths.len() == 1 { "" } else { "s" },
        if shortest == 1 { "" } else { "s" }
    );
    ToolResult::new(
        lines.join("\n"),
        json!({ "kind": "path", "label": label, "detail": detail }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, ty: Option<&str>, degree: i64) -> GraphNode {
        GraphNode {
            id: Uuid::now_v7(),
            name: name.into(),
            type_key: ty.map(|t| t.to_lowercase()),
            type_label: ty.map(String::from),
            color: "#000".into(),
            shape: "circle".into(),
            degree,
            disambiguator: None,
        }
    }

    /// 短语实体（untyped）不列，精确命中在前，事实多的在前
    #[test]
    fn a_name_match_is_ranked_by_type_exactness_and_weight() {
        let hits = vec![
            node("lawsuit against OpenAI", None, 5),
            node("OpenAI's board of directors", Some("Organization"), 23),
            node("OpenAI", Some("ResearchProject"), 555),
            node("OpenAI LP", Some("Corporation"), 6),
        ];
        let ranked = rank(hits, "OpenAI");
        let names: Vec<&str> = ranked.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            ["OpenAI", "OpenAI's board of directors", "OpenAI LP"],
            "untyped 短语不列，精确命中第一"
        );
        assert!(
            dominant(&ranked, "OpenAI"),
            "精确命中且事实数是第二名的三倍"
        );
    }

    /// 精确命中的 untyped 实体不被同名的有类型实体挤掉
    #[test]
    fn an_exact_untyped_match_is_not_hidden_by_a_typed_namesake() {
        let ranked = rank(
            vec![
                node("Acme lawsuit", Some("Event"), 1),
                node("Acme", None, 200),
                node("Acme corporate history", None, 4),
            ],
            "Acme",
        );
        let names: Vec<&str> = ranked.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            ["Acme", "Acme lawsuit"],
            "叫这个名字的 untyped 照列且在前；别的 untyped 短语仍不列"
        );
        assert!(dominant(&ranked, "Acme"), "只有它精确命中");
    }

    #[test]
    fn a_base_with_no_types_still_lists_its_entities() {
        let ranked = rank(
            vec![node("Acme", None, 3), node("Acme Corp", None, 9)],
            "acme",
        );
        let names: Vec<&str> = ranked.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            ["Acme", "Acme Corp"],
            "全 untyped 时照列，精确命中在前"
        );
        assert!(dominant(&ranked, "acme"));
    }

    /// 谁都不明显：两个同名同量级的实体，不替模型挑
    #[test]
    fn two_namesakes_of_equal_weight_are_not_dominant() {
        let ranked = rank(
            vec![
                node("Zhang Wei", Some("Person"), 12),
                node("Zhang Wei", Some("Person"), 10),
            ],
            "Zhang Wei",
        );
        assert!(!dominant(&ranked, "Zhang Wei"));
        let ranked = rank(
            vec![
                node("Zhang Wei", Some("Person"), 40),
                node("Zhang Wei", Some("Person"), 10),
            ],
            "Zhang Wei",
        );
        assert!(dominant(&ranked, "Zhang Wei"), "事实数三倍以上就明显");
    }

    fn fact(direction: &str, pred: &str, other: &str, other_type: Option<&str>) -> EntityFact {
        EntityFact {
            recorded_at: chrono::Utc::now(),
            invalidated_at: None,
            supersedes: None,
            document_ids: vec![],
            id: Uuid::now_v7(),
            direction: direction.into(),
            predicate_key: Some(pred.into()),
            predicate_label: Some(pred.into()),
            inferred: false,
            temporal: Some("state".into()),
            other_id: Some(Uuid::now_v7()),
            other_name: Some(other.into()),
            other_type: other_type.map(String::from),
            qualifiers: Vec::new(),
            object_value: None,
            valid_from: Some("2021-01-01T00:00:00Z".parse().unwrap()),
            valid_to: None,
            valid_from_precision: Some("year".into()),
            valid_to_precision: None,
            holds_from: Some("2021-01-01T00:00:00Z".parse().unwrap()),
            holds_to: None,
            confidence: 0.9,
            evidence_count: 1,
            stale: false,
            corrected: false,
            last_evidence_time: None,
            contested: None,
        }
    }

    /// 问题的向量只在同一档里排序：精确命中的仍在前，档内近的先，没画像的垫底
    #[test]
    fn the_question_orders_namesakes_but_not_over_an_exact_match() {
        let exact_far = node("Sam Altman", Some("Organization"), 66);
        let exact_near = node("Sam Altman", Some("Person"), 27);
        let partial_nearest = node("Sam Altman's efforts", Some("Event"), 2);
        let no_profile = node("Sam Altman Jr", Some("Person"), 1);
        let mut dist = HashMap::new();
        dist.insert(exact_far.id, 0.40);
        dist.insert(exact_near.id, 0.20);
        dist.insert(partial_nearest.id, 0.05);
        let ranked = reorder_by_distance(
            vec![
                exact_far.clone(),
                no_profile.clone(),
                partial_nearest.clone(),
                exact_near.clone(),
            ],
            "Sam Altman",
            &dist,
        );
        let ids: Vec<Uuid> = ranked.iter().map(|n| n.id).collect();
        assert_eq!(
            ids,
            vec![
                exact_near.id,
                exact_far.id,
                partial_nearest.id,
                no_profile.id
            ]
        );
    }

    #[test]
    fn a_multi_word_name_is_scored_by_the_words_it_contains() {
        let words = ["OpenAI", "board", "members"];
        assert_eq!(word_score("OpenAI's board of directors", &words), 2);
        assert_eq!(word_score("OpenAI LP", &words), 1);
        assert_eq!(word_score("openai board members list", &words), 3);
    }

    /// 空手而回时把实体身上的谓词报出来，多的在前
    #[test]
    fn an_empty_filter_names_the_predicates_that_exist() {
        let facts = vec![
            fact("out", "comprised", "Ilya Sutskever", Some("Person")),
            fact("out", "comprised", "Helen Toner", Some("Person")),
            fact("in", "removed", "Sam Altman", Some("Person")),
        ];
        assert_eq!(predicates_of(&facts), "comprised → (2), ← removed (1)");
    }

    /// 分组：多的组在前，出边 `pred →`，入边 `← pred`
    #[test]
    fn facts_group_by_predicate_largest_group_first() {
        let facts = vec![
            fact("out", "employee", "Bob", Some("Person")),
            fact("in", "founder", "Carol", Some("Person")),
            fact("out", "employee", "Dan", Some("Person")),
        ];
        let groups = grouped(&facts);
        let keys: Vec<&str> = groups.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["employee →", "← founder"]);
        assert_eq!(groups[0].1.len(), 2);
    }

    /// 过滤：谓词按子串、对象类型按子串、时段按说出来的区间
    #[test]
    fn a_filter_reads_predicate_type_and_window() {
        let mut f = fact("out", "works_for", "Acme", Some("Organization"));
        f.valid_from = Some("2019-01-01T00:00:00Z".parse().unwrap());
        f.valid_to = Some("2020-06-01T00:00:00Z".parse().unwrap());
        fn filter<'a>(
            p: Option<&'a str>,
            t: Option<&'a str>,
            since: Option<&str>,
            until: Option<&str>,
        ) -> FactFilter<'a> {
            FactFilter {
                predicate: p,
                object_type: t,
                since: since.map(|s| s.parse().unwrap()),
                until: until.map(|s| s.parse().unwrap()),
            }
        }
        assert!(filter(Some("works"), None, None, None).keeps(&f));
        assert!(!filter(Some("founder"), None, None, None).keeps(&f));
        assert!(filter(None, Some("organ"), None, None).keeps(&f));
        assert!(!filter(None, Some("person"), None, None).keeps(&f));
        assert!(
            filter(None, None, Some("2020-01-01T00:00:00Z"), None).keeps(&f),
            "2020 年初它还在"
        );
        assert!(
            !filter(None, None, Some("2021-01-01T00:00:00Z"), None).keeps(&f),
            "2021 年它已结束"
        );
        assert!(
            !filter(None, None, None, Some("2018-06-01T00:00:00Z")).keeps(&f),
            "2018 年它还没开始"
        );
        assert!(!filter(None, None, None, None).narrows());
    }

    /// 链的写法：顺着走用 →，逆着走用 ←，每条边带自己的区间
    #[test]
    fn a_chain_reads_in_walking_order() {
        let (a, x, b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let edge = |s: Uuid, sn: &str, o: Uuid, on: &str, pred: &str| PathEdge {
            fact_id: Uuid::now_v7(),
            subject_id: s,
            subject_name: sn.into(),
            object_id: o,
            object_name: on.into(),
            predicate: Some(pred.into()),
            valid_from: Some("2021-01-01T00:00:00Z".parse().unwrap()),
            valid_from_precision: Some("year".into()),
            valid_to: None,
            valid_to_precision: None,
            holds_from: Some("2021-01-01T00:00:00Z".parse().unwrap()),
            holds_to: None,
            confidence: 0.9,
        };
        let p = Path {
            nodes: vec![a, x, b],
            edges: vec![
                edge(a, "OpenAI", x, "Dario Amodei", "employee"),
                edge(x, "Dario Amodei", b, "Anthropic", "founder"),
            ],
            specificity: 0.0,
        };
        assert_eq!(
            path_text(&p),
            "OpenAI —employee→ Dario Amodei (2021 → now) [90%]; Dario Amodei —founder→ Anthropic (2021 → now) [90%]"
        );
        let back = Path {
            nodes: vec![b, x, a],
            edges: vec![
                edge(x, "Dario Amodei", b, "Anthropic", "founder"),
                edge(a, "OpenAI", x, "Dario Amodei", "employee"),
            ],
            specificity: 0.0,
        };
        assert_eq!(
            path_text(&back),
            "Anthropic ←founder— Dario Amodei (2021 → now) [90%]; Dario Amodei ←employee— OpenAI (2021 → now) [90%]"
        );
    }
}
