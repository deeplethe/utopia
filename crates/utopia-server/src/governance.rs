//! 治理任务（0025）：开关开着就把等人的重复对按先进先出过一遍，先读台账再裁。
//!
//! 人裁过的一对（同一对实体，或名字不同的同一对名字）照人的定，不问模型。
//! 其余两层。第一层是攒批：队头带着它的簇进一次模型调用，先例已经在提示词里；过闸的
//! 自己动手（合并可撤、分开可再合）。第二层（第二刀）只接第一层**判不定**的对：
//! 逐条带工具再看一遍——一侧的全部事实、原文片段、台账里人对这个名字的决定、
//! 同名的其他实体——看完要么 decide，要么 defer 留一个具体的问题给人。硬规则拦下
//! 的对（类型冲突、有撤回、与人对这一对的决定相反）不进第二层：再看也不会改规则。
//!
//! 两簇之间看一眼开关，关掉就停在这里——「关闭后队列自动终止」就是那一行。模型
//! 缺席时任务成功结束、什么都不动：没有依据的裁决不如不裁，队列原地等。
//!
//! 与攒批裁决器（`adjudication`）的分工：开关关着，那一档照旧独自判灰区对；
//! 开着，抽取结束排的是这里，灰区对与等人的对都从这条队列走。这里不读也不写
//! 裁决缓存——缓存键里没有先例，而这里的答案随先例变。

use crate::llm_util;
use crate::state::AppState;
use chrono::{Duration, Utc};
use futures_util::future::join_all;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use utopia_core::models::{LlmSettings, ReviewItem, Role};
use utopia_core::AppError;
use utopia_extract::governor::{self, Step};
use utopia_llm::{tool_result_message, LlmClient};
use utopia_store::alerts;
use utopia_store::execution_gate;
use utopia_store::governance::{self as gov, Gate, NewDecision, Precedents, AUTO_CONF};
use uuid::Uuid;

/// 一次模型调用带几对：队头加它的簇
const BATCH_SIZE: i64 = 12;
/// 一个任务最多走几轮，之后再排一个接着走——让别的库的任务也轮得上
const MAX_ROUNDS: usize = 20;
/// 每库每天第二层最多花几次模型调用。用完了，判不定的对照第一刀写成建议。
/// 300 在一次 229 块的导入上第一轮就用光了（132 对花了 301 次），所以是 2000
const LOOP_DAILY_CALLS: i64 = 2000;
/// 一次工具结果最多给模型多少字：原文片段与事实列表都可能很长
const TOOL_OUTPUT_CHARS: usize = 3000;

/// 一轮任务里到处要带的东西
struct Ctx<'a> {
    state: &'a AppState,
    kb_id: Uuid,
    run_id: Uuid,
    client: &'a LlmClient,
    settings: &'a Option<LlmSettings>,
}

/// 对一对的一次看法：第一层给的，或第二层看完改过的。裁决器（治理关着时）也用它：
/// 判不定的对走同一个第二层（0028）
pub(crate) struct Look {
    pub(crate) same: Option<bool>,
    pub(crate) conf: f32,
    pub(crate) why: Option<String>,
    /// defer 留下的问题
    pub(crate) question: Option<String>,
    /// 第二层看了什么
    pub(crate) trace: Vec<Value>,
    /// 第二层花的模型调用
    pub(crate) calls: i32,
}

impl Look {
    pub(crate) fn from_batch(same: Option<bool>, conf: f32, why: Option<String>) -> Self {
        Look {
            same,
            conf,
            why,
            question: None,
            trace: Vec::new(),
            calls: 0,
        }
    }

    /// 人裁过这一对：照人的定，不问模型
    fn from_people(merged: bool) -> Self {
        Look {
            same: Some(merged),
            conf: 1.0,
            why: Some("a person decided this same pair before".into()),
            question: None,
            trace: Vec::new(),
            calls: 0,
        }
    }

    /// 第一层没定：没判决，或置信度不到线
    fn uncertain(&self) -> bool {
        self.same.is_none() || self.conf < AUTO_CONF
    }

    fn action(&self) -> &'static str {
        match self.same {
            Some(true) => "merge",
            Some(false) => "keep",
            None => "unsure",
        }
    }
}

