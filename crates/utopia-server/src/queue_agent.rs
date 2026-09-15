//! 审核台其余几档交给 agent（0043）：低置信与证据过期的事实、时态冲突。
//!
//! 与重复对的治理（`governance`）同一个开关、同一个任务、同一张决定表、同一根保险丝，
//! 走同样的路：先读这个库里人在这一档上怎么定的，一批带编号的项问一次模型，把握够
//! （`AUTO_CONF`）就走人裁决时的那条路动手，不够就留一条建议等人。
//!
//! **动手的每一步都撤得回**，撤回要用的东西记在决定行的 `detail` 里：确认一条事实记下原来
//! 的置信度、补上的那条证据；驳回一条事实靠恢复它；关上旧值、改新值的起点记下改写出来的
//! 行与原行。人撤回一次，保险丝照重复对那样数。
//!
//! 模型给的日期与引文**由这里核对**，不照单全收：日期得是证据里看得到的——文档自己的
//! 日期、事实的起点、或引文里写着的那一天；过期事实的引文得原样出现在文档现在的那段里。
//! 核对不过的，有把握也只留建议。

use crate::llm_util;
use crate::state::AppState;
use chrono::{DateTime, Datelike, Utc};
use serde_json::{json, Value};
use utopia_core::models::{AgentDecisionView, LlmSettings};
use utopia_extract::queue_agent::{
    self as prompts, ConflictQuestion, EvidenceLine, FactCard, FactQuestion, QueueVerdict,
};
use utopia_llm::LlmClient;
use utopia_store::governance::{self as gov, NewDecision, Target, AUTO_CONF};
use utopia_store::queue_agent::{self as queues, Evidence, FactRow};
use uuid::Uuid;

/// 一次模型调用带几项
const BATCH: i64 = 8;
/// 一个任务里每一档最多问几批，之后再排一个接着走
const MAX_BATCHES: usize = 10;
/// 先例带几条
const PRECEDENTS: i64 = 8;

pub(crate) struct Ctx<'a> {
    pub state: &'a AppState,
    pub kb_id: Uuid,
    pub run_id: Uuid,
    pub client: &'a LlmClient,
    pub settings: &'a Option<LlmSettings>,
}

/// 走一遍事实与冲突两档。回 true = 批数用完还有积压
pub(crate) async fn run(ctx: &Ctx<'_>) -> anyhow::Result<bool> {
    let pool = &ctx.state.pool;
    for _ in 0..MAX_BATCHES {
        if !utopia_store::kbs::get(pool, ctx.kb_id).await?.governance {
            return Ok(false);
        }
        let items = queues::fact_queue(pool, ctx.kb_id, BATCH).await?;
        if items.is_empty() {
            break;
        }
        facts(ctx, items).await?;
        ctx.state.emit_review(ctx.kb_id);
    }
    for _ in 0..MAX_BATCHES {
        if !utopia_store::kbs::get(pool, ctx.kb_id).await?.governance {
            return Ok(false);
        }
        let items = queues::conflict_queue(pool, ctx.kb_id, BATCH).await?;
        if items.is_empty() {
            break;
        }
        conflicts(ctx, items).await?;
        ctx.state.emit_review(ctx.kb_id);
    }
    queues::has_backlog(pool, ctx.kb_id)
        .await
        .map_err(Into::into)
}

async fn ask(
    ctx: &Ctx<'_>,
    messages: &[utopia_llm::ChatMessage],
) -> anyhow::Result<Vec<QueueVerdict>> {
    let reply = {
        let _permit = match ctx.settings.as_ref() {
            Some(s) => llm_util::acquire_chat(ctx.state, s).await,
            None => None,
        };
        ctx.client.chat(messages).await?
    };
    prompts::parse_verdicts(&reply)
}

fn day(t: DateTime<Utc>, precision: Option<&str>) -> String {
    match precision {
        Some("year") => t.format("%Y").to_string(),
        Some("month") => t.format("%Y-%m").to_string(),
        _ => t.format("%Y-%m-%d").to_string(),
    }
}

