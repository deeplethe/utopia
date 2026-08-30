//! 实体消解攒批裁决任务：消费审核队列中 stage=adjudicating 的灰区对。
//! 先查裁决缓存，缓存未命中的攒成一批（一次 LLM 调用裁多对）；
//! 高置信 same → 自动合并（可回滚），高置信 different → 自动保持分开，
//! 其余转人工。未配模型时全部转人工——本任务失败或缺席都不影响抽取与查询。

use crate::llm_util;
use crate::state::AppState;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use utopia_core::models::ReviewItem;
use utopia_core::AppError;
use uuid::Uuid;

const BATCH_SIZE: i64 = 12;
const AUTO_CONF: f32 = 0.8;
const MAX_ROUNDS: usize = 20;

/// 缓存键：类型 + 双方名字 + 事实摘要（与实体 id 无关——重传文档不重复付费）。
fn pair_key(item: &ReviewItem) -> String {
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
    let digest = Sha256::digest(sides.join("##").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn adjudicate_entities(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
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

    for _ in 0..MAX_ROUNDS {
        let items =
            utopia_store::resolution::pending_adjudications(&state.pool, kb_id, BATCH_SIZE).await?;
        if items.is_empty() {
            break;
        }

        // 第一层：裁决缓存
        let mut to_ask: Vec<(ReviewItem, String)> = Vec::new();
        for item in items {
            let key = pair_key(&item);
            match utopia_store::resolution::get_verdict(&state.pool, kb_id, &key).await? {
                Some((same, conf)) => {
                    apply_verdict(state, kb_id, &item, same, conf, "cached").await?
                }
                None => to_ask.push((item, key)),
            }
        }
        if to_ask.is_empty() {
            continue;
        }

        // 第二层：攒批 LLM 裁决
        let pairs: Vec<utopia_extract::AdjudicationPair> = to_ask
            .iter()
            .map(|(item, _)| utopia_extract::AdjudicationPair {
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
            })
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

        for (idx, (item, key)) in to_ask.iter().enumerate() {
            match by_i.get(&idx) {
                Some(v) => {
                    let same = match v.verdict.as_str() {
                        "same" => Some(true),
                        "different" => Some(false),
                        _ => None,
                    };
                    let conf = v.confidence.unwrap_or(0.5).clamp(0.0, 1.0);
                    utopia_store::resolution::put_verdict(
                        &state.pool,
                        kb_id,
                        key,
                        same,
                        conf,
                        &model,
                    )
                    .await?;
                    apply_verdict(state, kb_id, item, same, conf, "adjudicated").await?;
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

async fn apply_verdict(
    state: &AppState,
    kb_id: Uuid,
    item: &ReviewItem,
    same: Option<bool>,
    conf: f32,
    via: &str,
) -> anyhow::Result<()> {
    match same {
        Some(true) if conf >= AUTO_CONF => {
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
                Ok(_) => {
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
                        }),
                    )
                    .await;
                }
                // 同批次连锁合并可能已吞掉其中一方：转人工而不是让任务失败
                Err(AppError::Conflict(_)) | Err(AppError::NotFound) => {
                    utopia_store::resolution::escalate_review(
                        &state.pool,
                        item.id,
                        "escalate_entity_changed",
                    )
                    .await?;
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
                }),
            )
            .await;
        }
        _ => {
            utopia_store::resolution::escalate_review(
                &state.pool,
                item.id,
                &format!("escalate_unsure|{via} {conf:.2}"),
            )
            .await?;
        }
    }
    Ok(())
}