pub async fn govern(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    if !kb.governance {
        return Ok(());
    }
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id).await?;
    let Some(client) = settings.as_ref().and_then(llm_util::chat_client) else {
        tracing::info!(%kb_id, "治理：没有配聊天模型，队列原地等");
        return Ok(());
    };
    // 同库已经有一个在跑：挂回队列等它跑完（不烧重试次数），别并排裁同一批
    let Some(run) = gov::claim_run(&state.pool, kb_id).await? else {
        return Err(
            anyhow::Error::msg("another govern run holds this base").context(
                utopia_core::Deferred::new(std::time::Duration::from_secs(30)),
            ),
        );
    };
    let outcome = govern_held(state, kb_id, &client, &settings).await;
    run.release().await;
    outcome
}

/// 拿到库锁之后的一次治理：重复对的几轮，再是其余几档
async fn govern_held(
    state: &AppState,
    kb_id: Uuid,
    client: &LlmClient,
    settings: &Option<LlmSettings>,
) -> anyhow::Result<()> {
    let ctx = Ctx {
        state,
        kb_id,
        run_id: Uuid::now_v7(),
        client,
        settings,
    };

    // 上一次任务半路留下的锁先放掉；跑完（不管怎么结束的）再放一次
    gov::release_locks(&state.pool, kb_id).await?;
    let outcome = rounds(&ctx).await;
    if let Err(e) = gov::release_locks(&state.pool, kb_id).await {
        tracing::warn!(%kb_id, error = %e, "治理：放锁失败");
    }
    state.emit_review(kb_id);
    let more = outcome?;
    // 重复对之后是其余几档（0043）：合并先定下来，事实与冲突看到的才是合并之后的图
    let queues = crate::queue_agent::Ctx {
        state,
        kb_id,
        run_id: ctx.run_id,
        client,
        settings,
    };
    let more = crate::queue_agent::run(&queues).await? || more;

    // 轮数用完还有积压：再排一个，下一轮从队头接着走
    if more {
        utopia_store::jobs::enqueue_unless_queued(&state.pool, "govern", json!({ "kb_id": kb_id }))
            .await?;
    }
    Ok(())
}

/// 一轮里并行裁几簇。只有互不共享实体、也不共享名字的簇才能同一轮：一簇里的合并
/// 会改另一簇看到的东西。模型那边的并发由 llm_util 的闸门限，这里只决定一次挂几个
const CLUSTERS_PER_ROUND: usize = 3;
/// 挑簇时从队头往后看多少对
const HEADS_TO_SCAN: i64 = 60;

/// 一簇：等人的对、各自的先例、给模型看的样子、已经有的看法（人定过的快路）
struct Cluster {
    items: Vec<ReviewItem>,
    precedents: Vec<Precedents>,
    pairs: Vec<utopia_extract::AdjudicationPair>,
    looks: Vec<Option<Look>>,
}