fn card(f: &FactRow, evidence: &[Evidence]) -> FactCard {
    FactCard {
        subject: f.subject.clone(),
        predicate: f.predicate.clone().unwrap_or_default(),
        object: f.object.clone().unwrap_or_default(),
        from: f
            .valid_from
            .map(|t| day(t, f.valid_from_precision.as_deref())),
        to: match (f.valid_to, f.valid_to_precision.as_deref()) {
            (Some(t), p) => Some(day(t, p)),
            (None, Some("unknown")) => Some("ended, date unknown".into()),
            _ => None,
        },
        confidence: f.confidence,
        evidence: evidence
            .iter()
            .map(|e| EvidenceLine {
                document: e.document.clone(),
                dated: e.dated.map(|t| t.format("%Y-%m-%d").to_string()),
                quote: e.quote.clone(),
            })
            .collect(),
    }
}

/// 给人读的一行：做决定那一刻这一项的样子
fn summary(f: &FactRow) -> String {
    let c = card(f, &[]);
    let when = match (&c.from, &c.to) {
        (None, None) => String::new(),
        (from, to) => format!(
            " [{} → {}]",
            from.as_deref().unwrap_or("?"),
            to.as_deref().unwrap_or("now")
        ),
    };
    format!("{} · {} · {}{when}", c.subject, c.predicate, c.object)
}

fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 模型说的日期在不在证据里：事实的起点、文档自己的日期，或引文里写着的那一天。
/// 回核对过的日期与精度
fn date_in_evidence(
    said: Option<&str>,
    fact: &FactRow,
    evidence: &[Evidence],
) -> Option<(DateTime<Utc>, &'static str)> {
    let (at, precision) = utopia_extract::read_time(said?)?;
    let same_day = |t: DateTime<Utc>| t.date_naive() == at.date_naive();
    if fact.valid_from.is_some_and(same_day)
        || evidence.iter().any(|e| e.dated.is_some_and(same_day))
    {
        return Some((at, precision));
    }
    let d = at.date_naive();
    let written = [
        d.format("%Y-%m-%d").to_string(),
        d.format("%B %-d, %Y").to_string(),
        d.format("%-d %B %Y").to_string(),
        d.format("%b %-d, %Y").to_string(),
        format!("{}年{}月{}日", d.year(), d.month(), d.day()),
    ];
    evidence
        .iter()
        .filter_map(|e| e.quote.as_deref())
        .map(normalized)
        .any(|q| written.iter().any(|w| q.contains(&normalized(w))))
        .then_some((at, precision))
}

fn verdict_for(verdicts: &[QueueVerdict], i: usize) -> Option<&QueueVerdict> {
    verdicts.iter().find(|v| v.i == i)
}

/// 一次动手的结果：动成了记在 detail 里，没动成（核对没过、行已经变了）说为什么
enum Done {
    Applied(Value),
    Held(String),
}

