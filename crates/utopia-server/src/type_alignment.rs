//! 类别词绑到类（0044 决定 3–4 的第一片：按签名绑定，签名 = 实体的类别词）。
//!
//! 开放抽取只记文档自己的类别词（`entities.specific_type`：company、person、「指标」），
//! 不选类。类是本体的事，绑定是对齐的事：一个库里 distinct 的类别词就几十上百个，每个
//! 只判一次——例句、它参与的关系短语、候选类的定义一起给模型，**两票一致**才绑，不一致
//! 记成 undecided 留给审核（#725 队列 2），没有类对得上的记成 none 并按老流程提成
//! 「建议加类」（`proposed_type` → 本体页采纳）。绑定存在 `type_bindings`，按类的
//! `updated_at` 与库里最新的类判过期，本体一改只重判过期的（决策记录里说的「版本」就是
//! 这两个时间戳）。绑上的类写到该类别词下每个实体的 `type_id`（`type_source = 'aligned'`，
//! 人定过类的不动），身份消解的按类圈范围随之恢复。
//!
//! 候选类怎么来：库配了嵌入模型就按类别词加例名检索最近的类；没配就在类不多时整表给；
//! 类太多又没嵌入时不判——瞎判比不判糟。

use crate::extraction::chat_retrying_rate_limits_at;
use crate::llm_util;
use crate::ontology_index::{self, Target};
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
use utopia_core::models::EntityType;
use utopia_extract::align::{
    build_kind_word_messages, parse_kind_word_response, ClassCandidate, KindWordItem,
};
use utopia_store::type_bindings::{self, KindWordSignature};
use uuid::Uuid;

/// 一次问多少个类别词。
const BATCH: usize = 20;
/// 每个类别词给几个候选类。
const CANDIDATES: i64 = 10;
/// 没有嵌入模型时，类不超过这个数就整表给。
const WHOLE_LIST_LIMIT: usize = 60;

/// 一票：这个类别词选了哪个键（None = 没有类对得上）。
type Vote = Option<String>;

/// 给一批类别词找候选类：有嵌入就检索，没有就整表（类少时）。返回每个签名的候选 id 列表。
async fn candidates_for(
    state: &AppState,
    kb_id: Uuid,
    sigs: &[&KindWordSignature],
    classes: &[EntityType],
) -> anyhow::Result<Vec<Vec<Uuid>>> {
    let queries: Vec<String> = sigs
        .iter()
        .map(|s| format!("{}: {}", s.kind_word, s.examples.join(", ")))
        .collect();
    let nearest =
        ontology_index::nearest_for_each(state, kb_id, &queries, CANDIDATES, Target::ClassLabel)
            .await
            .unwrap_or_default();
    let mut out = Vec::with_capacity(sigs.len());
    for (i, _) in sigs.iter().enumerate() {
        let found: Vec<Uuid> = nearest
            .get(i)
            .map(|v| v.iter().map(|c| c.id).collect())
            .unwrap_or_default();
        if !found.is_empty() {
            out.push(found);
        } else if classes.len() <= WHOLE_LIST_LIMIT {
            out.push(classes.iter().map(|c| c.id).collect());
        } else {
            out.push(Vec::new());
        }
    }
    Ok(out)
}

/// 两票是否一致：都选同一个键，或都说没有。
fn agree(a: &Vote, b: &Vote) -> bool {
    a == b
}

/// 对一个库跑一遍：新出现的和过期的类别词各判一次，绑上的写类，没有的提成建议。
pub async fn align_types(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let pool = &state.pool;
    let kb = utopia_store::kbs::get(pool, kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align kind words"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align kind words"))?;
    // 一个库同时只跑一份：抽完每篇、建每个类都会排一次，排队去重只挡「排队中」的，
    // 后一个开跑时前一个还在跑就并行了——实测种 14 个类跑出 14 份并行任务，把模型端点
    // 打出 502。拿不到锁的直接退出，正在跑的那份会看到同一批词；本轮没判到的下一轮再来
    let mut guard = pool.acquire().await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('align_types'), hashtext($1))")
            .bind(kb_id.to_string())
            .fetch_one(&mut *guard)
            .await?;
    if !locked {
        // 正在跑的那份结束时会自己看一眼有没有新东西（见 align_types_locked 末尾）；这里不排
        tracing::info!(%kb_id, "类别词对齐已有一份在跑，这次跳过");
        return Ok(());
    }
    let result = align_types_locked(state, kb_id, &settings, &client).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('align_types'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *guard)
        .await;
    result
}

