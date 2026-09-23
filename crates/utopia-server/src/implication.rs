//! 蕴含规则在服务端的两段活（0044 决定 3 第五片）：对齐结束时向模型提规则；
//! `read_phrases` 任务把已批准规则要的读数算进缓存，然后排物化。
//!
//! 物化本身不调模型（`utopia_store::materialize`），缓存没填上的读数那一轮就不算。

use std::collections::HashMap;
use utopia_core::models::RelationTypeView;
use utopia_extract::implication::{
    build_reading_messages, build_rule_messages, parse_reading_response, parse_rule_response,
    ReadingItem, RuleItem,
};
use utopia_extract::phrase_align::PropertyCandidate;
use utopia_store::implication_rules::{self, Proposal, READINGS};
use utopia_store::phrase_bindings::{self, PhraseSignature};
use utopia_store::type_bindings::KindWordSignature;
use uuid::Uuid;

use crate::extraction::chat_retrying_rate_limits_at;
use crate::llm_util;
use crate::state::AppState;

const BATCH: usize = 12;

/// 一次提规则的输入：签名（带它绑到的属性与候选）和类别词（带候选）。
pub struct RuleAsk<'a> {
    pub phrase: Option<&'a PhraseSignature>,
    pub kind_word: Option<&'a KindWordSignature>,
    pub bound_to: Option<&'a str>,
    pub candidates: Vec<&'a RelationTypeView>,
    pub basis: &'a str,
}

/// 向模型提规则，把答案落成提案（要人批）或代理的驳回（什么也不蕴含，记下免得再问）。
/// 返回 (提案数, 失败批次)。候选为空的形状不问
pub async fn propose_rules(
    state: &AppState,
    kb_id: Uuid,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
    asks: &[RuleAsk<'_>],
    class_key: &HashMap<Uuid, &str>,
    by_key: &HashMap<&str, &RelationTypeView>,
) -> anyhow::Result<(usize, usize)> {
    let pool = &state.pool;
    let keys_of = |ids: &[Uuid]| -> Vec<&str> {
        ids.iter()
            .filter_map(|id| class_key.get(id).copied())
            .collect()
    };
    let (mut proposed, mut failed) = (0usize, 0usize);
    let asks: Vec<&RuleAsk<'_>> = asks.iter().filter(|a| !a.candidates.is_empty()).collect();
    for batch in asks.chunks(BATCH) {
        let items: Vec<RuleItem<'_>> = batch
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let candidates = a
                    .candidates
                    .iter()
                    .map(|p| PropertyCandidate {
                        key: &p.key,
                        label: &p.label,
                        description: &p.description,
                        kind: &p.kind,
                        domains: keys_of(&p.domains),
                        ranges: keys_of(&p.ranges),
                        via: Vec::new(),
                    })
                    .collect();
                match (a.phrase, a.kind_word) {
                    (Some(s), _) => RuleItem {
                        id: i as i64,
                        trigger: "phrase",
                        phrase: &s.phrase,
                        subject_class: s.subject_type_key.as_deref(),
                        object_class: s.object_type_key.as_deref(),
                        object_is_value: s.object_is_value,
                        bound_to: a.bound_to,
                        examples: &s.examples,
                        candidates,
                    },
                    (None, Some(k)) => RuleItem {
                        id: i as i64,
                        trigger: "kind_word",
                        phrase: &k.kind_word,
                        subject_class: None,
                        object_class: None,
                        object_is_value: false,
                        bound_to: None,
                        examples: &k.examples,
                        candidates,
                    },
                    (None, None) => unreachable!("an ask is a phrase or a kind word"),
                }
            })
            .collect();
        let messages = build_rule_messages(&items, READINGS);
        let reply =
            match chat_retrying_rate_limits_at(state, settings, client, &messages, Some(0.0)).await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "提规则调用失败，这一批留到下次");
                    failed += 1;
                    continue;
                }
            };
        let (choices, malformed) = match parse_rule_response(&reply.text, &items, READINGS) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(%kb_id, error = %e, "提规则回复解析失败，这一批留到下次");
                failed += 1;
                continue;
            }
        };
        if malformed > 0 {
            tracing::info!(%kb_id, malformed, "提规则的回复里有坏项");
        }
        for c in choices {
            let Ok(i) = usize::try_from(c.id) else {
                continue;
            };
            let Some(a) = batch.get(i) else { continue };
            let (trigger, phrase, subject, object, value, count, examples) =
                match (a.phrase, a.kind_word) {
                    (Some(s), _) => (
                        "phrase",
                        s.phrase.as_str(),
                        s.subject_type_id,
                        s.object_type_id,
                        s.object_is_value,
                        s.count,
                        s.examples.as_slice(),
                    ),
                    (None, Some(k)) => (
                        "kind_word",
                        k.kind_word.as_str(),
                        None,
                        None,
                        false,
                        k.count,
                        k.examples.as_slice(),
                    ),
                    (None, None) => continue,
                };
            match c.implies {
                Some((key, reading)) => {
                    let Some(p) = by_key.get(key.as_str()) else {
                        continue;
                    };
                    let votes =
                        serde_json::json!({ "agent": { "property": key, "reading": reading } });
                    if implication_rules::propose(
                        pool,
                        kb_id,
                        &Proposal {
                            trigger,
                            phrase,
                            subject_type_id: subject,
                            object_type_id: object,
                            object_is_value: value,
                            conclude_property_id: p.id,
                            reading: reading.as_deref(),
                            status: "proposed",
                            votes: &votes,
                            basis: a.basis,
                            statement_count: count,
                            examples,
                        },
                    )
                    .await?
                    .is_some()
                    {
                        proposed += 1;
                    }
                }
                None => {
                    // 「什么也不蕴含」也要落下来，不然每轮都问。落成代理驳回的一行：
                    // 属性列非空不可，这里记的是形状本身，用签名绑到的属性或第一个候选占位
                    let placeholder = a
                        .bound_to
                        .and_then(|k| by_key.get(k))
                        .or_else(|| a.candidates.first())
                        .map(|p| p.id);
                    let Some(property) = placeholder else {
                        continue;
                    };
                    let votes = serde_json::json!({ "agent": null, "reason": "nothing_implied" });
                    implication_rules::propose(
                        pool,
                        kb_id,
                        &Proposal {
                            trigger,
                            phrase,
                            subject_type_id: subject,
                            object_type_id: object,
                            object_is_value: value,
                            conclude_property_id: property,
                            reading: None,
                            status: "rejected",
                            votes: &votes,
                            basis: a.basis,
                            statement_count: count,
                            examples,
                        },
                    )
                    .await?;
                }
            }
        }
    }
    Ok((proposed, failed))
}