async fn facts(ctx: &Ctx<'_>, items: Vec<FactRow>) -> anyhow::Result<()> {
    let pool = &ctx.state.pool;
    let precedents = queues::precedents(
        pool,
        ctx.kb_id,
        &["fact.confirm", "fact.reject"],
        PRECEDENTS,
    )
    .await?;
    let mut evidence = Vec::with_capacity(items.len());
    let mut current = Vec::with_capacity(items.len());
    for f in &items {
        evidence.push(queues::evidence(pool, f.id).await?);
        current.push(if f.stale {
            queues::current_text(pool, f.id, &f.subject).await?
        } else {
            Vec::new()
        });
    }
    let questions: Vec<FactQuestion> = items
        .iter()
        .enumerate()
        .map(|(i, f)| FactQuestion {
            fact: card(f, &evidence[i]),
            stale: f.stale,
            current: current[i].iter().map(|(_, t)| t.clone()).collect(),
            precedents: precedents.clone(),
        })
        .collect();
    let verdicts = ask(ctx, &prompts::fact_messages(&questions)).await?;

    for (i, f) in items.iter().enumerate() {
        let v = verdict_for(&verdicts, i);
        let action = v
            .map(|v| v.action.as_str())
            .filter(|a| matches!(*a, "confirm" | "reject"))
            .unwrap_or("unsure");
        let conf = v.and_then(|v| v.confidence).unwrap_or(0.0).clamp(0.0, 1.0);
        let why = v.and_then(|v| v.why.clone());
        let params = json!({ "quote": v.and_then(|v| v.quote.clone()) });
        let done = if action != "unsure" && conf >= AUTO_CONF {
            Some(perform_fact(ctx.state, ctx.kb_id, f, action, &params, &current[i]).await?)
        } else {
            None
        };
        settle_new(
            ctx,
            "fact",
            f.id,
            &summary(f),
            action,
            conf,
            why,
            params,
            done,
            "fact",
        )
        .await?;
    }
    Ok(())
}

/// 事实那两档的一个出路，走人裁决时的那条路
async fn perform_fact(
    state: &AppState,
    kb_id: Uuid,
    f: &FactRow,
    action: &str,
    params: &Value,
    current: &[(Uuid, String)],
) -> anyhow::Result<Done> {
    let pool = &state.pool;
    match action {
        "confirm" if f.stale => {
            // 过期的事实：文档现在那段里原样说着它，补上这条证据，它就不再是过期的
            let Some(quote) = params["quote"].as_str().filter(|q| !q.trim().is_empty()) else {
                return Ok(Done::Held("no quote from the current text".into()));
            };
            let Some((chunk_id, _)) = current
                .iter()
                .find(|(_, text)| normalized(text).contains(&normalized(quote)))
            else {
                return Ok(Done::Held("the quote is not in the current text".into()));
            };
            utopia_store::graph::add_evidence(pool, f.id, *chunk_id, Some(quote), None).await?;
            let fact_id = f.id;
            Ok(Done::Applied(
                json!({ "evidence": { "fact_id": fact_id, "chunk_id": chunk_id } }),
            ))
        }
        "confirm" => match utopia_store::temporal::set_confidence(pool, kb_id, f.id, 1.0).await? {
            Some(prior) => Ok(Done::Applied(json!({ "prior_confidence": prior }))),
            None => Ok(Done::Held(
                "the fact changed before it could be confirmed".into(),
            )),
        },
        "reject" => {
            if utopia_store::temporal::retract(pool, kb_id, f.id).await? {
                Ok(Done::Applied(json!({ "retracted": f.id })))
            } else {
                Ok(Done::Held(
                    "the fact changed before it could be rejected".into(),
                ))
            }
        }
        other => Ok(Done::Held(format!("no action {other} for a fact"))),
    }
}