/// 一轮轮走到队列空、开关关或轮数用完。回 true = 轮数用完还有积压
async fn rounds(ctx: &Ctx<'_>) -> anyhow::Result<bool> {
    let state = ctx.state;
    let kb_id = ctx.kb_id;
    let pool = &state.pool;
    for _ in 0..MAX_ROUNDS {
        // 关闭后队列自动终止：每一轮之前看一眼
        if !utopia_store::kbs::get(pool, kb_id).await?.governance {
            tracing::info!(%kb_id, "治理：开关已关，停在这里");
            return Ok(false);
        }

        // 从队头起挑互不重叠的簇，最多 K 簇；和已取的簇有交集的头留到下一轮
        let heads = gov::queue(pool, kb_id, HEADS_TO_SCAN).await?;
        if heads.is_empty() {
            return Ok(false);
        }
        let mut clusters: Vec<Vec<ReviewItem>> = Vec::new();
        let mut taken_ids: HashSet<Uuid> = HashSet::new();
        let mut taken_entities: HashSet<Uuid> = HashSet::new();
        let mut taken_names: HashSet<String> = HashSet::new();
        for head in heads {
            if clusters.len() >= CLUSTERS_PER_ROUND {
                break;
            }
            if taken_ids.contains(&head.id) {
                continue;
            }
            let mut items = vec![head];
            let siblings = gov::cluster_of(pool, kb_id, &items[0], BATCH_SIZE - 1).await?;
            items.extend(siblings);
            let overlaps = items.iter().any(|i| {
                taken_ids.contains(&i.id)
                    || taken_entities.contains(&i.left.id)
                    || taken_entities.contains(&i.right.id)
                    || taken_names.contains(&i.left.name.to_lowercase())
                    || taken_names.contains(&i.right.name.to_lowercase())
            });
            if overlaps {
                continue;
            }
            for i in &items {
                taken_ids.insert(i.id);
                taken_entities.insert(i.left.id);
                taken_entities.insert(i.right.id);
                taken_names.insert(i.left.name.to_lowercase());
                taken_names.insert(i.right.name.to_lowercase());
            }
            clusters.push(items);
        }
        // 这几簇归 agent 了：人在这几分钟里不能裁它们，界面上看得见
        let ids: Vec<Uuid> = taken_ids.into_iter().collect();
        gov::lock(pool, kb_id, &ids).await?;
        state.emit_review(kb_id);

        // 先例与快路（库操作，顺序做）
        let mut round: Vec<Cluster> = Vec::with_capacity(clusters.len());
        for items in clusters {
            let mut precedents = Vec::with_capacity(items.len());
            for item in &items {
                precedents.push(gov::precedents_for(pool, kb_id, item).await?);
            }
            let pairs: Vec<utopia_extract::AdjudicationPair> = items
                .iter()
                .zip(&precedents)
                .map(|(item, p)| pair_of(item, p))
                .collect();
            let mut looks = Vec::with_capacity(items.len());
            for (idx, item) in items.iter().enumerate() {
                let prior = gov::decided_before(pool, kb_id, item.left.id, item.right.id).await?;
                looks.push(
                    gov::settled_by_people(
                        &item.left.name,
                        &item.right.name,
                        &precedents[idx],
                        prior,
                    )
                    .map(Look::from_people),
                );
            }
            round.push(Cluster {
                items,
                precedents,
                pairs,
                looks,
            });
        }

        // 攒批：每簇一次调用，几簇同时挂。任何一次失败整轮失败，任务按退避重试；
        // 重试耗尽后这些对留在队列里，人照样能裁
        let batches = join_all(round.iter().map(|c| batch_looks(ctx, c))).await;
        for (c, looks) in round.iter_mut().zip(batches) {
            for (idx, look) in looks? {
                c.looks[idx] = Some(look);
            }
        }

        // 第二层：判不定的、同名却说不同的，带工具再看一遍——几对同时看，
        // 模型那边由闸门限并发
        let mut second: Vec<(usize, usize)> = Vec::new();
        for (ci, c) in round.iter().enumerate() {
            for (idx, look) in c.looks.iter().enumerate() {
                if let Some(look) = look {
                    if wants_second_look(&c.items[idx], &c.precedents[idx], look) {
                        second.push((ci, idx));
                    }
                }
            }
        }
        let seen = join_all(second.iter().map(|&(ci, idx)| {
            let c = &round[ci];
            second_look(
                ctx,
                &c.items[idx],
                &c.pairs[idx],
                c.looks[idx].as_ref().expect("a look"),
            )
        }))
        .await;
        for (&(ci, idx), look) in second.iter().zip(seen) {
            if let Some(look) = look {
                round[ci].looks[idx] = Some(look);
            }
        }

        // 落地：按簇、按对顺序写库——连锁合并要按顺序
        for c in round.iter_mut() {
            for idx in 0..c.items.len() {
                let look = c.looks[idx]
                    .take()
                    .unwrap_or_else(|| Look::from_batch(None, 0.0, None));
                apply(ctx, &c.items[idx], &c.precedents[idx], look).await?;
            }
        }
        state.emit_review(kb_id);
    }
    Ok(!gov::queue(pool, kb_id, 1).await?.is_empty())
}

/// 一簇的攒批调用：人定过的不问，其余一次问完；回每对的看法
async fn batch_looks(ctx: &Ctx<'_>, c: &Cluster) -> anyhow::Result<Vec<(usize, Look)>> {
    let asked: Vec<usize> = (0..c.items.len())
        .filter(|&i| c.looks[i].is_none())
        .collect();
    if asked.is_empty() {
        return Ok(Vec::new());
    }
    let batch: Vec<utopia_extract::AdjudicationPair> =
        asked.iter().map(|&i| c.pairs[i].clone()).collect();
    let messages = utopia_extract::build_adjudication_messages(&batch);
    let reply = {
        let _permit = permit(ctx).await;
        ctx.client.chat(&messages).await?
    };
    let verdicts = utopia_extract::parse_adjudication(&reply)?;
    let by_i: HashMap<usize, &utopia_extract::AdjudicationVerdict> =
        verdicts.iter().map(|v| (v.i, v)).collect();
    Ok(asked
        .iter()
        .enumerate()
        .map(|(n, &idx)| {
            let look = match by_i.get(&n) {
                Some(v) => Look::from_batch(
                    match v.verdict.as_str() {
                        "same" => Some(true),
                        "different" => Some(false),
                        _ => None,
                    },
                    v.confidence.unwrap_or(0.5).clamp(0.0, 1.0),
                    v.why.clone(),
                ),
                None => Look::from_batch(None, 0.0, None),
            };
            (idx, look)
        })
        .collect())
}

