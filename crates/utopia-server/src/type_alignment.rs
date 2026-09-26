//! 类别词绑到类（0044 决定 3–4 的第一片：按签名绑定，签名 = 实体的类别词）。
//!
//! 开放抽取只记文档自己的类别词（`entities.specific_type`：company、person、「指标」），
//! 不选类。类是本体的事，绑定是对齐的事：一个库里 distinct 的类别词就几十上百个，每个
//! 只判一次——例句、它参与的关系短语、候选类的定义一起给模型，**两票一致**才绑，不一致
//! 记成 undecided 留给审核（#725 队列 2），没有类对得上的记成 none 并按老流程提成
//! 「建议加类」（`proposed_type` → 本体页采纳）。绑定存在 `type_bindings`，连同判定时给
//! 模型看的候选类的指纹（`basis`：各自的 `updated_at` 与祖先闭包，0053 的类别词那一半）。
//! 过期 = 按现在的类重算的指纹对不上，本体一改只重判对不上的。从前按时间戳判，看不见
//! 两件事：模型答题期间改的定义（判定写在答完之后，时间戳说判定更新，#795），和不碰
//! `updated_at` 的父边增删。绑上的类写到该类别词下每个实体的 `type_id`（`type_source =
//! 'aligned'`，人定过类的不动），身份消解的按类圈范围随之恢复。
//!
//! 输入在调模型之前从一个快照里读齐；写判定时在同一事务里按当前的类再算一遍指纹，对不上
//! 就不收这份回复、再排一轮（`type_bindings::decide_and_apply_if_current`）。
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
use utopia_store::type_bindings::{self, Acceptance, Binding, KindWordSignature};
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
///
/// 检索报错也退回整表，同短语那边：候选算在判定的指纹里，报错这一轮判的词恢复以后
/// 指纹对不上、再问一轮。不退回的话，嵌入端点一直坏着时新来的词就永远没有类
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
    let nearest = match ontology_index::nearest_for_each(
        state,
        kb_id,
        &queries,
        CANDIDATES,
        Target::ClassLabel,
    )
    .await
    {
        Ok(nearest) => nearest,
        Err(e) => {
            tracing::warn!(%kb_id, error = %e, "类别词对齐检索候选类失败，退回整表");
            Vec::new()
        }
    };
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

/// 日志里放得下的一段回复：空白折成一个空格，最多这么多字符。
const SNIPPET_CHARS: usize = 240;

fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(SNIPPET_CHARS).collect();
    if flat.chars().count() > SNIPPET_CHARS {
        out.push('…');
    }
    out
}

/// 一轮里没判完的（调用失败、回复读不出、模型漏答）自己再排几次；超过这个数就等
/// 下一篇文档或本体的改动再问。不设上限的话，温度为零下一段每次都读不出的回复会让
/// 任务每隔几十秒把同一段提示词再送一遍，没有尽头
const MAX_REASK: u32 = 3;

