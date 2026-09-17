//! 关系短语按签名绑到属性（0044 决定 3 的第二片；账本侧见 `utopia_store::phrase_bindings`，
//! 合同见 `utopia_extract::phrase_align`）。
//!
//! 签名 = 短语 × 主语的类 × 宾语的类（宾语是字面值时记「值」）。两端的类来自类别词绑定
//! 写到实体上的 `type_id`，所以这一步排在类别词对齐之后。一个库里 distinct 的签名比陈述
//! 少得多，每条只判一次：**两票一致**才绑（第二票候选倒序，防「选第一个」冒充一致），
//! 不一致记 undecided 留给审核（#725 对齐队列），没有属性对得上记 none——陈述留在开放
//! 图谱，什么都不丢，签名计入工作台的建议。绑定按属性的 `updated_at` 与库里最新的属性
//! 判过期，本体一改只重判过期的。这一片只记判定；按绑定把陈述算成类型化事实是下一片。
//!
//! 候选属性怎么来：先按声明的定义域/值域筛——签名两端的类落在属性的域/值域里的，或者
//! 属性没声明域/值域的（两个方向都算，绑定可以是反向的）。**一端没绑到类的签名，只有
//! 没声明那一端的属性才算候选**：类别词还没绑上时把声明了域的属性也给模型，NVDA 四篇上
//! 现金流量表的每一行都绑到了泛泛的 value（主语 NVIDIA 没类，value 的域是指标）——裁判
//! 判成写错的一半是它。筛完仍超过上限就不判，瞎判比不判糟。

use crate::extraction::chat_retrying_rate_limits_at;
use crate::llm_util;
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
use utopia_core::models::RelationTypeView;
use utopia_extract::phrase_align::{
    build_phrase_messages, parse_phrase_response, Direction, PhraseItem, PropertyCandidate,
};
use utopia_store::phrase_bindings::{self, Decision, PhraseSignature};
use uuid::Uuid;

/// 一次问多少条签名。
const BATCH: usize = 12;
/// 筛过定义域/值域之后候选属性最多这么多，再多就不判。
const CANDIDATE_LIMIT: usize = 60;

/// 一票：这条签名选了哪个属性、哪个方向（None = 没有属性对得上）。
type Vote = Option<(String, Direction)>;

/// 签名两端的类落在属性声明的域/值域里（没声明的不限，没绑到类的一端只被没声明的
/// 一端接受）；正反两个方向都算。
fn fits(p: &RelationTypeView, sig: &PhraseSignature) -> bool {
    let within = |declared: &[Uuid], class: Option<Uuid>| -> bool {
        declared.is_empty() || class.is_some_and(|c| declared.contains(&c))
    };
    if sig.object_is_value {
        p.kind == "attribute" && within(&p.domains, sig.subject_type_id)
    } else {
        p.kind == "relation"
            && ((within(&p.domains, sig.subject_type_id) && within(&p.ranges, sig.object_type_id))
                || (within(&p.domains, sig.object_type_id)
                    && within(&p.ranges, sig.subject_type_id)))
    }
}

/// 对一个库跑一遍：新出现的和过期的签名各判一次。
pub async fn align_phrases(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let pool = &state.pool;
    let kb = utopia_store::kbs::get(pool, kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align phrases"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align phrases"))?;
    // 一个库同时只跑一份，理由同类别词对齐（并行跑会把端点打出 502）
    let mut guard = pool.acquire().await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('align_phrases'), hashtext($1))")
            .bind(kb_id.to_string())
            .fetch_one(&mut *guard)
            .await?;
    if !locked {
        // 正在跑的那份结束时会自己看一眼有没有新东西（见 align_phrases_locked 末尾）；这里
        // 不排——排回去会和跑着的那份互相踢成死循环
        tracing::info!(%kb_id, "短语对齐已有一份在跑，这次跳过");
        return Ok(());
    }
    let result = align_phrases_locked(state, kb_id, &settings, &client).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('align_phrases'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *guard)
        .await;
    result
}