async fn permit(ctx: &Ctx<'_>) -> Option<tokio::sync::OwnedSemaphorePermit> {
    match ctx.settings.as_ref() {
        Some(s) => llm_util::acquire_chat(ctx.state, s).await,
        None => None,
    }
}

fn pair_of(item: &ReviewItem, p: &Precedents) -> utopia_extract::AdjudicationPair {
    // 同名且大类不冲突的对：两侧写同一个类型标签。抽取器给同一家公司的两条记录
    // Store 与 Organization，模型就拿这个当「不同」的理由——把拐杖拿掉，让它看事实
    // 只在两侧都归得到同一个大类（人、组织、地点、事件）时才共用：Periodical 对 Service、
    // VideoGame 对没类型，那些标签是有信息的，留着
    let same_kind = gov::name_shape(&item.left.name, &item.right.name) == gov::NameShape::Identical
        && matches!(
            (
                item.left.type_label.as_deref().and_then(gov::type_family),
                item.right.type_label.as_deref().and_then(gov::type_family),
            ),
            (Some(a), Some(b)) if a == b
        );
    let shared = item
        .left
        .type_label
        .clone()
        .or_else(|| item.right.type_label.clone())
        .unwrap_or_else(|| "untyped".into());
    let side = |s: &utopia_core::models::ReviewSide| utopia_extract::AdjudicationSide {
        name: s.name.clone(),
        type_label: if same_kind {
            shared.clone()
        } else {
            s.type_label.clone().unwrap_or_else(|| "untyped".into())
        },
        facts: s.top_facts.clone(),
    };
    utopia_extract::AdjudicationPair {
        left: side(&item.left),
        right: side(&item.right),
        precedents: gov::render_lines(p),
    }
}

/// 第一层没定（没判决、置信度不到线），或者同名、大类不冲突、模型却说不同——那是它
/// 最爱错的一种：都让第二层带着全部事实与原文再看一遍。名字的形状让人起疑的合并（版本
/// 尾巴、含着名字的一长串）也再看：名字只能让人起疑，定不了是不是一个东西——「Lease
/// Agreement dated May 16, 2016, as amended」与「Lease Agreement」形状上是一句话，事实上
/// 是同一份租约。类型冲突这条硬规则拦下的不进：再看也不会改规则
fn wants_second_look(item: &ReviewItem, p: &Precedents, look: &Look) -> bool {
    let types_conflict = gov::types_conflict(
        item.left.type_label.as_deref(),
        item.right.type_label.as_deref(),
    );
    let shape = gov::name_shape(&item.left.name, &item.right.name);
    let doubted_split = shape == gov::NameShape::Identical
        && !types_conflict
        && look.same == Some(false)
        && look.calls == 0;
    let doubted_merge =
        name_doubts(shape) && !types_conflict && look.same == Some(true) && look.calls == 0;
    ((gov::gate(look.same, look.conf, types_conflict, shape, p) == Gate::Propose
        && look.uncertain())
        || doubted_split
        || doubted_merge)
        && p.reverts.is_empty()
}

/// 名字形状让人起疑的合并：版本尾巴，或含着另一个名字的一长串
fn name_doubts(shape: gov::NameShape) -> bool {
    matches!(shape, gov::NameShape::Version | gov::NameShape::Phrase)
}

/// 闸门看的名字形状。第二层带着两边的事实与原文看过、仍说是同一个的，形状的疑点已经由
/// 证据答过了，不再按形状拦——拦的只剩没看过证据的第一层
fn shape_for_gate(item: &ReviewItem, look: &Look) -> gov::NameShape {
    settled_shape(
        gov::name_shape(&item.left.name, &item.right.name),
        look.calls > 0,
    )
}

/// 形状的疑点在第二层读过证据之后就答完了
fn settled_shape(shape: gov::NameShape, evidence_read: bool) -> gov::NameShape {
    if evidence_read && name_doubts(shape) {
        gov::NameShape::Unrelated
    } else {
        shape
    }
}