async fn conflicts(ctx: &Ctx<'_>, items: Vec<queues::ConflictRow>) -> anyhow::Result<()> {
    let pool = &ctx.state.pool;
    let precedents = queues::precedents(
        pool,
        ctx.kb_id,
        &[
            "conflict.close_old",
            "conflict.keep_both",
            "conflict.reject_new",
        ],
        PRECEDENTS,
    )
    .await?;
    struct Loaded {
        row: queues::ConflictRow,
        old: FactRow,
        new: FactRow,
        new_evidence: Vec<Evidence>,
        question: ConflictQuestion,
    }
    let mut loaded = Vec::new();
    for row in items {
        let (Some(old), Some(new)) = (
            queues::fact(pool, ctx.kb_id, row.old_fact_id).await?,
            queues::fact(pool, ctx.kb_id, row.new_fact_id).await?,
        ) else {
            continue;
        };
        let old_evidence = queues::evidence(pool, old.id).await?;
        let new_evidence = queues::evidence(pool, new.id).await?;
        let neighbours = queues::neighbours(pool, ctx.kb_id, old.id).await?;
        let mut cards = Vec::new();
        for n in neighbours.iter().filter(|n| n.id != new.id) {
            cards.push(card(n, &queues::evidence(pool, n.id).await?));
        }
        let question = ConflictQuestion {
            reason: row.reason.clone(),
            old: card(&old, &old_evidence),
            new: card(&new, &new_evidence),
            neighbours: cards,
            precedents: precedents.clone(),
        };
        loaded.push(Loaded {
            row,
            old,
            new,
            new_evidence,
            question,
        });
    }
    if loaded.is_empty() {
        return Ok(());
    }
    let questions: Vec<ConflictQuestion> = loaded.iter().map(|l| l.question.clone()).collect();
    let verdicts = ask(ctx, &prompts::conflict_messages(&questions)).await?;

    for (i, l) in loaded.iter().enumerate() {
        let v = verdict_for(&verdicts, i);
        let action = v
            .map(|v| v.action.as_str())
            .filter(|a| matches!(*a, "close_old" | "retime_new" | "keep_both" | "reject_new"))
            .unwrap_or("unsure");
        let conf = v.and_then(|v| v.confidence).unwrap_or(0.0).clamp(0.0, 1.0);
        let why = v.and_then(|v| v.why.clone());
        let params = json!({ "date": v.and_then(|v| v.date.clone()) });
        let done = if action != "unsure" && conf >= AUTO_CONF {
            Some(
                perform_conflict(
                    ctx.state,
                    ctx.kb_id,
                    &l.row,
                    &l.new,
                    &l.new_evidence,
                    action,
                    &params,
                )
                .await?,
            )
        } else {
            None
        };
        let text = format!(
            "{} ({}) · old {} · new {}",
            l.old.predicate.clone().unwrap_or_default(),
            l.row.reason,
            summary(&l.old),
            summary(&l.new)
        );
        settle_new(
            ctx, "conflict", l.row.id, &text, action, conf, why, params, done, "conflict",
        )
        .await?;
    }
    Ok(())
}

/// 冲突的一个出路，走人裁决时的那条路
async fn perform_conflict(
    state: &AppState,
    kb_id: Uuid,
    row: &queues::ConflictRow,
    new: &FactRow,
    new_evidence: &[Evidence],
    action: &str,
    params: &Value,
) -> anyhow::Result<Done> {
    let pool = &state.pool;
    let said = params["date"].as_str();
    match action {
        "close_old" => {
            let at = match said {
                Some(_) => match date_in_evidence(said, new, new_evidence) {
                    Some(at) => Some(at),
                    None => {
                        return Ok(Done::Held(
                            "the date is not in the new fact's evidence".into(),
                        ))
                    }
                },
                None => None,
            };
            if at.is_none() && new.valid_from.is_none() {
                return Ok(Done::Held("the new value has no start to close at".into()));
            }
            let corrected = utopia_store::temporal::resolve_conflict(
                pool,
                kb_id,
                row.id,
                "close",
                at.map(|(t, _)| t),
                at.map(|(_, p)| p).unwrap_or("day"),
            )
            .await?;
            Ok(Done::Applied(
                json!({ "corrected": corrected, "original": row.old_fact_id }),
            ))
        }
        "retime_new" => {
            let Some((from, precision)) = date_in_evidence(said, new, new_evidence) else {
                return Ok(Done::Held(
                    "the date is not in the new fact's evidence".into(),
                ));
            };
            let validity = utopia_store::graph::Validity {
                from: Some(from),
                from_precision: Some(precision),
                to: new.valid_to,
                to_precision: new.valid_to_precision.as_deref().map(|p| match p {
                    "year" => "year",
                    "month" => "month",
                    "unknown" => "unknown",
                    _ => "day",
                }),
                attested_at: None,
            };
            let Some(corrected) =
                utopia_store::temporal::correct_interval(pool, new.id, validity).await?
            else {
                return Ok(Done::Held(
                    "the new fact changed before it could be retimed".into(),
                ));
            };
            utopia_store::temporal::reconcile_moved_facts(pool, kb_id, &[corrected]).await?;
            Ok(Done::Applied(
                json!({ "corrected": corrected, "original": new.id }),
            ))
        }
        "keep_both" => {
            utopia_store::temporal::resolve_conflict(pool, kb_id, row.id, "keep", None, "day")
                .await?;
            Ok(Done::Applied(json!({})))
        }
        "reject_new" => {
            utopia_store::temporal::resolve_conflict(
                pool,
                kb_id,
                row.id,
                "reject_new",
                None,
                "day",
            )
            .await?;
            Ok(Done::Applied(json!({ "retracted": row.new_fact_id })))
        }
        other => Ok(Done::Held(format!("no action {other} for a conflict"))),
    }
}