/// `read_phrases` 任务：已批准规则要的、缓存里还没有的读数，问一遍模型，落进缓存；
/// 名字解析成库里的实体（没有就建一个有名字的）；读不出来的也记，别再问。
/// 填完排一次物化——隐含行在那里算
pub async fn read_phrases(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let pool = &state.pool;
    let pending = implication_rules::pending_readings(pool, kb_id).await?;
    if pending.is_empty() {
        tracing::info!(%kb_id, "没有待读的字");
    } else {
        let kb = utopia_store::kbs::get(pool, kb_id).await?;
        let settings = utopia_store::settings::get(pool, kb.workspace_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot read phrases"))?;
        let client = llm_util::chat_client(&settings)
            .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot read phrases"))?;
        let (mut answered, mut failed) = (0usize, 0usize);
        for batch in pending.chunks(BATCH * 2) {
            let items: Vec<ReadingItem<'_>> = batch
                .iter()
                .enumerate()
                .map(|(i, p)| ReadingItem {
                    id: i as i64,
                    reading: &p.reading,
                    phrase: &p.phrase,
                })
                .collect();
            let messages = build_reading_messages(&items, READINGS);
            let reply =
                match chat_retrying_rate_limits_at(state, &settings, &client, &messages, Some(0.0))
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(%kb_id, error = %e, "读数调用失败，这一批留到下次");
                        failed += 1;
                        continue;
                    }
                };
            let (answers, malformed) = match parse_reading_response(&reply.text, &items) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "读数回复解析失败，这一批留到下次");
                    failed += 1;
                    continue;
                }
            };
            if malformed > 0 {
                tracing::info!(%kb_id, malformed, "读数的回复里有坏项");
            }
            for a in answers {
                let Ok(i) = usize::try_from(a.id) else {
                    continue;
                };
                let Some(p) = batch.get(i) else { continue };
                let entity = match &a.name {
                    Some(name) => {
                        Some(implication_rules::resolve_or_create_named(pool, kb_id, name).await?)
                    }
                    None => None,
                };
                let value = a.value.as_ref().map(|v| serde_json::json!({ "value": v }));
                implication_rules::record_reading(
                    pool,
                    kb_id,
                    &p.reading,
                    &p.phrase,
                    entity,
                    value.as_ref(),
                )
                .await?;
                answered += 1;
            }
        }
        tracing::info!(%kb_id, pending = pending.len(), answered, failed, "读数填缓存完成");
        if failed > 0 {
            anyhow::bail!("{failed} reading batches failed; the job retries");
        }
    }
    utopia_store::jobs::enqueue_unless_queued(
        pool,
        phrase_bindings::MATERIALIZE_KIND,
        serde_json::json!({ "kb_id": kb_id }),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
#[path = "implication_tests.rs"]
mod tests;