/// 裁决器的入口（0028）：治理关着，攒批判不定的对也带工具再看一遍——同一个循环、
/// 同一份预算。每次调用一个 run_id：一次裁决任务就是一次 run
pub(crate) async fn look_again(
    state: &AppState,
    kb_id: Uuid,
    client: &LlmClient,
    settings: &Option<LlmSettings>,
    item: &ReviewItem,
    pair: &utopia_extract::AdjudicationPair,
    earlier: &Look,
) -> Option<Look> {
    let ctx = Ctx {
        state,
        kb_id,
        run_id: Uuid::now_v7(),
        client,
        settings,
    };
    second_look(&ctx, item, pair, earlier).await
}

/// 第二层看一对：预算够就看，看完的看法替掉第一层的；看不成（预算用完、模型出错）回 None，
/// 照第一层的看法办
async fn second_look(
    ctx: &Ctx<'_>,
    item: &ReviewItem,
    pair: &utopia_extract::AdjudicationPair,
    look: &Look,
) -> Option<Look> {
    let kb_id = ctx.kb_id;
    let spent = match gov::loop_calls_today(&ctx.state.pool, kb_id).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(%kb_id, error = %e, "治理：数循环预算失败");
            return None;
        }
    };
    if spent >= LOOP_DAILY_CALLS {
        tracing::info!(%kb_id, spent, "治理：今天的循环预算用完，判不定的只写建议");
        return None;
    }
    match investigate(ctx, item, pair, look).await {
        Ok(l) => Some(l),
        // 第二层失败不拖累第一层：这一对照第一层的看法写成建议
        Err(e) => {
            tracing::warn!(%kb_id, review = %item.id, error = %e, "治理：第二层没跑完");
            None
        }
    }
}

/// 一对的判决落地：过闸就动手并记 applied，否则转人工并记 proposed（带着问题与轨迹）
async fn apply(ctx: &Ctx<'_>, item: &ReviewItem, p: &Precedents, look: Look) -> anyhow::Result<()> {
    let pool = &ctx.state.pool;
    let kb_id = ctx.kb_id;
    let types_conflict = gov::types_conflict(
        item.left.type_label.as_deref(),
        item.right.type_label.as_deref(),
    );
    let shape = shape_for_gate(item, &look);

    let action = look.action();
    let precedents = gov::precedents_json(p);
    let conf = look.conf;
    let decision = |status: &'static str, merge_id: Option<Uuid>| NewDecision {
        run_id: ctx.run_id,
        target_id: item.id,
        action,
        confidence: conf,
        reason: look.why.as_deref(),
        precedents: precedents.clone(),
        status,
        merge_id,
        question: look.question.as_deref(),
        trace: Value::Array(look.trace.clone()),
        calls: look.calls,
    };

    match gov::gate(look.same, look.conf, types_conflict, shape, p) {
        Gate::Apply if look.same == Some(true) => {
            let reason = format!("governed|{conf:.2}");
            // 同簇连锁：前一对合完，这一对的一侧可能已经并进了别人——合活着的那个
            let (l, r) = (
                utopia_store::resolution::survivor(pool, kb_id, item.left.id).await?,
                utopia_store::resolution::survivor(pool, kb_id, item.right.id).await?,
            );
            if l == r {
                // 两边已经是同一个实体：只剩把审核行关上
                utopia_store::resolution::close_review_auto(pool, item.id, "merged", &reason)
                    .await?;
                let id = gov::record(pool, kb_id, decision("applied", None)).await?;
                audit(ctx, "review.merge", item, conf, id).await;
                return Ok(());
            }
            // 执行闸门（0027）：合并会立刻送出图外的东西——违规、派生、答案——留给人，
            // 把握再高也不动手。agent 的看法照记成建议，理由前面写明是闸门留下的
            let impact = execution_gate::impact_of(pool, kb_id, l, r).await?;
            if let Some(hold) = execution_gate::hold(&impact) {
                utopia_store::resolution::escalate_review(
                    pool,
                    item.id,
                    &format!("escalate_impact|{hold}"),
                )
                .await?;
                let held = match look.why.as_deref() {
                    Some(why) => format!("held for a person: {}; {why}", hold.explain()),
                    None => format!("held for a person: {}", hold.explain()),
                };
                gov::record(
                    pool,
                    kb_id,
                    NewDecision {
                        reason: Some(&held),
                        ..decision("proposed", None)
                    },
                )
                .await?;
                return Ok(());
            }
            let (target, source) = utopia_store::resolution::merge_direction(pool, l, r).await?;
            match utopia_store::resolution::merge_entities(
                pool, kb_id, source, target, None, &reason,
            )
            .await
            {
                Ok(merge_id) => {
                    utopia_store::resolution::close_review_auto(pool, item.id, "merged", &reason)
                        .await?;
                    let id = gov::record(pool, kb_id, decision("applied", Some(merge_id))).await?;
                    audit(ctx, "review.merge", item, conf, id).await;
                }
                // 同簇连锁合并已吞掉一方：留给人，并记一条 unsure 免得下一轮又撞上
                Err(AppError::Conflict(_)) | Err(AppError::NotFound) => {
                    utopia_store::resolution::escalate_review(
                        pool,
                        item.id,
                        "escalate_entity_changed",
                    )
                    .await?;
                    gov::record(
                        pool,
                        kb_id,
                        NewDecision {
                            action: "unsure",
                            reason: Some("one side changed while this cluster was being decided"),
                            ..decision("proposed", None)
                        },
                    )
                    .await?;
                }
                Err(e) => return Err(e.into()),
            }
        }
        Gate::Apply => {
            let reason = format!("governed|{conf:.2}");
            utopia_store::resolution::close_review_auto(pool, item.id, "kept", &reason).await?;
            let id = gov::record(pool, kb_id, decision("applied", None)).await?;
            audit(ctx, "review.keep", item, conf, id).await;
        }
        Gate::Propose => {
            utopia_store::resolution::escalate_review(pool, item.id, "proposed").await?;
            gov::record(pool, kb_id, decision("proposed", None)).await?;
        }
    }
    Ok(())
}