/// 记一笔 agent 的决定，动了手的记台账
#[allow(clippy::too_many_arguments)]
async fn settle_new(
    ctx: &Ctx<'_>,
    kind: &str,
    target_id: Uuid,
    summary: &str,
    action: &str,
    conf: f32,
    why: Option<String>,
    params: Value,
    done: Option<Done>,
    audit_kind: &str,
) -> anyhow::Result<()> {
    let pool = &ctx.state.pool;
    let (status, undo, held) = match done {
        Some(Done::Applied(undo)) => ("applied", undo, None),
        Some(Done::Held(reason)) => ("proposed", json!({}), Some(reason)),
        None => ("proposed", json!({}), None),
    };
    let reason = match (&why, &held) {
        (Some(w), Some(h)) => Some(format!("held for a person: {h}; {w}")),
        (None, Some(h)) => Some(format!("held for a person: {h}")),
        (w, None) => w.clone(),
    };
    let id = gov::record_for(
        pool,
        ctx.kb_id,
        NewDecision {
            run_id: ctx.run_id,
            target_id,
            action,
            confidence: conf,
            reason: reason.as_deref(),
            precedents: json!([]),
            status,
            merge_id: None,
            question: None,
            trace: json!([]),
            calls: 0,
        },
        Target {
            kind,
            summary: Some(summary),
            detail: json!({ "params": params, "undo": undo }),
        },
    )
    .await?;
    if status == "applied" {
        let _ = utopia_store::audit::record_opt(
            pool,
            Some(ctx.kb_id),
            None,
            &audit_action(kind, action),
            audit_kind,
            Some(target_id),
            json!({ "summary": summary, "confidence": conf, "via": "governor", "decision": id,
                    "why": why }),
        )
        .await;
    }
    Ok(())
}

fn audit_action(kind: &str, action: &str) -> String {
    match (kind, action) {
        ("fact", a) => format!("fact.{a}"),
        ("conflict", "retime_new") => "fact.time_corrected".into(),
        ("conflict", a) => format!("conflict.{a}"),
        (k, a) => format!("{k}.{a}"),
    }
}