/// 对一个库跑一遍：新出现的和过期的类别词各判一次，绑上的写类，没有的提成建议。
/// `reask` 是这份任务已经是第几次自己排的（文档、建类排的是 0）。
pub async fn align_types_reasking(state: &AppState, kb_id: Uuid, reask: u32) -> anyhow::Result<()> {
    let pool = &state.pool;
    let kb = utopia_store::kbs::get(pool, kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align kind words"))?;
    let client = llm_util::chat_client_thinking(&settings)
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
    let result = align_types_locked(state, kb_id, reask, &settings, &client).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('align_types'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *guard)
        .await;
    result
}

async fn align_types_locked(
    state: &AppState,
    kb_id: Uuid,
    reask: u32,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    // 调模型之前在一个快照里读齐输入（#795）：类的定义、类的版本与父边、类别词、已有的
    // 判定。分几次读的话，两次读之间提交的编辑会让提示词里的定义和指纹里的版本对不上
    let (classes, snapshot, sigs, existing) = {
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let classes = utopia_store::graph::entity_types(&mut *tx, kb_id).await?;
        let snapshot = type_bindings::class_snapshot(&mut *tx, kb_id).await?;
        let sigs = type_bindings::signatures(&mut *tx, kb_id).await?;
        let existing: HashMap<String, Binding> = type_bindings::bindings(&mut *tx, kb_id)
            .await?
            .into_iter()
            .map(|b| (b.kind_word.clone(), b))
            .collect();
        tx.commit().await?;
        (classes, snapshot, sigs, existing)
    };
    let by_id: HashMap<Uuid, &EntityType> = classes.iter().map(|c| (c.id, c)).collect();
    let by_key: HashMap<&str, &EntityType> = classes.iter().map(|c| (c.key.as_str(), c)).collect();
    // 每个活着的类别词此刻给模型看的候选与指纹。检索一轮只做一次：开跑时拿它决定判谁，
    // 收尾时拿同一份候选、按重新读的类再算指纹，看跑着的时候有没有东西过期
    let all: Vec<&KindWordSignature> = sigs.iter().collect();
    let considered: HashMap<String, (Vec<Uuid>, String)> = sigs
        .iter()
        .zip(candidates_for(state, kb_id, &all, &classes).await?)
        .map(|(s, c)| {
            let basis = snapshot.basis(&c);
            (s.kind_word.clone(), (c, basis))
        })
        .collect();
    // 过期 = 存下的指纹和此刻的不一样（没有指纹的是这一列出现前判的，各重判一次）。人的判定不重判
    let todo: Vec<&KindWordSignature> = sigs
        .iter()
        .filter(|s| match existing.get(&s.kind_word) {
            None => true,
            Some(b) => {
                b.decided_by != "person"
                    && b.basis.as_deref() != Some(considered[&s.kind_word].1.as_str())
            }
        })
        .collect();
    let attempted: HashSet<String> = todo.iter().map(|s| s.kind_word.clone()).collect();
    // 本轮写下的，或人已判过、代理不覆盖的。问了却没落下结论的不算，见收尾
    let mut settled: HashSet<String> = HashSet::new();
    tracing::info!(%kb_id, kind_words = sigs.len(), to_decide = todo.len(), classes = classes.len(), "类别词对齐开始");

    // 没有类可绑：每个词都是「没有」，并提成建议；类出现后候选变了，指纹对不上，它们会再交回来
    if classes.is_empty() {
        for s in &todo {
            let (candidates, basis) = &considered[&s.kind_word];
            let accepted = type_bindings::decide_and_apply_if_current(
                pool,
                kb_id,
                &s.kind_word,
                &s.words,
                None,
                "none",
                &serde_json::json!({ "reason": "no classes" }),
                basis,
                candidates,
            )
            .await?;
            if accepted == Acceptance::Written {
                type_bindings::propose(
                    pool,
                    kb_id,
                    &s.kind_word,
                    s.words.first().unwrap_or(&s.kind_word),
                )
                .await?;
            }
        }
        return Ok(());
    }

    let (mut bound, mut none, mut undecided, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    // 调用或解析失败的批次：这轮跳过，结束时自己再排一次，不等下一篇文档来排
    let mut failed = 0usize;
    // 问了、模型也答了、却没答到的词：两票缺一票就不下结论
    let mut unanswered = 0usize;
    // 模型答题期间候选类变了、回复没收下的词：它们答的是旧输入，收尾时再排一轮拿新的问
    let mut moved = 0usize;
    for batch in todo.chunks(BATCH) {
        let cands: Vec<&[Uuid]> = batch
            .iter()
            .map(|s| considered[&s.kind_word].0.as_slice())
            .collect();
        // 两票：第二票把候选倒过来给，防止「选第一个」这种顺序偏好冒充一致
        let mut votes: Vec<(Vote, Vote)> = vec![(None, None); batch.len()];
        let mut answered = vec![(false, false); batch.len()];
        for pass in 0..2 {
            let items: Vec<KindWordItem<'_>> = batch
                .iter()
                .enumerate()
                .filter(|(i, _)| !cands[*i].is_empty())
                .map(|(i, s)| {
                    let mut ids: Vec<Uuid> = cands[i].to_vec();
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
            let (choices, malformed) = match parse_kind_word_response(&reply.text, &items) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "类别词对齐回复解析失败，这一批留到下次");
                    failed += 1;
                    continue;
                }
            };
            skipped += malformed;
            if choices.is_empty() {
                // 解出来了却一对都没读到：回复的形状不是我们认得的。这和解析失败是一回事，
                // 按失败算、留到下次。从前这里什么都不说，一个库连着十几轮一个词都没绑上，
                // 日志里只有一串「完成 bound=0」——回复的开头要进日志，下次才知道它长什么样
                tracing::warn!(
                    %kb_id,
                    pass,
                    items = items.len(),
                    malformed,
                    finish_reason = ?reply.finish_reason,
                    chars = reply.text.chars().count(),
                    reply = %snippet(&reply.text),
                    "类别词对齐回复读不出一项，这一批留到下次"
                );
                failed += 1;
                continue;
            }
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
                unanswered += 1;
                continue;
            }
            let record = serde_json::json!({ "first": a, "second": b });
            let (type_id, status) = if !agree(a, b) {
                (None, "undecided")
            } else {
                match a.as_deref().and_then(|k| by_key.get(k)) {
                    Some(class) => (Some(class.id), "bound"),
                    None => (None, "none"),
                }
            };
            let (candidates, basis) = &considered[&s.kind_word];
            match type_bindings::decide_and_apply_if_current(
                pool,
                kb_id,
                &s.kind_word,
                &s.words,
                type_id,
                status,
                &record,
                basis,
                candidates,
            )
            .await?
            {
                Acceptance::Written => {
                    settled.insert(s.kind_word.clone());
                    match status {
                        "bound" => bound += 1,
                        "undecided" => undecided += 1,
                        _ => {
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
                Acceptance::KeptPerson => {
                    settled.insert(s.kind_word.clone());
                }
                Acceptance::Moved => {
                    tracing::info!(%kb_id, kind_word = %s.kind_word, "类别词对齐：模型答题期间候选类变了，这份回复不收");
                    moved += 1;
                }
            }
        }
    }
    tracing::info!(%kb_id, bound, none, undecided, skipped, failed, unanswered, moved, "类别词对齐完成");
    if unanswered > 0 {
        tracing::warn!(%kb_id, unanswered, "类别词对齐有词模型没答到，这些词这轮没有结论");
    }
    if bound + none + undecided > 0 {
        state.emit_graph(kb_id);
    }
    // 同短语对齐：来了没试过的新词、本轮判完的又过期了，就再排一次（从头算一份，
    // 新词换了提示词）。「过期」不限本轮判的，跑着时改的类也要让老绑定再判一次——比的
    // 是按**重新读**的类算出来的指纹（候选沿用开跑时检索的那份）。本轮问了却没落下结论
    // 的词不在这里算：调用失败、读不出、漏答、没有候选的走下面有上限的再问；算进来的话，
    // 一个永久报错的端点会让任务一轮接一轮立刻重排
    let changed = moved > 0 || {
        let now = type_bindings::class_snapshot(pool, kb_id).await?;
        let decided: HashMap<String, Binding> = type_bindings::bindings(pool, kb_id)
            .await?
            .into_iter()
            .map(|b| (b.kind_word.clone(), b))
            .collect();
        type_bindings::signatures(pool, kb_id)
            .await?
            .iter()
            .any(|s| match decided.get(&s.kind_word) {
                None => !attempted.contains(&s.kind_word),
                Some(b) => {
                    b.decided_by != "person"
                        && (settled.contains(&s.kind_word) || !attempted.contains(&s.kind_word))
                        && considered.get(&s.kind_word).is_some_and(|(candidates, _)| {
                            b.basis.as_deref() != Some(now.basis(candidates).as_str())
                        })
                }
            })
    };
    finish(state, kb_id, reask, changed, failed, unanswered).await
}

/// 一轮的收尾：有新活就立刻再排一轮；没判完的延时再问，次数有上限；不再排自己时才排
/// 短语对齐。
async fn finish(
    state: &AppState,
    kb_id: Uuid,
    reask: u32,
    changed: bool,
    failed: usize,
    unanswered: usize,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    // 本轮没判完的（调用失败、回复读不出、模型漏答了几个 id）自己再排，最多 MAX_REASK 次，
    // 每次多等一会。从前只有失败的批次会再排，漏答的词就只能等下一篇文档来排——最后
    // 一篇之后没有下一篇，它们就永远没有结论；而漏答不写任何行，本体页也看不见
    let unfinished = failed > 0 || unanswered > 0;
    // 还有下一轮（来了新词、有过期的，或没判完要再问）就先不排短语对齐：两端的类还在变，
    // 这时判的签名指纹一变就得重判——100 篇的一次跑里，短语对齐第一轮判的 1010 条签名
    // 里有 713 条因为类别词第二轮改了类而重判。最后一轮（不再排自己）才排它；再问的
    // 次数用尽那一支也排：判定不会再变了
    let mut another_round = false;
    if changed {
        utopia_store::jobs::enqueue_unless_queued(
            pool,
            "align_types",
            serde_json::json!({ "kb_id": kb_id }),
        )
        .await?;
        another_round = true;
    } else if unfinished && reask < MAX_REASK {
        let delay = std::time::Duration::from_secs(20 * u64::from(reask + 1));
        tracing::info!(%kb_id, failed, unanswered, reask = reask + 1, delay_secs = delay.as_secs(), "类别词对齐没判完，稍后再问");
        utopia_store::jobs::enqueue_unless_pending(
            pool,
            "align_types",
            serde_json::json!({ "kb_id": kb_id, "reask": reask + 1 }),
            delay,
        )
        .await?;
        another_round = true;
    } else if unfinished {
        tracing::warn!(%kb_id, failed, unanswered, reask, "类别词对齐问了几轮仍没判完，等下一篇文档或本体改动再问");
    }
    // 两端的类定了，短语的签名才定：短语对齐排在类别词对齐**收尾**之后
    if !another_round {
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
#[path = "type_alignment_tests.rs"]
mod tests;