/// 第二层：带工具逐条再看。模型每回合要么查一样东西（查完把结果喂回去），要么
/// decide / defer 收尾。回合数封顶；模型不用工具收尾就提醒一次，再不收尾就当没定
async fn investigate(
    ctx: &Ctx<'_>,
    item: &ReviewItem,
    pair: &utopia_extract::AdjudicationPair,
    earlier: &Look,
) -> anyhow::Result<Look> {
    let tools = governor::tools();
    let mut messages = governor::messages(
        pair,
        &governor::EarlierLook {
            verdict: match earlier.same {
                Some(true) => "same",
                Some(false) => "different",
                None => "unsure",
            },
            confidence: earlier.conf,
            why: earlier.why.as_deref(),
        },
    );
    let mut trace: Vec<Value> = Vec::new();
    let mut calls = 0;
    let mut nudged = false;

    // 回合上限 = 查询次数 + 收尾那一次 + 一次提醒
    for _ in 0..(governor::MAX_STEPS + 2) {
        let turn = {
            let _permit = permit(ctx).await;
            ctx.client.chat_tools(&messages, &tools).await?
        };
        calls += 1;
        messages.push(turn.to_message());
        if turn.tool_calls.is_empty() {
            if nudged {
                break;
            }
            nudged = true;
            messages.push(json!({ "role": "user", "content": governor::NUDGE }));
            continue;
        }
        for call in &turn.tool_calls {
            match governor::read_step(&call.name, &call.arguments) {
                Step::Decide {
                    same,
                    confidence,
                    why,
                } => {
                    return Ok(Look {
                        same: Some(same),
                        conf: confidence,
                        why: Some(why).filter(|w| !w.is_empty()),
                        question: None,
                        trace,
                        calls,
                    });
                }
                Step::Defer { question } => {
                    return Ok(Look {
                        same: None,
                        conf: 0.0,
                        why: None,
                        question: Some(question),
                        trace,
                        calls,
                    });
                }
                Step::Lookup { tool, args } => {
                    let out = if trace.len() >= governor::MAX_STEPS {
                        "Lookup limit reached; finish with decide or defer.".to_string()
                    } else {
                        let (out, note) = lookup(ctx, item, &tool, &args).await?;
                        trace.push(json!({ "tool": tool, "args": args, "note": note }));
                        out
                    };
                    messages.push(tool_result_message(&call.id, &out));
                }
                Step::Unknown(problem) => {
                    messages.push(tool_result_message(&call.id, &problem));
                }
            }
        }
    }
    // 看了，没收尾：当没定，轨迹留下
    Ok(Look {
        same: None,
        conf: 0.0,
        why: Some("the agent looked but did not conclude".into()),
        question: None,
        trace,
        calls,
    })
}