/// 人回答事实或冲突上的一条 agent 决定：接受或改判一条建议、撤回一次动手。
/// 回 `Ok(())` 之后调用方刷新审核台
pub(crate) async fn answer(
    state: &AppState,
    kb_id: Uuid,
    d: &AgentDecisionView,
    action: &str,
    user_id: Uuid,
    rationale: Option<&str>,
) -> utopia_core::AppResult<()> {
    let pool = &state.pool;
    let invalid = |msg: String| utopia_core::AppError::invalid("agent_answer", msg);
    match (d.status.as_str(), action) {
        ("applied", "revert") => {
            undo(state, kb_id, d).await?;
            gov::settle(pool, kb_id, d.id, "reverted", user_id).await?;
            let _ = utopia_store::audit::record(
                pool,
                Some(kb_id),
                user_id,
                "agent.revert",
                &d.target_kind,
                Some(d.target_id),
                json!({ "agent_decision": d.id, "agent_action": d.action,
                        "summary": d.summary, "why": rationale }),
            )
            .await;
            crate::governance::fuse(state, kb_id).await;
            Ok(())
        }
        ("proposed", act) => {
            let allowed: &[&str] = match d.target_kind.as_str() {
                "fact" => &["confirm", "reject"],
                "conflict" => &["close_old", "retime_new", "keep_both", "reject_new"],
                other => return Err(invalid(format!("no answers for {other} decisions here"))),
            };
            if !allowed.contains(&act) {
                return Err(invalid(format!(
                    "{act} is not an answer to a {} decision",
                    d.target_kind
                )));
            }
            let params = d.detail.get("params").cloned().unwrap_or(json!({}));
            let done = match d.target_kind.as_str() {
                "fact" => {
                    let f = queues::fact(pool, kb_id, d.target_id)
                        .await?
                        .ok_or(utopia_core::AppError::NotFound)?;
                    let current = if f.stale {
                        queues::current_text(pool, f.id, &f.subject).await?
                    } else {
                        Vec::new()
                    };
                    perform_fact(state, kb_id, &f, act, &params, &current)
                        .await
                        .map_err(|e| invalid(e.to_string()))?
                }
                _ => {
                    let row = queues::conflict_queue(pool, kb_id, i64::MAX)
                        .await?
                        .into_iter()
                        .find(|c| c.id == d.target_id);
                    let row = match row {
                        Some(r) => r,
                        None => sqlx::query_as(
                            "SELECT id, reason, old_fact_id, new_fact_id FROM fact_conflicts
                              WHERE id = $1 AND kb_id = $2 AND status = 'open'",
                        )
                        .bind(d.target_id)
                        .bind(kb_id)
                        .fetch_optional(pool)
                        .await?
                        .ok_or(utopia_core::AppError::NotFound)?,
                    };
                    let new = queues::fact(pool, kb_id, row.new_fact_id)
                        .await?
                        .ok_or(utopia_core::AppError::NotFound)?;
                    let evidence = queues::evidence(pool, new.id).await?;
                    perform_conflict(state, kb_id, &row, &new, &evidence, act, &params)
                        .await
                        .map_err(|e| invalid(e.to_string()))?
                }
            };
            if let Done::Held(why) = done {
                return Err(invalid(why));
            }
            let status = if act == d.action {
                "accepted"
            } else {
                "overridden"
            };
            gov::settle(pool, kb_id, d.id, status, user_id).await?;
            let _ = utopia_store::audit::record(
                pool,
                Some(kb_id),
                user_id,
                &audit_action(&d.target_kind, act),
                &d.target_kind,
                Some(d.target_id),
                json!({ "summary": d.summary, "agent_decision": d.id, "agent_action": d.action,
                        "why": rationale }),
            )
            .await;
            Ok(())
        }
        (status, act) => Err(invalid(format!(
            "cannot {act} an agent decision that is {status}"
        ))),
    }
}

