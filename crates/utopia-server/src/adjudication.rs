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
    utopia_core::review_reasons::similarity_proposed(item.reason.as_deref())
}

fn needs_second_look(
    item: &ReviewItem,
    p: &gov::Precedents,
    same: Option<bool>,
    conf: f32,
) -> bool {
    wants_another_look(item, p, same, conf) || !batch_verdict_may_apply(item, same)
}

/// 攒批那一眼的看法能不能不经第二眼就落地。名字向量提的对说 same 不能：第二眼没跑成
/// （预算用完、模型出错）就上交给人，不照攒批的看法合，也不把那个看法记进缓存——记了
/// 之后这一对每次再来都从缓存直接合，第二眼永远轮不到（#889 评审）
pub(crate) fn batch_verdict_may_apply(item: &ReviewItem, same: Option<bool>) -> bool {
    !(similarity_proposed(item) && same == Some(true))
}

/// 第二眼没跑成时上交的理由
pub(crate) const SECOND_LOOK_UNAVAILABLE: &str = "escalate_unsure|second_look_unavailable";

pub(crate) const NO_IDENTITY_EVIDENCE: &str = "kept_apart|no_checkable_identity_evidence";

pub(crate) fn no_identity_reason(item: &ReviewItem) -> String {
    match item.reason.as_deref() {
        Some(reason)
            if item.stage == "human"
                && reason.starts_with("namesake_tie|")
                && !reason.contains("|redirected") =>
        {
            // Left is the unresolved mention, not an identified namesake. Keeping
            // it apart closes review work without inventing an identity for it.
            format!("{NO_IDENTITY_EVIDENCE}|unresolved_namesake_left|{reason}")
        }
        _ => NO_IDENTITY_EVIDENCE.into(),
    }
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
    let evidence_held = matches!(outcome, Outcome::Kept) && look.same == Some(true);
    let held_reason = evidence_held.then(|| no_identity_reason(item));
    let action = match look.same {
        Some(true) if evidence_held => "keep",
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
            reason: held_reason.as_deref().or(look.why.as_deref()),
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

/// 一批的名字：第一对的两个名字。日志里认得出是哪一批就够了，整批的名字太长
pub(crate) fn batch_name(pairs: &[utopia_extract::AdjudicationPair]) -> String {
    match pairs.first() {
        Some(p) => format!("{} / {}", p.left.name, p.right.name),
        None => "(empty)".into(),
    }
}

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
                    proposed_because: utopia_extract::proposed_because(
                        utopia_core::review_reasons::name_vector_cosine(item.reason.as_deref()),
                    ),
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
        let (verdicts, repaired) = utopia_extract::parse_adjudication_repairing(&reply)?;
        if repaired {
            // 模型在引号里的理由中间直接换行：修补后照读，但记下是哪一批（#894/#895 同款）
            tracing::warn!(
                %kb_id,
                pairs = pairs.len(),
                first_pair = %batch_name(&pairs),
                "裁决回复的字符串里有裸控制字符，转义后才解开"
            );
        }
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
                        // 第二眼没跑成：相似提的 same 不能照攒批的看法合，也不进缓存
                        if !batch_verdict_may_apply(item, same) {
                            utopia_store::resolution::escalate_review(
                                &state.pool,
                                item.id,
                                SECOND_LOOK_UNAVAILABLE,
                            )
                            .await?;
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
    let evidence = if same == Some(true) && conf >= AUTO_CONF && item.left.name == item.right.name {
        match utopia_store::resolution::namesake_identity_evidence(
            &state.pool,
            kb_id,
            item.left.id,
            item.right.id,
        )
        .await?
        {
            Some(proof) => Some(proof),
            None => {
                // Keeping is reversible and does not turn one missing identity
                // link into another item in the human queue (#1193).
                let reason = no_identity_reason(item);
                utopia_store::resolution::close_review_auto(&state.pool, item.id, "kept", &reason)
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
                        "why": why, "model_same": true, "reason": reason,
                    }),
                )
                .await;
                return Ok(Outcome::Kept);
            }
        }
    } else {
        None
    };
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
            let mut reason = format!("auto_merged|{via} {conf:.2}");
            if let Some(proof) = evidence {
                reason.push('|');
                reason.push_str(&proof);
            }
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

    /// 第二眼没跑成时：相似提的 same 不落地也不进缓存；其余照攒批的看法办
    #[test]
    fn a_similarity_proposed_same_never_applies_on_the_batch_verdict_alone() {
        assert!(!batch_verdict_may_apply(
            &item("name_vector|0.78"),
            Some(true)
        ));
        assert!(batch_verdict_may_apply(
            &item("name_vector|0.78"),
            Some(false)
        ));
        assert!(batch_verdict_may_apply(&item("name_vector|0.78"), None));
        assert!(batch_verdict_may_apply(
            &item("ambiguous_name|0.41"),
            Some(true)
        ));
        assert!(batch_verdict_may_apply(
            &item("shared_name|张伟"),
            Some(true)
        ));
    }

    /// 同名对的证据闸门不增加第二眼调用；不到线的仍按原来的路径再看。
    #[test]
    fn a_same_name_pair_does_not_add_a_second_look() {
        let p = gov::Precedents::default();
        let it = item("ambiguous_name|0.41");
        assert!(!needs_second_look(&it, &p, Some(true), 0.9));
        assert!(needs_second_look(&it, &p, Some(true), 0.6), "不到线才再看");
    }

    struct EvidenceFixture {
        state: AppState,
        org: Uuid,
        kb: Uuid,
        workspace: Uuid,
        relation: Uuid,
        facts: [Uuid; 2],
        chunks: [Uuid; 2],
        _dir: tempfile::TempDir,
    }

    async fn evidence_fixture() -> anyhow::Result<Option<EvidenceFixture>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        let (org, workspace, kb, relation) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let (left, right, doc) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let facts = [Uuid::now_v7(), Uuid::now_v7()];
        let chunks = [Uuid::now_v7(), Uuid::now_v7()];
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','identity-evidence');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{workspace}','{org}','identity-evidence');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{workspace}','identity-evidence');
             INSERT INTO entities(id,kb_id,canonical_name) VALUES ('{left}','{kb}','张伟'),('{right}','{kb}','张伟');
             INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','identity.txt','{doc}');
             INSERT INTO relation_types(id,kb_id,key,label,kind,datatype,temporal,inverse_functional)
                 VALUES ('{relation}','{kb}','identifier','identifier','attribute','text','eternal',TRUE);"
        )).execute(&pool).await?;
        for (seq, subject) in [left, right].iter().enumerate() {
            sqlx::raw_sql(&format!(
                "INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_value)
                     VALUES ('{fact}','{kb}','{subject}','{relation}','{{\"value\":\"ID-7\"}}');
                 INSERT INTO chunks(id,kb_id,document_id,seq,text)
                     VALUES ('{chunk}','{kb}','{doc}',{seq},'张伟的终身编号是 ID-7。');
                 INSERT INTO fact_evidence(fact_id,chunk_id,document_id,doc_version,quote)
                     VALUES ('{fact}','{chunk}','{doc}',1,'张伟的终身编号是 ID-7。');",
                fact = facts[seq],
                chunk = chunks[seq],
            ))
            .execute(&pool)
            .await?;
        }
        utopia_store::resolution::create_review(
            &pool,
            kb,
            left,
            right,
            0.85,
            "ambiguous_name|0.85",
            utopia_store::resolution::ReviewStage::Adjudicating,
        )
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        Ok(Some(EvidenceFixture {
            state: AppState::new(pool, &cfg, search, "test-only".into()),
            org,
            kb,
            workspace,
            relation,
            facts,
            chunks,
            _dir: dir,
        }))
    }

    impl EvidenceFixture {
        async fn item(&self) -> anyhow::Result<ReviewItem> {
            Ok(
                utopia_store::resolution::pending_adjudications(&self.state.pool, self.kb, 1)
                    .await?
                    .remove(0),
            )
        }
        async fn finish(self, result: anyhow::Result<()>) -> anyhow::Result<()> {
            sqlx::query("DELETE FROM organizations WHERE id=$1")
                .bind(self.org)
                .execute(&self.state.pool)
                .await?;
            result
        }
    }

    /// Known true duplicates with only a common employer are one intentional lost
    /// merge; timeless unique identifiers preserve the supported true duplicate.
    #[tokio::test]
    async fn namesake_evidence_keeps_employer_only_apart_but_preserves_unique_identity(
    ) -> anyhow::Result<()> {
        for unique in [false, true] {
            let Some(f) = evidence_fixture().await? else {
                return Ok(());
            };
            let result = async {
                if !unique {
                    let employer = Uuid::now_v7();
                    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Acme')").bind(employer).bind(f.kb).execute(&f.state.pool).await?;
                    sqlx::query("UPDATE relation_types SET key='works_for', label='employer', kind='relation', datatype=NULL, temporal='state', inverse_functional=FALSE WHERE id=$1").bind(f.relation).execute(&f.state.pool).await?;
                    sqlx::query("UPDATE facts SET object_value=NULL, object_id=$2 WHERE id=ANY($1)").bind(f.facts.as_slice()).bind(employer).execute(&f.state.pool).await?;
                    sqlx::query("UPDATE chunks SET text='张伟任职于 Acme。' WHERE id=ANY($1)").bind(f.chunks.as_slice()).execute(&f.state.pool).await?;
                    sqlx::query("UPDATE fact_evidence SET quote='张伟任职于 Acme。' WHERE fact_id=ANY($1)").bind(f.facts.as_slice()).execute(&f.state.pool).await?;
                }
                let mut it = f.item().await?;
                if !unique {
                    it.stage = "human".into();
                    it.reason = Some("namesake_tie|0.85".into());
                    sqlx::query("UPDATE resolution_reviews SET stage='human', reason='namesake_tie|0.85' WHERE id=$1").bind(it.id).execute(&f.state.pool).await?;
                }
                let outcome = apply_verdict(&f.state, f.kb, &it, Some(true), 0.99, "adjudicated", Some("nothing contradicts")).await?;
                assert_eq!(matches!(outcome, Outcome::Merged(_)), unique);
                let (status, reason, human): (String, String, i64) = sqlx::query_as("SELECT status, reason, (SELECT count(*) FROM resolution_reviews WHERE kb_id=$2 AND status='pending' AND stage='human') FROM resolution_reviews WHERE id=$1").bind(it.id).bind(f.kb).fetch_one(&f.state.pool).await?;
                assert_eq!(human, 0);
                if unique { assert_eq!(status, "merged"); assert!(reason.contains("identity_evidence|facts=")); }
                else {
                    assert_eq!(status, "kept"); assert_eq!(reason, format!("{NO_IDENTITY_EVIDENCE}|unresolved_namesake_left|namesake_tie|0.85"));
                    let detail: serde_json::Value = sqlx::query_scalar("SELECT detail FROM audit_events WHERE target_id=$1 AND action='review.keep'").bind(it.id).fetch_one(&f.state.pool).await?;
                    assert_eq!(detail["reason"], reason);
                    assert_eq!(detail["model_same"], true);
                    let look = Look::from_batch(Some(true), 0.99, Some("nothing contradicts".into()));
                    record_look(&f.state, f.kb, Uuid::now_v7(), &it, &gov::Precedents::default(), &look, &outcome).await?;
                    let row: (String,String,String) = sqlx::query_as("SELECT action,status,reason FROM agent_decisions WHERE target_id=$1").bind(it.id).fetch_one(&f.state.pool).await?;
                    assert_eq!(row, ("keep".into(), "applied".into(), no_identity_reason(&it)));
                }
                Ok(())
            }.await;
            f.finish(result).await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn namesake_cached_same_rechecks_invalidated_evidence() -> anyhow::Result<()> {
        let Some(f) = evidence_fixture().await? else {
            return Ok(());
        };
        let result = async {
            let it = f.item().await?;
            assert!(utopia_store::resolution::namesake_identity_evidence(
                &f.state.pool,
                f.kb,
                it.left.id,
                it.right.id
            )
            .await?
            .is_some());
            let p = gov::precedents_for(&f.state.pool, f.kb, &it).await?;
            utopia_store::resolution::put_verdict(
                &f.state.pool,
                f.kb,
                &pair_key(&it, &gov::render_lines(&p)),
                Some(true),
                0.99,
                "test-only",
            )
            .await?;
            sqlx::query("UPDATE chunks SET superseded_at=now() WHERE id=$1")
                .bind(f.chunks[1])
                .execute(&f.state.pool)
                .await?;
            // The dead endpoint makes an accidental extra model call fail the test.
            utopia_store::settings::upsert(
                &f.state.pool,
                f.workspace,
                Some("http://127.0.0.1:1/v1"),
                Some("test-only"),
                Some("test-only"),
                None,
                None,
                None,
                None,
            )
            .await?;
            adjudicate_entities(&f.state, f.kb).await?;
            let status: String =
                sqlx::query_scalar("SELECT status FROM resolution_reviews WHERE id=$1")
                    .bind(it.id)
                    .fetch_one(&f.state.pool)
                    .await?;
            assert_eq!(status, "kept");
            Ok(())
        }
        .await;
        f.finish(result).await
    }

    #[tokio::test]
    async fn namesake_incoming_unique_identity_still_merges_and_rolls_back() -> anyhow::Result<()> {
        let Some(f) = evidence_fixture().await? else {
            return Ok(());
        };
        let result = async {
            let it = f.item().await?;
            let identifier = Uuid::now_v7();
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'ID-7')").bind(identifier).bind(f.kb).execute(&f.state.pool).await?;
            sqlx::query("UPDATE relation_types SET kind='relation', datatype=NULL, functional=TRUE, inverse_functional=FALSE WHERE id=$1").bind(f.relation).execute(&f.state.pool).await?;
            for (fact, person) in f.facts.iter().zip([it.left.id, it.right.id]) {
                sqlx::query("UPDATE facts SET subject_id=$2, object_id=$3, object_value=NULL WHERE id=$1").bind(fact).bind(identifier).bind(person).execute(&f.state.pool).await?;
            }
            sqlx::query("UPDATE chunks SET text='终身编号 ID-7 的持有人是张伟。' WHERE id=ANY($1)").bind(f.chunks.as_slice()).execute(&f.state.pool).await?;
            sqlx::query("UPDATE fact_evidence SET quote='终身编号 ID-7 的持有人是张伟。' WHERE fact_id=ANY($1)").bind(f.facts.as_slice()).execute(&f.state.pool).await?;
            let outcome = apply_verdict(&f.state, f.kb, &it, Some(true), 0.99, "investigated", None).await?;
            let Outcome::Merged(merge) = outcome else { anyhow::bail!("timeless unique incoming link did not merge") };
            utopia_store::resolution::revert_merge(&f.state.pool, f.kb, merge).await?;
            assert!(utopia_store::resolution::namesake_identity_evidence(&f.state.pool, f.kb, it.left.id, it.right.id).await?.is_some());
            Ok(())
        }.await;
        f.finish(result).await
    }

    #[tokio::test]
    async fn namesake_evidence_rejects_stale_unrelated_and_missing_quotes() -> anyhow::Result<()> {
        let Some(f) = evidence_fixture().await? else {
            return Ok(());
        };
        let result = async {
            let it = f.item().await?;
            for change in [
                "UPDATE facts SET valid_to_precision='unknown', attested_to=now() WHERE id=ANY($1)",
                "UPDATE facts SET object_value='{\"value\":null}' WHERE id=ANY($1)",
                "UPDATE facts SET object_value='{\"value\":\" \"}' WHERE id=ANY($1)",
                "UPDATE facts SET invalidated_at=now() WHERE id=ANY($1)",
            ] {
                sqlx::query(change).bind(f.facts.as_slice()).execute(&f.state.pool).await?;
                assert!(utopia_store::resolution::namesake_identity_evidence(&f.state.pool, f.kb, it.left.id, it.right.id).await?.is_none());
                sqlx::query("UPDATE facts SET valid_to_precision=NULL, attested_to=NULL, invalidated_at=NULL, object_value='{\"value\":\"ID-7\"}' WHERE id=ANY($1)").bind(f.facts.as_slice()).execute(&f.state.pool).await?;
            }
            for change in [
                "UPDATE fact_evidence SET quote=NULL WHERE fact_id=$1",
                "UPDATE fact_evidence SET quote='not in this source' WHERE fact_id=$1",
                "UPDATE fact_evidence SET quote='张伟的终身编号是 ID-7。', document_id=NULL WHERE fact_id=$1",
            ] {
                sqlx::query(change).bind(f.facts[1]).execute(&f.state.pool).await?;
                assert!(utopia_store::resolution::namesake_identity_evidence(&f.state.pool, f.kb, it.left.id, it.right.id).await?.is_none());
            }
            sqlx::query("UPDATE fact_evidence SET document_id=(SELECT document_id FROM chunks WHERE id=chunk_id), quote='张伟的终身编号是 ID-7。' WHERE fact_id=$1").bind(f.facts[1]).execute(&f.state.pool).await?;
            sqlx::query("UPDATE chunks SET superseded_at=now() WHERE id=$1").bind(f.chunks[1]).execute(&f.state.pool).await?;
            assert!(utopia_store::resolution::namesake_identity_evidence(&f.state.pool, f.kb, it.left.id, it.right.id).await?.is_none());
            Ok(())
        }.await;
        f.finish(result).await
    }
}