async fn align_types_locked(
    state: &AppState,
    kb_id: Uuid,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let run_started = chrono::Utc::now();
    let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
    let by_id: HashMap<Uuid, &EntityType> = classes.iter().map(|c| (c.id, c)).collect();
    let by_key: HashMap<&str, &EntityType> = classes.iter().map(|c| (c.key.as_str(), c)).collect();
    let sigs = type_bindings::signatures(pool, kb_id).await?;
    let existing: HashMap<String, utopia_store::type_bindings::Binding> =
        type_bindings::bindings(pool, kb_id)
            .await?
            .into_iter()
            .map(|b| (b.kind_word.clone(), b))
            .collect();
    let stale: HashSet<String> = type_bindings::stale(pool, kb_id)
        .await?
        .into_iter()
        .collect();
    let todo: Vec<&KindWordSignature> = sigs
        .iter()
        .filter(|s| match existing.get(&s.kind_word) {
            None => true,
            Some(b) => b.decided_by != "person" && stale.contains(&s.kind_word),
        })
        .collect();
    let attempted: HashSet<String> = todo.iter().map(|s| s.kind_word.clone()).collect();
    tracing::info!(%kb_id, kind_words = sigs.len(), to_decide = todo.len(), classes = classes.len(), "类别词对齐开始");

    // 没有类可绑：每个词都是「没有」，并提成建议；类出现后 `stale` 会把它们再交回来
    if classes.is_empty() {
        for s in &todo {
            type_bindings::decide(
                pool,
                kb_id,
                &s.kind_word,
                &s.words,
                None,
                "none",
                &serde_json::json!({ "reason": "no classes" }),
                "agent",
            )
            .await?;
            type_bindings::propose(
                pool,
                kb_id,
                &s.kind_word,
                s.words.first().unwrap_or(&s.kind_word),
            )
            .await?;
        }
        return Ok(());
    }

    let (mut bound, mut none, mut undecided, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    // 调用或解析失败的批次：这轮跳过，结束时自己再排一次，不等下一篇文档来排
    let mut failed = 0usize;
    for batch in todo.chunks(BATCH) {
        let cands = candidates_for(state, kb_id, batch, &classes).await?;
        // 两票：第二票把候选倒过来给，防止「选第一个」这种顺序偏好冒充一致
        let mut votes: Vec<(Vote, Vote)> = vec![(None, None); batch.len()];
        let mut answered = vec![(false, false); batch.len()];
        for pass in 0..2 {
            let items: Vec<KindWordItem<'_>> = batch
                .iter()
                .enumerate()
                .filter(|(i, _)| !cands[*i].is_empty())
                .map(|(i, s)| {
                    let mut ids: Vec<Uuid> = cands[i].clone();
                    if pass == 1 {
                        ids.reverse();
                    }
                    KindWordItem {
                        id: i as i64,
                        kind_word: &s.kind_word,
                        spellings: &s.words,
                        examples: &s.examples,
                        phrases: &s.phrases,
                        candidates: ids
                            .iter()
                            .filter_map(|id| by_id.get(id))
                            .map(|c| ClassCandidate {
                                key: &c.key,
                                label: &c.label,
                                description: &c.description,
                            })
                            .collect(),
                    }
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            let messages = build_kind_word_messages(&items);
            let reply =
                match chat_retrying_rate_limits_at(state, settings, client, &messages, Some(0.0))
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(%kb_id, error = %e, "类别词对齐调用失败，这一批留到下次");
                        failed += 1;
                        continue;
                    }
                };
            let (choices, malformed) = match parse_kind_word_response(&reply, &items) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "类别词对齐回复解析失败，这一批留到下次");
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
                        slot.0 = c.key;
                        answered[i].0 = true;
                    } else {
                        slot.1 = c.key;
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
            let record = serde_json::json!({ "first": a, "second": b });
            if !agree(a, b) {
                type_bindings::decide(
                    pool,
                    kb_id,
                    &s.kind_word,
                    &s.words,
                    None,
                    "undecided",
                    &record,
                    "agent",
                )
                .await?;
                undecided += 1;
                continue;
            }
            match a.as_deref().and_then(|k| by_key.get(k)) {
                Some(class) => {
                    if type_bindings::decide(
                        pool,
                        kb_id,
                        &s.kind_word,
                        &s.words,
                        Some(class.id),
                        "bound",
                        &record,
                        "agent",
                    )
                    .await?
                    {
                        type_bindings::apply(pool, kb_id, &s.kind_word, class.id).await?;
                        bound += 1;
                    }
                }
                None => {
                    if type_bindings::decide(
                        pool,
                        kb_id,
                        &s.kind_word,
                        &s.words,
                        None,
                        "none",
                        &record,
                        "agent",
                    )
                    .await?
                    {
                        type_bindings::unapply(pool, kb_id, &s.kind_word).await?;
                        type_bindings::propose(
                            pool,
                            kb_id,
                            &s.kind_word,
                            s.words.first().unwrap_or(&s.kind_word),
                        )
                        .await?;
                        none += 1;
                    }
                }
            }
        }
    }
    tracing::info!(%kb_id, bound, none, undecided, skipped, failed, "类别词对齐完成");
    if bound > 0 {
        state.emit_graph(kb_id);
    }
    // 同短语对齐：失败过、来了没试过的新词、本轮判完的又过期了，就再排一次
    let again = failed > 0 || {
        let stale_now: HashSet<String> = type_bindings::stale(pool, kb_id)
            .await?
            .into_iter()
            .collect();
        type_bindings::signatures(pool, kb_id)
            .await?
            .iter()
            .any(|s| !attempted.contains(&s.kind_word) && !existing.contains_key(&s.kind_word))
            || type_bindings::bindings(pool, kb_id).await?.iter().any(|b| {
                b.decided_by != "person"
                    && b.decided_at >= run_started
                    && stale_now.contains(&b.kind_word)
            })
    };
    if again {
        utopia_store::jobs::enqueue_unless_queued(
            pool,
            "align_types",
            serde_json::json!({ "kb_id": kb_id }),
        )
        .await?;
    }
    // 两端的类定了，短语的签名才定：短语对齐排在它后面
    utopia_store::jobs::enqueue_unless_queued(
        pool,
        "align_phrases",
        serde_json::json!({ "kb_id": kb_id }),
    )
    .await?;
    Ok(())
}