/// 撤回 agent 动过的一次手，按 detail 里记下的东西
async fn undo(state: &AppState, kb_id: Uuid, d: &AgentDecisionView) -> utopia_core::AppResult<()> {
    let pool = &state.pool;
    let u = d.detail.get("undo").cloned().unwrap_or(json!({}));
    let uuid = |k: &str| {
        u.get(k)
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<Uuid>().ok())
    };
    match (d.target_kind.as_str(), d.action.as_str()) {
        ("fact", "confirm") => {
            if let Some(prior) = u.get("prior_confidence").and_then(|v| v.as_f64()) {
                utopia_store::temporal::set_confidence(pool, kb_id, d.target_id, prior as f32)
                    .await?;
            }
            if let Some(ev) = u.get("evidence") {
                let fact = ev
                    .get("fact_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok());
                let chunk = ev
                    .get("chunk_id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<Uuid>().ok());
                if let (Some(fact), Some(chunk)) = (fact, chunk) {
                    sqlx::query("DELETE FROM fact_evidence WHERE fact_id = $1 AND chunk_id = $2")
                        .bind(fact)
                        .bind(chunk)
                        .execute(pool)
                        .await?;
                    utopia_store::temporal::reconcile_moved_facts(pool, kb_id, &[fact]).await?;
                }
            }
        }
        ("fact", "reject") => {
            utopia_store::temporal::restore(pool, kb_id, d.target_id).await?;
        }
        ("conflict", "close_old" | "retime_new") => {
            if let (Some(corrected), Some(original)) = (uuid("corrected"), uuid("original")) {
                if utopia_store::temporal::undo_rewrite(pool, kb_id, corrected, original).await? {
                    // 原行回来了，它与邻居之间那道题也回来：重新「来到」时间线上，冲突照记
                    utopia_store::temporal::reconcile_moved_facts(pool, kb_id, &[original]).await?;
                }
            }
            utopia_store::temporal::reopen_conflict(pool, kb_id, d.target_id).await?;
        }
        ("conflict", "keep_both") => {
            utopia_store::temporal::reopen_conflict(pool, kb_id, d.target_id).await?;
        }
        ("conflict", "reject_new") => {
            if let Some(fact) = uuid("retracted") {
                utopia_store::temporal::restore(pool, kb_id, fact).await?;
            }
            utopia_store::temporal::reopen_conflict(pool, kb_id, d.target_id).await?;
        }
        (kind, action) => {
            return Err(utopia_core::AppError::invalid(
                "agent_answer",
                format!("cannot revert {kind} {action}"),
            ))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        format!("{s}T00:00:00Z").parse().unwrap()
    }

    fn row(from: Option<&str>) -> FactRow {
        FactRow {
            id: Uuid::nil(),
            subject: "HQ Lease".into(),
            predicate: Some("landlord".into()),
            object: Some("BBHQ1".into()),
            valid_from: from.map(at),
            valid_from_precision: from.map(|_| "day".into()),
            valid_to: None,
            valid_to_precision: None,
            confidence: 0.9,
            stale: false,
        }
    }

    fn ev(dated: Option<&str>, quote: &str) -> Evidence {
        Evidence {
            chunk_id: Uuid::nil(),
            document: "eleventh-amendment.html".into(),
            dated: dated.map(at),
            quote: Some(quote.into()),
        }
    }

    /// 模型给的日期要在证据里：文档的日期、事实的起点，或引文写着的那一天
    #[test]
    fn a_date_is_taken_only_when_the_evidence_shows_it() {
        let evidence = [ev(
            Some("2020-08-13"),
            "BBHQ1, LLC succeeded HPBB1 as landlord on July 31, 2020",
        )];
        let f = row(Some("2016-05-16"));
        assert!(
            date_in_evidence(Some("2020-08-13"), &f, &evidence).is_some(),
            "文档的日期"
        );
        assert!(
            date_in_evidence(Some("2016-05-16"), &f, &evidence).is_some(),
            "事实的起点"
        );
        assert!(
            date_in_evidence(Some("2020-07-31"), &f, &evidence).is_some(),
            "引文里写着"
        );
        assert!(
            date_in_evidence(Some("July 31, 2020"), &f, &evidence).is_some(),
            "写法不同也认"
        );
        assert!(
            date_in_evidence(Some("2020-09-01"), &f, &evidence).is_none(),
            "证据里没有"
        );
        assert!(date_in_evidence(None, &f, &evidence).is_none());
        assert!(date_in_evidence(Some("soon"), &f, &evidence).is_none());
    }

    #[test]
    fn an_action_lands_in_the_ledger_under_the_queue_it_came_from() {
        assert_eq!(audit_action("fact", "confirm"), "fact.confirm");
        assert_eq!(audit_action("conflict", "close_old"), "conflict.close_old");
        assert_eq!(
            audit_action("conflict", "retime_new"),
            "fact.time_corrected"
        );
    }
}

#[cfg(test)]
#[path = "queue_agent_tests.rs"]
mod queue_agent_tests;