async fn align_phrases_locked(
    state: &AppState,
    kb_id: Uuid,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let run_started = chrono::Utc::now();
    let props = utopia_store::ontology::relation_type_views(pool, kb_id).await?;
    let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
    let class_key: HashMap<Uuid, &str> = classes.iter().map(|c| (c.id, c.key.as_str())).collect();
    let by_key: HashMap<&str, &RelationTypeView> =
        props.iter().map(|p| (p.key.as_str(), p)).collect();
    let sigs = phrase_bindings::signatures(pool, kb_id).await?;
    let existing: HashMap<_, _> = phrase_bindings::bindings(pool, kb_id)
        .await?
        .into_iter()
        .map(|b| (b.key(), b))
        .collect();
    let stale: HashSet<_> = phrase_bindings::stale(pool, kb_id)
        .await?
        .into_iter()
        .map(|b| b.key())
        .collect();
    let todo: Vec<&PhraseSignature> = sigs
        .iter()
        .filter(|s| match existing.get(&s.key()) {
            None => true,
            Some(b) => b.decided_by != "person" && stale.contains(&s.key()),
        })
        .collect();
    let attempted: HashSet<_> = todo.iter().map(|s| s.key()).collect();
    tracing::info!(%kb_id, signatures = sigs.len(), to_decide = todo.len(), properties = props.len(), "短语对齐开始");

    // 没有属性可绑：每条都是「没有」；属性出现后 `stale` 会把它们再交回来
    if props.is_empty() {
        for s in &todo {
            phrase_bindings::decide(
                pool,
                kb_id,
                s,
                Decision {
                    relation_type_id: None,
                    direction: None,
                    status: "none",
                    votes: &serde_json::json!({ "reason": "no properties" }),
                    decided_by: "agent",
                },
            )
            .await?;
        }
        return Ok(());
    }

    let keys_of = |ids: &[Uuid]| -> Vec<&str> {
        ids.iter()
            .filter_map(|id| class_key.get(id).copied())
            .collect()
    };
    let (mut bound, mut none, mut undecided, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    // 调用或解析失败的批次：这轮跳过，结束时自己再排一次
    let mut failed = 0usize;
    for batch in todo.chunks(BATCH) {
        let cands: Vec<Vec<&RelationTypeView>> = batch
            .iter()
            .map(|s| {
                let fitting: Vec<&RelationTypeView> = props.iter().filter(|p| fits(p, s)).collect();
                if fitting.len() > CANDIDATE_LIMIT {
                    Vec::new()
                } else {
                    fitting
                }
            })
            .collect();
        let mut votes: Vec<(Vote, Vote)> = vec![(None, None); batch.len()];
        let mut answered = vec![(false, false); batch.len()];
        for pass in 0..2 {
            let items: Vec<PhraseItem<'_>> = batch
                .iter()
                .enumerate()
                .filter(|(i, _)| !cands[*i].is_empty())
                .map(|(i, s)| {
                    let mut list: Vec<&RelationTypeView> = cands[i].clone();
                    if pass == 1 {
                        list.reverse();
                    }
                    PhraseItem {
                        id: i as i64,
                        phrase: &s.phrase,
                        subject_class: s.subject_type_key.as_deref(),
                        object_class: s.object_type_key.as_deref(),
                        object_is_value: s.object_is_value,
                        statement_count: s.count,
                        examples: &s.examples,
                        quotes: &s.quotes,
                        candidates: list
                            .iter()
                            .map(|p| PropertyCandidate {
                                key: &p.key,
                                label: &p.label,
                                description: &p.description,
                                kind: &p.kind,
                                domains: keys_of(&p.domains),
                                ranges: keys_of(&p.ranges),
                            })
                            .collect(),
                    }
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            let messages = build_phrase_messages(&items);
            let reply =
                match chat_retrying_rate_limits_at(state, settings, client, &messages, Some(0.0))
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(%kb_id, error = %e, "短语对齐调用失败，这一批留到下次");
                        failed += 1;
                        continue;
                    }
                };
            let (choices, malformed) = match parse_phrase_response(&reply, &items) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "短语对齐回复解析失败，这一批留到下次");
                    failed += 1;
                    continue;
                }
            };
            skipped += malformed;
            for c in choices {
                let Ok(i) = usize::try_from(c.id) else {
                    continue;
                };
                if let Some(slot) = votes.get_mut(i) {
                    if pass == 0 {
                        slot.0 = c.property;
                        answered[i].0 = true;
                    } else {
                        slot.1 = c.property;
                        answered[i].1 = true;
                    }
                }
            }
        }
        for (i, s) in batch.iter().enumerate() {
            if cands[i].is_empty() {
                skipped += 1;
                continue;
            }
            let (a, b) = &votes[i];
            let (ans_a, ans_b) = answered[i];
            if !ans_a || !ans_b {
                // 有一票没答到：不下结论，下次再问
                continue;
            }
            let show = |v: &Vote| {
                v.as_ref()
                    .map(|(k, d)| serde_json::json!({ "property": k, "direction": d.as_str() }))
                    .unwrap_or(serde_json::Value::Null)
            };
            let record = serde_json::json!({ "first": show(a), "second": show(b) });
            if a != b {
                phrase_bindings::decide(
                    pool,
                    kb_id,
                    s,
                    Decision {
                        relation_type_id: None,
                        direction: None,
                        status: "undecided",
                        votes: &record,
                        decided_by: "agent",
                    },
                )
                .await?;
                undecided += 1;
                continue;
            }
            match a
                .as_ref()
                .and_then(|(k, d)| by_key.get(k.as_str()).map(|p| (p, *d)))
            {
                Some((p, d)) => {
                    if phrase_bindings::decide(
                        pool,
                        kb_id,
                        s,
                        Decision {
                            relation_type_id: Some(p.id),
                            direction: Some(d.as_str()),
                            status: "bound",
                            votes: &record,
                            decided_by: "agent",
                        },
                    )
                    .await?
                    {
                        bound += 1;
                    }
                }
                None => {
                    if phrase_bindings::decide(
                        pool,
                        kb_id,
                        s,
                        Decision {
                            relation_type_id: None,
                            direction: None,
                            status: "none",
                            votes: &record,
                            decided_by: "agent",
                        },
                    )
                    .await?
                    {
                        none += 1;
                    }
                }
            }
        }
    }
    tracing::info!(%kb_id, bound, none, undecided, skipped, failed, "短语对齐完成");
    // 这一轮跑着的时候世界没停：新文档带来新签名，改了的属性让刚判的绑定过期，本轮没排上
    // 的触发也都落在这里。有失败的批次、有没试过的新签名、有本轮判完又过期的绑定，就再排
    // 一次
    let again = failed > 0
        || phrase_bindings::signatures(pool, kb_id)
            .await?
            .iter()
            .any(|s| !attempted.contains(&s.key()) && !existing.contains_key(&s.key()))
        || phrase_bindings::stale(pool, kb_id)
            .await?
            .iter()
            .any(|b| b.decided_by != "person" && b.decided_at >= run_started);
    if again {
        utopia_store::jobs::enqueue_unless_queued(
            pool,
            "align_phrases",
            serde_json::json!({ "kb_id": kb_id }),
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(kind: &str, domains: Vec<Uuid>, ranges: Vec<Uuid>) -> RelationTypeView {
        RelationTypeView {
            id: Uuid::now_v7(),
            key: "p".into(),
            label: "p".into(),
            temporal: "state".into(),
            functional: false,
            inverse_functional: false,
            is_transitive: false,
            is_symmetric: false,
            is_asymmetric: false,
            is_irreflexive: false,
            inverse_of: None,
            sub_property_of: None,
            builtin: false,
            description: String::new(),
            kind: kind.into(),
            domains,
            ranges,
            datatype: None,
            unit: None,
            qualifiers: Vec::new(),
            usage: 0,
        }
    }

    fn sig(subject: Option<Uuid>, object: Option<Uuid>, value: bool) -> PhraseSignature {
        PhraseSignature {
            phrase: "x".into(),
            subject_type_id: subject,
            subject_type_key: None,
            object_type_id: object,
            object_type_key: None,
            object_is_value: value,
            count: 1,
            examples: Vec::new(),
            quotes: Vec::new(),
        }
    }

    /// 域/值域筛候选：声明了的要落在里面（正反都算），没声明的不限，类为空的一端不限
    #[test]
    fn a_property_fits_a_signature_by_its_declared_ends_in_either_direction() {
        let (org, place, person) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let hq = view("relation", vec![org], vec![place]);
        assert!(fits(&hq, &sig(Some(org), Some(place), false)));
        assert!(fits(&hq, &sig(Some(place), Some(org), false)), "反向也算");
        assert!(!fits(&hq, &sig(Some(person), Some(place), false)));
        assert!(
            !fits(&hq, &sig(None, Some(place), false)),
            "没绑到类的一端不算落在声明的域里"
        );
        let any_to_place = view("relation", vec![], vec![place]);
        assert!(
            fits(&any_to_place, &sig(None, Some(place), false)),
            "没声明的一端接受没绑到类的"
        );
        assert!(!fits(&hq, &sig(Some(org), None, true)), "关系不接字面值");
        let open = view("relation", vec![], vec![]);
        assert!(
            fits(&open, &sig(Some(person), Some(person), false)),
            "没声明就不限"
        );
        let revenue = view("attribute", vec![org], vec![]);
        assert!(fits(&revenue, &sig(Some(org), None, true)));
        assert!(!fits(&revenue, &sig(Some(person), None, true)));
        assert!(
            !fits(&revenue, &sig(Some(org), Some(place), false)),
            "属性只接字面值"
        );
    }
}