/// 跑一个工具：给模型的文本，以及写进轨迹的一句话
async fn lookup(
    ctx: &Ctx<'_>,
    item: &ReviewItem,
    tool: &str,
    args: &Value,
) -> anyhow::Result<(String, String)> {
    let pool = &ctx.state.pool;
    let kb_id = ctx.kb_id;
    let side = |s: &str| {
        if s == "A" {
            &item.left
        } else {
            &item.right
        }
    };
    let clip = |s: String| {
        if s.chars().count() > TOOL_OUTPUT_CHARS {
            let mut t: String = s.chars().take(TOOL_OUTPUT_CHARS).collect();
            t.push_str("\n…");
            t
        } else {
            s
        }
    };
    let (out, note) = match tool {
        "facts" => {
            let s = args["side"].as_str().unwrap_or("A");
            let e = side(s);
            let lines = utopia_store::resolution::entity_fact_lines(pool, kb_id, e.id, 30).await?;
            let n = lines.len();
            let out = if lines.is_empty() {
                "(no recorded facts)".to_string()
            } else {
                lines.join("\n")
            };
            (out, format!("{n} facts of {s} \"{}\"", e.name))
        }
        "quotes" => {
            let s = args["side"].as_str().unwrap_or("A");
            let e = side(s);
            let rows = gov::quotes_of(pool, kb_id, e.id, 6).await?;
            let n = rows.len();
            let out = if rows.is_empty() {
                "(no source passages)".to_string()
            } else {
                rows.iter()
                    .map(|(doc, q)| format!("[{doc}] {q}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            (out, format!("{n} passages about {s} \"{}\"", e.name))
        }
        "ledger" => {
            let q = args["query"].as_str().unwrap_or("");
            let rows = gov::ledger_search(pool, kb_id, q, 10).await?;
            let n = rows.len();
            let out = if rows.is_empty() {
                "(no decisions by people about this name)".to_string()
            } else {
                rows.iter()
                    .map(|x| {
                        let verb = match x.action.as_str() {
                            "merge.revert" => "merge reverted",
                            "review.keep" => "kept apart",
                            _ => "merged",
                        };
                        // 人写的理由跟在后面：第二层去查台账，查到的该是「凭什么」
                        let because = x
                            .why
                            .as_deref()
                            .map(|w| format!("; they wrote: \"{w}\""))
                            .unwrap_or_default();
                        format!(
                            "\"{}\" ≟ \"{}\": {verb} by a person on {}{because}",
                            x.left,
                            x.right,
                            x.at.format("%Y-%m-%d")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            (out, format!("{n} decisions about \"{q}\""))
        }
        // 合并会牵动什么（0028）：模型先看到闸门（0027）会看到的东西，再决定是裁还是问
        "consequences" => {
            let impact =
                execution_gate::impact_of(pool, kb_id, item.left.id, item.right.id).await?;
            let families = if gov::types_conflict(
                item.left.type_label.as_deref(),
                item.right.type_label.as_deref(),
            ) {
                "\n- the two types belong to different families; the rules never merge across families"
            } else {
                ""
            };
            let out = format!("{}{families}", impact.describe());
            let held = execution_gate::hold(&impact)
                .map(|h| h.to_string())
                .unwrap_or_else(|| "nothing held".into());
            (out, format!("what a merge would touch: {held}"))
        }
        "namesakes" => {
            let q = args["query"].as_str().unwrap_or("");
            let rows = gov::namesakes(pool, kb_id, q, 10).await?;
            let n = rows.len();
            let out = if rows.is_empty() {
                "(no other entity with such a name)".to_string()
            } else {
                rows.iter()
                    .map(|x| {
                        format!(
                            "\"{}\" ({}) · {} facts{}",
                            x.name,
                            x.type_label.as_deref().unwrap_or("untyped"),
                            x.facts,
                            if x.merged {
                                " · merged into another"
                            } else {
                                ""
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            (out, format!("{n} entities named like \"{q}\""))
        }
        other => (
            format!("no tool named {other}"),
            format!("unknown tool {other}"),
        ),
    };
    Ok((clip(out), note))
}

/// 决策台账：actor 为空（机器），detail 里说是治理、置信度多少、对应哪条 agent 决定
async fn audit(ctx: &Ctx<'_>, action: &str, item: &ReviewItem, conf: f32, id: Uuid) {
    let _ = utopia_store::audit::record_opt(
        &ctx.state.pool,
        Some(ctx.kb_id),
        None,
        action,
        "review",
        Some(item.id),
        json!({
            "left": item.left.name, "right": item.right.name, "score": item.score,
            "confidence": conf, "via": "governor", "decision": id,
        }),
    )
    .await;
}

/// 保险丝（0025 决定 9）：这次打开以来、七天之内，人撤回 agent 的自动合并到了两次，
/// 开关自动关掉，发一条告警，台账记一笔。它替人做主做错了两回，该停下来等人再开；
/// 人再打开时 governance_since 更新，之前的撤回不再算
pub async fn fuse(state: &AppState, kb_id: Uuid) {
    let pool = &state.pool;
    let kb = match utopia_store::kbs::get(pool, kb_id).await {
        Ok(kb) if kb.governance => kb,
        _ => return,
    };
    let window = Utc::now() - Duration::days(gov::FUSE_WINDOW_DAYS);
    let since = kb.governance_since.map_or(window, |s| s.max(window));
    let reverts = match gov::reverts_since(pool, kb_id, since).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(%kb_id, error = %e, "治理：数撤回失败");
            return;
        }
    };
    if reverts < gov::FUSE_REVERTS {
        return;
    }
    match gov::trip(pool, kb_id).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!(%kb_id, error = %e, "治理：关开关失败");
            return;
        }
    }
    tracing::warn!(%kb_id, reverts, "治理：保险丝跳了，开关已关");
    if let Err(e) = alerts::raise(
        pool,
        alerts::NewAlert {
            kb_id: Some(kb_id),
            severity: "warning",
            kind: alerts::kind::GOVERNANCE_TRIPPED,
            min_role: Role::Editor,
            subject_type: Some("kb"),
            subject_id: Some(kb_id),
            detail: json!({
                "kb": kb.name, "reverts": reverts, "window_days": gov::FUSE_WINDOW_DAYS,
            }),
        },
    )
    .await
    {
        tracing::warn!(%kb_id, error = %e, "上报 governance.tripped 失败");
    }
    let _ = utopia_store::audit::record_opt(
        pool,
        Some(kb_id),
        None,
        "kb.updated",
        "kb",
        Some(kb_id),
        json!({ "governance": false, "via": "fuse", "reverts": reverts }),
    )
    .await;
    state.emit_review(kb_id);
}

/// 人从合并历史撤回了一条合并：是 agent 自己裁的就记成 reverted，然后看保险丝
pub async fn after_revert(state: &AppState, kb_id: Uuid, merge_id: Uuid, user_id: Uuid) {
    match gov::settle_by_merge(&state.pool, kb_id, merge_id, user_id).await {
        Ok(Some(_)) => fuse(state, kb_id).await,
        Ok(None) => {}
        Err(e) => tracing::warn!(%kb_id, merge = %merge_id, error = %e, "治理：记撤回失败"),
    }
}

/// 人裁了一对之后：这一对上开着的建议就是被回答了（与建议相同是接受，不同是改判）；
/// 同名或同实体的对上的建议过时、那些对回到队列；开关开着就排一轮。
/// 人批量分开三对张伟，agent 顺着同一簇把剩下的照办——#428 要的自动处理就是这一步。
/// `action` 为 None 时不答本对的建议（Agent 队列那条路自己已经答过了）
pub async fn after_human_decision(
    state: &AppState,
    kb_id: Uuid,
    review_ids: &[Uuid],
    action: Option<&str>,
    user_id: Uuid,
) {
    for &id in review_ids {
        if let Some(action) = action {
            if let Err(e) = gov::answer_open(&state.pool, kb_id, id, action, user_id).await {
                tracing::warn!(%kb_id, review = %id, error = %e, "治理：建议回答失败");
            }
        }
        if let Err(e) = gov::supersede_siblings(&state.pool, kb_id, id).await {
            tracing::warn!(%kb_id, review = %id, error = %e, "治理：建议作废失败");
        }
    }
    match utopia_store::kbs::get(&state.pool, kb_id).await {
        Ok(kb) if kb.governance => {
            if let Err(e) = utopia_store::jobs::enqueue_unless_queued(
                &state.pool,
                "govern",
                json!({ "kb_id": kb_id }),
            )
            .await
            {
                tracing::warn!(%kb_id, error = %e, "治理任务入队失败");
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_doubtful_shape_stops_counting_once_the_evidence_was_read() {
        let lease = gov::name_shape(
            "Lease Agreement dated May 16, 2016, as amended",
            "Lease Agreement",
        );
        assert!(name_doubts(lease), "{lease:?}");
        assert_eq!(settled_shape(lease, false), lease);
        assert_eq!(settled_shape(lease, true), gov::NameShape::Unrelated);

        let version = gov::name_shape("Claude Mythos 5", "Claude Mythos");
        assert!(name_doubts(version), "{version:?}");
        assert_eq!(settled_shape(version, false), version);

        // 形状本来就不起疑的，读没读过证据都照旧
        let same = gov::name_shape("OpenAI", "OpenAI");
        assert!(!name_doubts(same));
        assert_eq!(settled_shape(same, true), same);
    }
}
