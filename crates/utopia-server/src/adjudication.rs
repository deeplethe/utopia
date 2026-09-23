//! 实体消解攒批裁决任务：消费审核队列中 stage=adjudicating 的灰区对。
//! 先查裁决缓存，缓存未命中的攒成一批（一次 LLM 调用裁多对）；
//! 高置信 same → 自动合并（可回滚），高置信 different → 自动保持分开，
//! 其余转人工。未配模型时全部转人工——本任务失败或缺席都不影响抽取与查询。

use crate::governance::{look_again, Look};
use crate::llm_util;
use crate::state::AppState;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use utopia_core::models::ReviewItem;
use utopia_core::AppError;
use utopia_store::governance as gov;
use uuid::Uuid;

const BATCH_SIZE: i64 = 12;
const AUTO_CONF: f32 = 0.8;
const MAX_ROUNDS: usize = 20;
/// 缓存键：类型 + 双方名字 + 事实摘要 + 先例（与实体 id 无关——重传文档不重复付费）。
/// **先例也进键**：答案随先例变（0025 说的正是这个，治理那一路因此干脆不用缓存）；
/// 人又裁了一笔，键就变，旧答案自然作废，不必去清
fn pair_key(item: &ReviewItem, precedents: &[String]) -> String {
    let side = |s: &utopia_core::models::ReviewSide| {
        format!(
            "{}|{}|{}",
            s.name.to_lowercase(),
            // 没判出类型的一侧照样要能缓存（0009）
            s.type_label.as_deref().unwrap_or("untyped"),
            s.top_facts.join(";")
        )
    };
    let mut sides = [side(&item.left), side(&item.right)];
    sides.sort();
    let digest =
        Sha256::digest(format!("{}##{}", sides.join("##"), precedents.join("\n")).as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 攒批没定的对要不要带工具再看一遍（0028）：没判决或把握不到线；硬规则拦得住的不看
/// （大类不同、版本尾巴、含名字的一句话——再看也改不了规则）；有撤回的不看，那是人的事
fn wants_another_look(
    item: &ReviewItem,
    p: &gov::Precedents,
    same: Option<bool>,
    conf: f32,
) -> bool {
    let unsettled = same.is_none() || conf < AUTO_CONF;
    let ruled = gov::types_conflict(
        item.left.type_label.as_deref(),
        item.right.type_label.as_deref(),
    ) || matches!(
        gov::name_shape(&item.left.name, &item.right.name),
        gov::NameShape::Version | gov::NameShape::Phrase
    );
    unsettled && !ruled && p.reverts.is_empty()
}

/// 名字向量召回提的对（0041 第 2 刀）：两个**不同的字符串**因相近而被放到一起。
/// 「因相似而提议」和「因同名而提议」是两种证据强度，不该用同一根线自动合：批量裁决只看
/// 名字和几条事实，测量台上它把「张伟」并进了「财务部总监张伟」（0.85，过了线）。所以这类
/// 对的 same 一律走带工具的第二眼——它能读两边的事实与原文再定；different 与 unsure
/// 照旧，分开是安全的方向。不看 `ruled` 与撤回：那两条是「再看也改不了」的省事，这里要的
/// 恰恰是再看
fn similarity_proposed(item: &ReviewItem) -> bool {
    item.reason
        .as_deref()
        .is_some_and(|r| r.starts_with("name_vector|"))
}

fn needs_second_look(
    item: &ReviewItem,
    p: &gov::Precedents,
    same: Option<bool>,
    conf: f32,
) -> bool {
    wants_another_look(item, p, same, conf) || (similarity_proposed(item) && same == Some(true))
}

/// 一次裁决落地成了什么：第二层的行按它记 applied 还是 proposed
enum Outcome {
    Merged(Uuid),
    Kept,
    Escalated,
}

/// 第二层看过的一对，记一行 `agent_decisions`（0028）：轨迹、问题、花的调用都在里面。
/// 预算按这些行算，Agent 队列也从这里读——治理开没开，机器去看过的都看得见
async fn record_look(
    state: &AppState,
    kb_id: Uuid,
    run_id: Uuid,
    item: &ReviewItem,
    p: &gov::Precedents,
    look: &Look,
    outcome: &Outcome,
) -> anyhow::Result<()> {
    let action = match look.same {
        Some(true) => "merge",
        Some(false) => "keep",
        None => "unsure",
    };
    let (status, merge_id) = match outcome {
        Outcome::Merged(id) => ("applied", Some(*id)),
        Outcome::Kept => ("applied", None),
        Outcome::Escalated => ("proposed", None),
    };
    gov::record(
        &state.pool,
        kb_id,
        gov::NewDecision {
            run_id,
            target_id: item.id,
            action,
            confidence: look.conf,
            reason: look.why.as_deref(),
            precedents: gov::precedents_json(p),
            status,
            merge_id,
            question: look.question.as_deref(),
            trace: serde_json::Value::Array(look.trace.clone()),
            calls: look.calls,
        },
    )
    .await?;
    Ok(())
}

/// 同一个库的裁决一次只跑一个（与 `ontology_index::PER_KB` 同一套理由）。
///
/// 入队去重只挡得住「还排着队的」；一批文档陆续抽完时，后一个入队时前一个
/// 已经是 `running`，于是七个任务同时跑同一个库。它们各自 `pending_adjudications`
/// 拿到**同一批**待裁项：模型调用翻七倍，而两个任务对同一对下判断时，第二个
/// 撞 `agent_decisions_open_idx` 唯一索引直接失败（2026-09-08 实测：七个任务里
/// 两个这样挂掉）。
///
/// **等而不是跳过**：后到的等前一个跑完再进，进来时重新查一遍待裁项，
/// 通常只剩前一个读完之后才新增的那几对。
static PER_KB: LazyLock<Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(Default::default);

fn lock_for(kb_id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
    PER_KB
        .lock()
        .expect("per-kb adjudication lock table poisoned")
        .entry(kb_id)
        .or_default()
        .clone()
}

pub async fn adjudicate_entities(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let lock = lock_for(kb_id);
    let _serial = lock.lock().await;
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id).await?;
    let client = settings.as_ref().and_then(llm_util::chat_client);
    let model = settings
        .as_ref()
        .and_then(|s| s.chat_model.clone())
        .unwrap_or_default();

    let Some(client) = client else {
        // 无模型可用：全部转人工，任务本身成功结束
        let items =
            utopia_store::resolution::pending_adjudications(&state.pool, kb_id, 500).await?;
        for item in items {
            utopia_store::resolution::escalate_review(&state.pool, item.id, "escalate_no_model")
                .await?;
        }
        state.emit_review(kb_id);
        return Ok(());
    };
    // 第二层的行都挂在这一次任务上
    let run_id = Uuid::now_v7();

    for _ in 0..MAX_ROUNDS {
        let items =
            utopia_store::resolution::pending_adjudications(&state.pool, kb_id, BATCH_SIZE).await?;
        if items.is_empty() {
            break;
        }

        // 第一层：裁决缓存。先例（人在这个库里对这些名字做过什么，连同他们写的理由）
        // 先取出来：它既进提示词也进缓存键
        let mut to_ask: Vec<(ReviewItem, String, gov::Precedents, Vec<String>)> = Vec::new();
        for item in items {
            let p = gov::precedents_for(&state.pool, kb_id, &item).await?;
            let precedents = gov::render_lines(&p);
            let key = pair_key(&item, &precedents);
            match utopia_store::resolution::get_verdict(&state.pool, kb_id, &key).await? {
                Some((same, conf)) => {
                    apply_verdict(state, kb_id, &item, same, conf, "cached", None).await?;
                }
                None => to_ask.push((item, key, p, precedents)),
            }
        }
        if to_ask.is_empty() {
            continue;
        }

        // 第二层：攒批 LLM 裁决
        let pairs: Vec<utopia_extract::AdjudicationPair> = to_ask
            .iter()
            .map(
                |(item, _, _, precedents)| utopia_extract::AdjudicationPair {
                    left: utopia_extract::AdjudicationSide {
                        name: item.left.name.clone(),
                        type_label: item
                            .left
                            .type_label
                            .clone()
                            .unwrap_or_else(|| "untyped".into()),
                        facts: item.left.top_facts.clone(),
                    },
                    right: utopia_extract::AdjudicationSide {
                        name: item.right.name.clone(),
                        type_label: item
                            .right
                            .type_label
                            .clone()
                            .unwrap_or_else(|| "untyped".into()),
                        facts: item.right.top_facts.clone(),
                    },
                    precedents: precedents.clone(),
                    proposed_because: utopia_extract::proposed_because(item.reason.as_deref()),
                },
            )
            .collect();
        let messages = utopia_extract::build_adjudication_messages(&pairs);
        // 调用/解析失败 → 任务按退避重试；重试耗尽后行停留在队列里，人工仍可定夺
        let _permit = settings.as_ref().map(|s| llm_util::acquire_chat(state, s));
        let _permit = match _permit {
            Some(f) => f.await,
            None => None,
        };
        let reply = client.chat(&messages).await?;
        let verdicts = utopia_extract::parse_adjudication(&reply)?;
        let by_i: HashMap<usize, &utopia_extract::AdjudicationVerdict> =
            verdicts.iter().map(|v| (v.i, v)).collect();

        for (idx, (item, key, p, _)) in to_ask.iter().enumerate() {
            match by_i.get(&idx) {
                Some(v) => {
                    let same = match v.verdict.as_str() {
                        "same" => Some(true),
                        "different" => Some(false),
                        _ => None,
                    };
                    let conf = v.confidence.unwrap_or(0.5).clamp(0.0, 1.0);
                    // 第二层（0028）：攒批没定的，带工具再看一遍再落地。预算用完或
                    // 循环没跑成就照攒批的看法办
                    if needs_second_look(item, p, same, conf) {
                        let earlier = Look::from_batch(same, conf, v.why.clone());
                        if let Some(look) = look_again(
                            state,
                            kb_id,
                            &client,
                            &settings,
                            item,
                            &pairs[idx],
                            &earlier,
                        )
                        .await
                        {
                            let outcome = if look.same.is_some() {
                                utopia_store::resolution::put_verdict(
                                    &state.pool,
                                    kb_id,
                                    key,
                                    look.same,
                                    look.conf,
                                    &model,
                                )
                                .await?;
                                apply_verdict(
                                    state,
                                    kb_id,
                                    item,
                                    look.same,
                                    look.conf,
                                    "investigated",
                                    look.why.as_deref(),
                                )
                                .await?
                            } else {
                                // 问了一个问题，或看了没结论：留给人，卡片上带着建议与问题
                                utopia_store::resolution::escalate_review(
                                    &state.pool,
                                    item.id,
                                    "proposed",
                                )
                                .await?;
                                Outcome::Escalated
                            };
                            record_look(state, kb_id, run_id, item, p, &look, &outcome).await?;
                            continue;
                        }
                    }
                    utopia_store::resolution::put_verdict(
                        &state.pool,
                        kb_id,
                        key,
                        same,
                        conf,
                        &model,
                    )
                    .await?;
                    apply_verdict(
                        state,
                        kb_id,
                        item,
                        same,
                        conf,
                        "adjudicated",
                        v.why.as_deref(),
                    )
                    .await?;
                }
                None => {
                    utopia_store::resolution::escalate_review(
                        &state.pool,
                        item.id,
                        "escalate_no_verdict",
                    )
                    .await?;
                }
            }
        }
        // 本轮裁决落库完毕，推给前端刷新审核队列
        state.emit_review(kb_id);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_verdict(
    state: &AppState,
    kb_id: Uuid,
    item: &ReviewItem,
    same: Option<bool>,
    conf: f32,
    via: &str,
    why: Option<&str>,
) -> anyhow::Result<Outcome> {
    // 有把握就动手，不再抽一成给人（0026 修订）：队列里等人的，只剩机器拿不准、
    // 或闸门说合了会送出图外的那些
    let outcome = match same {
        Some(true) if conf >= AUTO_CONF => {
            // 执行闸门（0027）：合并会立刻送出图外的东西——违规、派生、答案——留给人，
            // 把握再高也不动手。人看到的是留下的原因，不是「裁决器没把握」
            let impact = utopia_store::execution_gate::impact_of(
                &state.pool,
                kb_id,
                item.left.id,
                item.right.id,
            )
            .await?;
            if let Some(hold) = utopia_store::execution_gate::hold(&impact) {
                utopia_store::resolution::escalate_review(
                    &state.pool,
                    item.id,
                    &format!("escalate_impact|{hold}"),
                )
                .await?;
                return Ok(Outcome::Escalated);
            }
            let (target, source) =
                utopia_store::resolution::merge_direction(&state.pool, item.left.id, item.right.id)
                    .await?;
            let reason = format!("auto_merged|{via} {conf:.2}");
            match utopia_store::resolution::merge_entities(
                &state.pool,
                kb_id,
                source,
                target,
                None,
                &reason,
            )
            .await
            {
                Ok(merge_id) => {
                    utopia_store::resolution::close_review_auto(
                        &state.pool,
                        item.id,
                        "merged",
                        &reason,
                    )
                    .await?;
                    // 决策台账：AI 自动合并（actor 为空 = 系统）
                    let _ = utopia_store::audit::record_opt(
                        &state.pool,
                        Some(kb_id),
                        None,
                        "review.merge",
                        "review",
                        Some(item.id),
                        serde_json::json!({
                            "left": item.left.name, "right": item.right.name,
                            "score": item.score, "confidence": conf, "via": via,
                            // 模型的一句理由也留下：机器的行不是先例（0025 决定 1），
                            // 但人看 Decisions 时该看得见它凭什么
                            "why": why,
                        }),
                    )
                    .await;
                    Outcome::Merged(merge_id)
                }
                // 同批次连锁合并可能已吞掉其中一方：转人工而不是让任务失败
                Err(AppError::Conflict(_)) | Err(AppError::NotFound) => {
                    utopia_store::resolution::escalate_review(
                        &state.pool,
                        item.id,
                        "escalate_entity_changed",
                    )
                    .await?;
                    Outcome::Escalated
                }
                Err(e) => return Err(e.into()),
            }
        }
        Some(false) if conf >= AUTO_CONF => {
            utopia_store::resolution::close_review_auto(
                &state.pool,
                item.id,
                "kept",
                &format!("kept_apart|{via} {conf:.2}"),
            )
            .await?;
            let _ = utopia_store::audit::record_opt(
                &state.pool,
                Some(kb_id),
                None,
                "review.keep",
                "review",
                Some(item.id),
                serde_json::json!({
                    "left": item.left.name, "right": item.right.name,
                    "score": item.score, "confidence": conf, "via": via,
                    "why": why,
                }),
            )
            .await;
            Outcome::Kept
        }
        _ => {
            utopia_store::resolution::escalate_review(
                &state.pool,
                item.id,
                &format!("escalate_unsure|{via} {conf:.2}"),
            )
            .await?;
            Outcome::Escalated
        }
    };
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use utopia_core::models::{ReviewItem, ReviewSide};

    fn item(reason: &str) -> ReviewItem {
        let side = |name: &str| ReviewSide {
            id: Uuid::now_v7(),
            name: name.into(),
            type_label: Some("person".into()),
            color: String::new(),
            disambiguator: None,
            degree: 0,
            top_facts: vec![],
        };
        ReviewItem {
            id: Uuid::now_v7(),
            score: 0.78,
            reason: Some(reason.into()),
            stage: "adjudicating".into(),
            created_at: chrono::Utc::now(),
            left: side("张伟"),
            right: side("财务部总监张伟"),
            proposal: None,
        }
    }

    /// 名字向量提的对：批量说 same 再有把握也要第二眼；说 different / unsure 照旧
    #[test]
    fn a_similarity_proposed_same_always_gets_the_second_look() {
        let p = gov::Precedents::default();
        let it = item("name_vector|0.78");
        assert!(needs_second_look(&it, &p, Some(true), 0.99));
        assert!(needs_second_look(&it, &p, Some(true), 0.85));
        assert!(
            !needs_second_look(&it, &p, Some(false), 0.9),
            "分开是安全方向，照旧落地"
        );
        assert!(needs_second_look(&it, &p, None, 0.5), "没定的本来就要再看");
    }

    /// 同名家族的对不受影响：够线就照旧自动落地
    #[test]
    fn a_same_name_pair_keeps_the_old_rule() {
        let p = gov::Precedents::default();
        let it = item("ambiguous_name|0.41");
        assert!(!needs_second_look(&it, &p, Some(true), 0.9));
        assert!(needs_second_look(&it, &p, Some(true), 0.6), "不到线才再看");
    }
}
