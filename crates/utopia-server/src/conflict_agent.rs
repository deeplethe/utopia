//! 0043's conflict cut: evidence-backed batches after duplicate governance.
//! The store owns freshness checks, execution gates, atomic recording and undo.

use crate::{llm_util, state::AppState};
use serde_json::Value;
use utopia_core::models::LlmSettings;
use utopia_extract::conflict_agent::{self as prompt, Question};
use utopia_llm::LlmClient;
use utopia_store::conflict_governance::{self as store, Case, Evidence, Fact, Parameters};
use uuid::Uuid;

const BATCH: i64 = 8;
const ROUNDS: usize = 20;
/// Count reviewed items, not rows remaining in the queue. An unsure/malformed
/// item spends budget too, and does not get asked again on the next run.
const DAILY_ITEMS: i64 = 2000;

pub async fn run(
    state: &AppState,
    kb_id: Uuid,
    client: &LlmClient,
    settings: &Option<LlmSettings>,
) -> anyhow::Result<bool> {
    let pool = &state.pool;
    store::expire(pool, kb_id).await?;
    let run_id = Uuid::now_v7();
    for _ in 0..ROUNDS {
        if !utopia_store::kbs::get(pool, kb_id).await?.governance {
            return Ok(false);
        }
        let left = DAILY_ITEMS - store::reviewed_today(pool, kb_id).await?;
        if left <= 0 {
            return Ok(false);
        }
        let ids = store::queue(pool, kb_id, BATCH.min(left)).await?;
        if ids.is_empty() {
            return Ok(false);
        }
        let precedents = store::precedents(pool, kb_id).await?;
        let mut items = Vec::new();
        let mut questions = Vec::new();
        for id in ids {
            let Some(item) = store::load(pool, kb_id, id).await? else {
                continue;
            };
            questions.push(question(&item, &precedents));
            items.push(item);
        }
        if items.is_empty() {
            return Ok(false);
        }
        let messages = prompt::messages(&questions);
        let reply = {
            let _permit = match settings {
                Some(s) => llm_util::acquire_chat(state, s).await,
                None => None,
            };
            client.chat(&messages).await?
        };
        let verdicts = prompt::parse_verdicts(&reply, items.len());
        let mut recorded = 0;
        for (item, verdict) in items.iter().zip(verdicts) {
            let params = verdict
                .date
                .as_deref()
                .and_then(utopia_extract::parse_time)
                .map(|(date, precision)| Parameters {
                    date: Some(date),
                    precision: Some(precision.into()),
                })
                .unwrap_or_default();
            if store::record(
                pool,
                kb_id,
                item,
                store::Decision {
                    run_id,
                    action: &verdict.action,
                    confidence: verdict.confidence,
                    why: &verdict.why,
                    params,
                    precedents: &precedents,
                },
            )
            .await?
            .is_some()
            {
                recorded += 1
            }
        }
        state.emit_review(kb_id);
        // All items changed during the call. Give concurrent ingestion a chance
        // to settle instead of spending another whole batch on a moving snapshot.
        if recorded == 0 {
            return Ok(false);
        }
    }
    Ok(!store::queue(pool, kb_id, 1).await?.is_empty())
}

fn date_text(
    date: Option<chrono::DateTime<chrono::Utc>>,
    precision: Option<&str>,
    end: bool,
) -> String {
    match (date, precision) {
        (Some(d), Some("year")) => d.format("%Y").to_string(),
        (Some(d), Some("month")) => d.format("%Y-%m").to_string(),
        (Some(d), _) => d.to_rfc3339(),
        (None, Some("unknown")) => "ended, date unknown".into(),
        (None, _) if end => "open".into(),
        _ => "unknown".into(),
    }
}

fn card(f: &Fact, evidence: &[Evidence]) -> String {
    let mut text = format!(
        "{} — {} → {} [{} → {}] (extraction confidence {})",
        f.subject,
        f.predicate,
        f.object,
        date_text(f.valid_from, f.valid_from_precision.as_deref(), false),
        date_text(f.valid_to, f.valid_to_precision.as_deref(), true),
        f.confidence
    );
    text.push_str(&format!(
        "\n  Property: {}\n  Qualifiers: {}",
        f.property, f.qualifiers
    ));
    if f.valid_from_grade.as_deref() == Some("C") {
        text.push_str("\n  The start's time reference has no resolved anchor.");
    }
    if evidence.is_empty() {
        text.push_str(
            "\n  (no current source passage available; this alone does not refute the fact)",
        )
    }
    for e in evidence {
        let quote = e.quote.as_deref().unwrap_or("(no quoted passage)");
        let mut excerpt: String = quote.chars().take(700).collect();
        if quote.chars().count() > 700 {
            excerpt.push_str(" … (excerpt truncated)")
        }
        text.push_str(&format!(
            "\n  [{}{}{}] {}",
            e.document,
            e.dated
                .map(|d| format!(", document dated {}", d.format("%Y-%m-%d")))
                .unwrap_or_default(),
            if e.described {
                ", visual description"
            } else {
                ""
            },
            excerpt
        ));
    }
    text
}

fn question(item: &Case, precedents: &[Value]) -> Question {
    Question {
        reason: item.reason.clone(),
        old: card(&item.old, &item.old_evidence),
        new: card(&item.new, &item.new_evidence),
        neighbours: item.neighbours.iter().map(|f| card(f, &[])).collect(),
        precedents: precedents
            .iter()
            .map(|p| {
                let d = &p["detail"];
                let text = |key: &str| d[key].as_str().unwrap_or("");
                format!(
                    "{}: {} — {} → {} versus {} → {}; {}",
                    p["action"].as_str().unwrap_or(""),
                    text("old_subject"),
                    text("predicate"),
                    text("old_object"),
                    text("new_subject"),
                    text("new_object"),
                    text("why")
                )
            })
            .collect(),
    }
}
