//! 本体代理（0061 决定 2、3）：对齐判成「没有」「没定」的短语形状与类别词，对着现有本体和
//! 能力问题交给模型，让它提该建的类与属性。提案进 `ontology_proposals`，**人采纳才成为本体**
//! （0012）；采纳建出元素后排一次对齐，那些形状按 0053 的指纹自己重判。
//!
//! 形状的身份是短语加两端的类键加宾语是不是字面值——同 `phrase_bindings` 的键，只是用键
//! 而不是 id 存进提案，好让人读得懂

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use utopia_core::models::RelationAxioms;
use utopia_core::AppError;
use utopia_extract::ontology_agent as agent;
use utopia_store::agent_reviews::{self, Review};
use utopia_store::phrase_bindings::PhraseSignature;
use utopia_store::type_bindings::KindWordSignature;
use utopia_store::{competency_questions, phrase_bindings, type_bindings};
use uuid::Uuid;

use crate::extraction::chat_retrying_rate_limits_at;
use crate::llm_util;
use crate::state::AppState;

/// 给没问题的库一次提这么多问题
pub(crate) const QUESTIONS_PER_ROUND: usize = 10;
/// 提问题时看多少条说得最多的形状 / 类别词 / 实体
const TOP_SIGNATURES: usize = 40;
const TOP_KIND_WORDS: usize = 20;
const TOP_ENTITIES: usize = 20;

/// 一次调用交给模型的形状数（0044 决定 3 的成本教训：词表每批只带一次）
pub(crate) const SIGNATURES_PER_CALL: usize = 12;
/// 每批跟着的类别词至多这么多
pub(crate) const KIND_WORDS_PER_CALL: usize = 6;
/// 一轮至多问这么多形状，按陈述数从多到少；其余等下一轮。界面上点一次也只花这么多
pub(crate) const SIGNATURES_PER_ROUND: usize = 120;
/// 对齐收尾时没绑上的形状够这么多，才叫代理来看（少于这个数人自己在审核页就处理了）
pub(crate) const TRIGGER_UNBOUND: usize = 20;

/// 形状写进提案里的样子
fn shape_of(sig: &PhraseSignature) -> Value {
    json!({
        "phrase": sig.phrase,
        "subject": sig.subject_type_key,
        "object": sig.object_type_key,
        "value": sig.object_is_value,
    })
}

/// 任务入口：一个库同时只跑一份（同两种对齐，并行会把端点打出 502）
pub async fn propose(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    let pool = &state.pool;
    let kb = utopia_store::kbs::get(pool, kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot propose ontology"))?;
    // 提本体是判断题：按端点默认的强度想
    let client = llm_util::chat_client_thinking(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot propose ontology"))?;
    let mut guard = pool.acquire().await?;
    let locked: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_lock(hashtext('propose_ontology'), hashtext($1))",
    )
    .bind(kb_id.to_string())
    .fetch_one(&mut *guard)
    .await?;
    if !locked {
        tracing::info!(%kb_id, "本体代理已有一份在跑，这次跳过");
        return Ok(());
    }
    let result = propose_locked(state, kb_id, &settings, &client).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('propose_ontology'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *guard)
        .await;
    result
}

/// 一条合并后的提案：同一个键在几批里都被提了，形状、类别词、问题并起来，定义取第一次的
struct Merged {
    proposal: agent::Proposal,
    /// Some = 这是「本体里已经有」：relation | attribute | class，进 map_to。
    /// 方向记在每条形状上（同一个属性，"X wrote Y" 反着读、"Y's author X" 正着读）
    map_kind: Option<String>,
    shapes: Vec<Value>,
    phrases: Vec<String>,
    words: Vec<String>,
    quotes: Vec<String>,
    serves: Vec<Uuid>,
}

async fn propose_locked(
    state: &AppState,
    kb_id: Uuid,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
    let props = utopia_store::ontology::relation_type_views(pool, kb_id).await?;
    let class_key: HashMap<Uuid, &str> = classes.iter().map(|c| (c.id, c.key.as_str())).collect();
    let class_keys: HashSet<&str> = classes.iter().map(|c| c.key.as_str()).collect();
    let prop_keys: HashSet<&str> = props.iter().map(|p| p.key.as_str()).collect();
    let prop_kind: HashMap<&str, &str> = props
        .iter()
        .map(|p| (p.key.as_str(), p.kind.as_str()))
        .collect();
    // 词表的指纹：类键 + 属性键。代理看过没提、或答「已有」的形状，词表不变就不再送
    let basis = {
        let mut keys: Vec<&str> = class_keys.iter().copied().collect();
        keys.sort_unstable();
        let mut pkeys: Vec<&str> = prop_keys.iter().copied().collect();
        pkeys.sort_unstable();
        let digest = Sha256::digest(format!("{}##{}", keys.join(" "), pkeys.join(" ")).as_bytes());
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };

    // 已经提过的（不论表态与否）不再提：采纳的等对齐重判；拒绝的人已经说过不要；
    // 还开着的人还没看。看过没提的和答「已有」的，词表变了才再送（cut 1.1：第一次真跑
    // 120 条里 85 条没进提案，下一轮又排在最前）
    // 看过没提的形状记的是当时的陈述数（basis = "count:n"）：陈述数翻倍了才再送——多了一个
    // 属性不必再问代理，对齐自己会拿新属性去重判（0053 指纹），问代理只会每次采纳后把全部
    // 没提的形状再烧一遍。答「已有」的记的是词表指纹：词表变了再送
    let mut proposed_shapes: HashSet<String> = HashSet::new();
    let mut proposed_words: HashSet<String> = HashSet::new();
    let mut declined_shapes: HashMap<String, i64> = HashMap::new();
    let mut declined_words: HashMap<String, i64> = HashMap::new();
    for r in agent_reviews::list(pool, kb_id).await? {
        let word = r.shape.get("kind_word").and_then(Value::as_str);
        if r.outcome == "declined" {
            let seen_count = r
                .basis
                .strip_prefix("count:")
                .and_then(|n| n.parse::<i64>().ok())
                .unwrap_or(1);
            match (r.kind.as_str(), word) {
                ("phrase", _) => {
                    declined_shapes.insert(shape_key(&r.shape), seen_count);
                }
                (_, Some(w)) => {
                    declined_words.insert(w.to_string(), seen_count);
                }
                _ => {}
            }
            continue;
        }
        if r.outcome != "proposed" && r.basis != basis {
            continue;
        }
        match (r.kind.as_str(), word) {
            ("phrase", _) => {
                proposed_shapes.insert(shape_key(&r.shape));
            }
            (_, Some(w)) => {
                proposed_words.insert(w.to_string());
            }
            _ => {}
        }
    }
    for s in utopia_store::ontology::signatures_already_proposed(pool, kb_id).await? {
        for p in s
            .get("phrases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            proposed_shapes.insert(shape_key(p));
        }
        for w in s
            .get("kind_words")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(w) = w.as_str() {
                proposed_words.insert(w.to_string());
            }
        }
    }

    // 没绑上的形状：对齐判成「没有」或「没定」的。还没判的等对齐，不在这里猜
    let sigs = phrase_bindings::signatures(pool, kb_id).await?;
    let bindings: HashMap<_, _> = phrase_bindings::bindings(pool, kb_id)
        .await?
        .into_iter()
        .map(|b| (b.key(), b))
        .collect();
    let mut open: Vec<&PhraseSignature> = sigs
        .iter()
        .filter(|s| {
            matches!(
                bindings.get(&s.key()).map(|b| b.status.as_str()),
                Some("none" | "undecided")
            )
        })
        .filter(|s| {
            let key = shape_key(&shape_of(s));
            !proposed_shapes.contains(&key)
                && declined_shapes
                    .get(&key)
                    .is_none_or(|seen| s.count >= seen * 2)
        })
        .collect();
    open.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.phrase.cmp(&b.phrase)));
    open.truncate(SIGNATURES_PER_ROUND);

    // 没归类的类别词同理
    let words = type_bindings::signatures(pool, kb_id).await?;
    let word_bindings: HashMap<String, _> = type_bindings::bindings(pool, kb_id)
        .await?
        .into_iter()
        .map(|b| (b.kind_word.clone(), b))
        .collect();
    let mut open_words: Vec<&KindWordSignature> = words
        .iter()
        .filter(|w| {
            matches!(
                word_bindings.get(&w.kind_word).map(|b| b.status.as_str()),
                Some("none" | "undecided")
            )
        })
        .filter(|w| {
            !proposed_words.contains(&w.kind_word)
                && declined_words
                    .get(&w.kind_word)
                    .is_none_or(|seen| w.count >= seen * 2)
        })
        .collect();
    open_words.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.kind_word.cmp(&b.kind_word))
    });
    let calls = open
        .len()
        .div_ceil(SIGNATURES_PER_CALL)
        .max(open_words.len().div_ceil(KIND_WORDS_PER_CALL));
    if calls == 0 {
        tracing::info!(%kb_id, "本体代理：没有新的形状要看");
        return Ok(());
    }
    open_words.truncate(calls * KIND_WORDS_PER_CALL);

    // 一条问题都没有的库（决定 1）：先让代理提问题，人接受了下一轮才算数；这一轮照旧按说法提
    if competency_questions::list(pool, kb_id).await?.is_empty() {
        if let Err(e) = propose_questions_with(state, kb_id, settings, client).await {
            tracing::warn!(%kb_id, error = %e, "本体代理：提问题失败，这一轮不带问题");
        }
    }
    let questions = competency_questions::accepted(pool, kb_id).await?;
    let q_items: Vec<agent::QuestionItem<'_>> = questions
        .iter()
        .enumerate()
        .map(|(i, q)| agent::QuestionItem {
            id: i as i64,
            text: &q.question,
        })
        .collect();
    let q_ids: HashSet<i64> = (0..questions.len() as i64).collect();
    let keys_of = |ids: &[Uuid]| -> Vec<&str> {
        ids.iter()
            .filter_map(|id| class_key.get(id).copied())
            .collect()
    };
    let glossary = agent::Glossary {
        classes: classes
            .iter()
            .map(|c| (c.key.as_str(), c.label.as_str(), c.description.as_str()))
            .collect(),
        properties: props
            .iter()
            .map(|p| {
                (
                    p.key.as_str(),
                    p.label.as_str(),
                    p.kind.as_str(),
                    keys_of(&p.domains),
                    keys_of(&p.ranges),
                    p.description.as_str(),
                )
            })
            .collect(),
    };
    let sig_items: Vec<agent::OpenSignature<'_>> = open
        .iter()
        .enumerate()
        .map(|(i, s)| agent::OpenSignature {
            id: i as i64,
            phrase: &s.phrase,
            subject_class: s.subject_type_key.as_deref(),
            object_class: s.object_type_key.as_deref(),
            object_is_value: s.object_is_value,
            statement_count: s.count,
            examples: &s.examples,
            quotes: &s.quotes,
        })
        .collect();
    let word_items: Vec<agent::OpenKindWord<'_>> = open_words
        .iter()
        .enumerate()
        .map(|(i, w)| agent::OpenKindWord {
            id: i as i64,
            kind_word: &w.kind_word,
            count: w.count,
            examples: &w.examples,
            phrases: &w.phrases,
        })
        .collect();
    tracing::info!(%kb_id, shapes = open.len(), kind_words = open_words.len(), questions = questions.len(), calls, "本体代理开始");

    let mut merged: HashMap<String, Merged> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    // 这一轮看过的每条形状的去向（调用失败的批次不记，下一轮再送）
    let mut reviews: Vec<Review> = Vec::new();
    let (mut failed, mut malformed, mut skipped_existing) = (0usize, 0usize, 0usize);
    for b in 0..calls {
        let sig_batch = sig_items
            .get(b * SIGNATURES_PER_CALL..)
            .map(|rest| &rest[..rest.len().min(SIGNATURES_PER_CALL)])
            .unwrap_or(&[]);
        let word_batch = word_items
            .get(b * KIND_WORDS_PER_CALL..)
            .map(|rest| &rest[..rest.len().min(KIND_WORDS_PER_CALL)])
            .unwrap_or(&[]);
        let sig_ids: HashSet<i64> = sig_batch.iter().map(|s| s.id).collect();
        let word_ids: HashSet<i64> = word_batch.iter().map(|w| w.id).collect();
        let messages = agent::build_proposal_messages(sig_batch, word_batch, &glossary, &q_items);
        let reply =
            match chat_retrying_rate_limits_at(state, settings, client, &messages, Some(0.0)).await
            {
                Ok(r) => r,
                Err(e) => {
                    if utopia_core::error::is_terminal(&e) {
                        return Err(e);
                    }
                    failed += 1;
                    tracing::warn!(%kb_id, batch = b, error = %e, "本体代理：这一批调用失败");
                    continue;
                }
            };
        let parsed = match agent::parse_proposal_response(&reply.text, &sig_ids, &word_ids, &q_ids)
        {
            Ok(p) => p,
            Err(e) => {
                failed += 1;
                tracing::warn!(%kb_id, batch = b, error = %e, "本体代理：回复读不出");
                continue;
            }
        };
        malformed += parsed.malformed;
        let mut sig_outcome: HashMap<i64, (&str, String)> = HashMap::new();
        let mut word_outcome: HashMap<i64, (&str, String)> = HashMap::new();
        // 「本体里已经有」：存成 map_to 提案，人点「用已有的」就是一次人的绑定判定
        for e in &parsed.existing {
            let map_kind = if let Some(k) = prop_kind.get(e.key.as_str()) {
                (*k).to_string()
            } else if class_keys.contains(e.key.as_str()) {
                "class".to_string()
            } else {
                malformed += 1;
                continue;
            };
            let slot = format!("map_to:{}", e.key);
            let entry = merged.entry(slot.clone()).or_insert_with(|| {
                order.push(slot);
                Merged {
                    proposal: agent::Proposal {
                        kind: "existing".into(),
                        key: e.key.clone(),
                        label: e.key.clone(),
                        definition: String::new(),
                        value: false,
                        datatype: None,
                        domains: Vec::new(),
                        ranges: Vec::new(),
                        parents: Vec::new(),
                        signatures: Vec::new(),
                        kind_words: Vec::new(),
                        questions: Vec::new(),
                    },
                    map_kind: Some(map_kind),
                    shapes: Vec::new(),
                    phrases: Vec::new(),
                    words: Vec::new(),
                    quotes: Vec::new(),
                    serves: Vec::new(),
                }
            });
            if map_kind_is_class(entry) {
                // 类接的是类别词；形状指给类是答错了格，不记
                for id in &e.kind_words {
                    if let Some(w) = open_words.get(*id as usize) {
                        push_word(entry, w);
                        word_outcome.insert(*id, ("existing", e.key.clone()));
                    }
                }
            } else {
                for id in &e.signatures {
                    if let Some(sig) = open.get(*id as usize) {
                        push_shape(entry, sig, e.direction.as_deref());
                        sig_outcome.insert(*id, ("existing", e.key.clone()));
                    }
                }
            }
        }
        for p in parsed.proposals {
            // 模型被告知已有的不要提；还是提了就当它没说（对齐那边会把形状挂上去）
            let taken = match p.kind.as_str() {
                "class" => class_keys.contains(p.key.as_str()),
                _ => prop_keys.contains(p.key.as_str()),
            };
            if taken {
                skipped_existing += 1;
                continue;
            }
            let slot = format!("{}:{}", p.kind, p.key);
            let entry = merged.entry(slot.clone()).or_insert_with(|| {
                order.push(slot);
                Merged {
                    proposal: agent::Proposal {
                        signatures: Vec::new(),
                        kind_words: Vec::new(),
                        questions: Vec::new(),
                        ..p.clone()
                    },
                    map_kind: None,
                    shapes: Vec::new(),
                    phrases: Vec::new(),
                    words: Vec::new(),
                    quotes: Vec::new(),
                    serves: Vec::new(),
                }
            });
            let target = format!("{}:{}", section_of(&p), p.key);
            for id in p.signatures {
                let Some(s) = open.get(id as usize) else {
                    continue;
                };
                push_shape(entry, s, None);
                sig_outcome.insert(id, ("proposed", target.clone()));
            }
            for id in p.kind_words {
                let Some(w) = open_words.get(id as usize) else {
                    continue;
                };
                push_word(entry, w);
                word_outcome.insert(id, ("proposed", target.clone()));
            }
            for id in p.questions {
                let Some(q) = questions.get(id as usize) else {
                    continue;
                };
                if !entry.serves.contains(&q.id) {
                    entry.serves.push(q.id);
                }
            }
        }
        for si in sig_batch {
            let (outcome, target) = sig_outcome
                .get(&si.id)
                .map(|(o, t)| (*o, Some(t.clone())))
                .unwrap_or(("declined", None));
            reviews.push(Review {
                kind: "phrase".into(),
                shape: shape_of(open[si.id as usize]),
                outcome: outcome.into(),
                target,
                basis: if outcome == "declined" {
                    format!("count:{}", open[si.id as usize].count)
                } else {
                    basis.clone()
                },
            });
        }
        for wi in word_batch {
            let (outcome, target) = word_outcome
                .get(&wi.id)
                .map(|(o, t)| (*o, Some(t.clone())))
                .unwrap_or(("declined", None));
            reviews.push(Review {
                kind: "kind_word".into(),
                shape: json!({ "kind_word": wi.kind_word }),
                outcome: outcome.into(),
                target,
                basis: if outcome == "declined" {
                    format!("count:{}", wi.count)
                } else {
                    basis.clone()
                },
            });
        }
    }

    let items: Vec<utopia_store::ontology::AgentProposal> = order
        .iter()
        .filter_map(|slot| merged.get(slot))
        .filter(|m| !m.shapes.is_empty() || !m.words.is_empty())
        .map(|m| {
            let p = &m.proposal;
            if let Some(map_kind) = &m.map_kind {
                let label = if map_kind == "class" {
                    classes
                        .iter()
                        .find(|c| c.key == p.key)
                        .map(|c| c.label.as_str())
                } else {
                    props
                        .iter()
                        .find(|r| r.key == p.key)
                        .map(|r| r.label.as_str())
                };
                return utopia_store::ontology::AgentProposal {
                    section: "map_to".into(),
                    key: p.key.clone(),
                    payload: json!({
                        "key": p.key,
                        "label": label.unwrap_or(p.key.as_str()),
                        "kind": map_kind,
                        "forms": m.phrases,
                        "kind_words": m.words,
                        "examples": m.quotes,
                        "proposed_by": "agent",
                    }),
                    serves: Vec::new(),
                    signatures: json!({ "phrases": m.shapes, "kind_words": m.words }),
                };
            }
            let section = section_of(p);
            let mut payload = json!({
                "key": p.key,
                "label": p.label,
                "description": p.definition,
                "forms": m.phrases,
                "kind_words": m.words,
                "examples": m.quotes,
                "proposed_by": "agent",
            });
            match section {
                "entity_types" => {
                    payload["parents"] = json!(p.parents);
                }
                "attribute_types" => {
                    payload["datatype"] = json!(datatype_or_text(p.datatype.as_deref()));
                    payload["domains"] = json!(p.domains);
                }
                _ => {
                    payload["temporal"] = json!("state");
                    payload["domains"] = json!(p.domains);
                    payload["ranges"] = json!(p.ranges);
                }
            }
            utopia_store::ontology::AgentProposal {
                section: section.to_string(),
                key: p.key.clone(),
                payload,
                serves: m.serves.clone(),
                signatures: json!({ "phrases": m.shapes, "kind_words": m.words }),
            }
        })
        .collect();
    // 同一个目标上一轮已经答过一批形状：并起来，不是盖掉（第一次真跑第三轮盖掉了第二轮的）
    let mut items = items;
    for it in items.iter_mut().filter(|it| it.section == "map_to") {
        if let Some(prev) =
            utopia_store::ontology::open_proposal(pool, kb_id, "map_to", &it.key).await?
        {
            merge_map_to(it, &prev);
        }
    }
    utopia_store::ontology::save_agent_proposals(pool, kb_id, &items).await?;
    agent_reviews::record(pool, kb_id, &reviews).await?;
    tracing::info!(%kb_id, proposals = items.len(), failed, malformed, skipped_existing, "本体代理结束");
    if !items.is_empty() {
        state.emit_pending(kb_id);
    }
    Ok(())
}

fn section_of(p: &agent::Proposal) -> &'static str {
    match (p.kind.as_str(), p.value) {
        ("class", _) => "entity_types",
        (_, true) => "attribute_types",
        _ => "relation_types",
    }
}

fn map_kind_is_class(m: &Merged) -> bool {
    m.map_kind.as_deref() == Some("class")
}

/// 形状的身份写成一个键：短语 + 两端的类键 + 宾语是不是字面值（方向不算——
/// map_to 里的形状带方向，比身份时得去掉）
fn shape_key(v: &Value) -> String {
    json!({
        "phrase": v.get("phrase"),
        "subject": v.get("subject"),
        "object": v.get("object"),
        "value": v.get("value"),
    })
    .to_string()
}

/// 形状的身份：短语 + 两端的类键 + 宾语是不是字面值（方向不算）
fn same_shape(a: &Value, b: &Value) -> bool {
    ["phrase", "subject", "object", "value"]
        .iter()
        .all(|k| a.get(k) == b.get(k))
}

fn push_shape(entry: &mut Merged, s: &PhraseSignature, direction: Option<&str>) {
    let mut shape = shape_of(s);
    if let Some(d) = direction {
        shape["direction"] = json!(d);
    }
    if entry.shapes.iter().any(|x| same_shape(x, &shape)) {
        return;
    }
    entry.shapes.push(shape);
    if !entry.phrases.contains(&s.phrase) {
        entry.phrases.push(s.phrase.clone());
    }
    for q in s.quotes.iter().take(1) {
        if entry.quotes.len() < 3 && !entry.quotes.contains(q) {
            entry.quotes.push(q.clone());
        }
    }
}

fn push_word(entry: &mut Merged, w: &KindWordSignature) {
    if entry.words.contains(&w.kind_word) {
        return;
    }
    entry.words.push(w.kind_word.clone());
    for e in w.examples.iter().take(1) {
        if entry.quotes.len() < 3 && !entry.quotes.contains(e) {
            entry.quotes.push(e.clone());
        }
    }
}

/// 把上一轮开着的 map_to 行并进这一轮的：形状、说法、类别词、例句取并集，先前的在前
fn merge_map_to(
    it: &mut utopia_store::ontology::AgentProposal,
    prev: &utopia_store::ontology::StoredProposal,
) {
    let mut shapes: Vec<Value> = prev
        .signatures
        .get("phrases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for s in it
        .signatures
        .get("phrases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !shapes.iter().any(|x| same_shape(x, s)) {
            shapes.push(s.clone());
        }
    }
    let union = |field: &str, from_payload: bool| -> Vec<String> {
        let (a, b) = if from_payload {
            (prev.payload.get(field), it.payload.get(field))
        } else {
            (prev.signatures.get(field), it.signatures.get(field))
        };
        let mut out = strings(a);
        for x in strings(b) {
            if !out.contains(&x) {
                out.push(x);
            }
        }
        out
    };
    let words = union("kind_words", false);
    let forms = union("forms", true);
    let examples: Vec<String> = union("examples", true).into_iter().take(3).collect();
    it.payload["forms"] = json!(forms);
    it.payload["kind_words"] = json!(words);
    it.payload["examples"] = json!(examples);
    it.signatures = json!({ "phrases": shapes, "kind_words": words });
}

fn datatype_or_text(d: Option<&str>) -> &str {
    match d {
        Some(d @ ("text" | "number" | "date" | "bool")) => d,
        _ => "text",
    }
}

/// 采纳时人改过的几格。None = 照提案
#[derive(Debug, Default, Deserialize)]
pub struct AdoptEdits {
    pub label: Option<String>,
    pub description: Option<String>,
    pub datatype: Option<String>,
    pub domains: Option<Vec<String>>,
    pub ranges: Option<Vec<String>>,
    pub parents: Option<Vec<String>>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect()
}

/// 采纳一条代理提案（0061 决定 3）：建出元素，标记提案，排对齐让形状重判。
/// 返回新元素的 id
pub async fn adopt(
    state: &AppState,
    kb_id: Uuid,
    section: &str,
    key: &str,
    edits: AdoptEdits,
    actor: Uuid,
) -> Result<Uuid, AppError> {
    let pool = &state.pool;
    let Some(p) = utopia_store::ontology::open_proposal(pool, kb_id, section, key).await? else {
        return Err(AppError::NotFound);
    };
    // 人改过几格：决定 5 的第二个数要它
    let edited = edits.label.is_some()
        || edits.description.is_some()
        || edits.datatype.is_some()
        || edits.domains.is_some()
        || edits.ranges.is_some()
        || edits.parents.is_some();
    let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
    let by_key: HashMap<&str, Uuid> = classes.iter().map(|c| (c.key.as_str(), c.id)).collect();
    let resolve = |keys: Vec<String>| -> Result<Vec<Uuid>, AppError> {
        keys.iter()
            .map(|k| {
                by_key.get(k.as_str()).copied().ok_or_else(|| {
                    AppError::invalid("unknown_class", format!("class `{k}` does not exist"))
                })
            })
            .collect()
    };
    let label = edits
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| p.payload.get("label").and_then(Value::as_str))
        .unwrap_or(key)
        .to_string();
    let description = edits
        .description
        .as_deref()
        .or_else(|| p.payload.get("description").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let id = match section {
        // 「用已有的」：形状各写一条人的绑定判定（同审核页），类别词各归到那个类
        "map_to" => adopt_map_to(state, kb_id, &p, &classes).await?,
        "entity_types" => {
            let parents = resolve(
                edits
                    .parents
                    .unwrap_or_else(|| strings(p.payload.get("parents"))),
            )?;
            let id = utopia_store::ontology::create_entity_type(
                pool,
                kb_id,
                key,
                &label,
                utopia_store::palette::color_for_key(key),
                "circle",
                &parents,
                &description,
            )
            .await?;
            // 多了一个类：类别词里「没有」「没定」的要重判，短语对齐跟在它后面
            let _ = utopia_store::jobs::enqueue_unless_pending(
                pool,
                "align_types",
                json!({ "kb_id": kb_id }),
                std::time::Duration::from_secs(5),
            )
            .await;
            id
        }
        "relation_types" | "attribute_types" => {
            let kind = if section == "attribute_types" {
                "attribute"
            } else {
                "relation"
            };
            let domains = resolve(
                edits
                    .domains
                    .unwrap_or_else(|| strings(p.payload.get("domains"))),
            )?;
            let ranges = if kind == "relation" {
                resolve(
                    edits
                        .ranges
                        .unwrap_or_else(|| strings(p.payload.get("ranges"))),
                )?
            } else {
                Vec::new()
            };
            let datatype = if kind == "attribute" {
                Some(
                    datatype_or_text(
                        edits
                            .datatype
                            .as_deref()
                            .or_else(|| p.payload.get("datatype").and_then(Value::as_str)),
                    )
                    .to_string(),
                )
            } else {
                None
            };
            let id = utopia_store::ontology::create_relation_type(
                pool,
                kb_id,
                key,
                &label,
                "state",
                RelationAxioms::default(),
                &description,
                kind,
                &domains,
                &ranges,
                datatype.as_deref(),
                None,
            )
            .await?;
            // 多了一个属性：判成 none / undecided 的形状也许对得上了（0053 的指纹变了）
            let _ = utopia_store::jobs::enqueue_unless_pending(
                pool,
                "align_phrases",
                json!({ "kb_id": kb_id }),
                std::time::Duration::from_secs(5),
            )
            .await;
            id
        }
        _ => {
            return Err(AppError::invalid(
                "bad_section",
                "section 只能是 map_to、entity_types、relation_types 或 attribute_types",
            ))
        }
    };
    // 新元素要有向量，对齐的短名单才看得见它（候选多于短名单长度时按向量挑；没向量的
    // 属性永远进不了名单，形状的指纹就不变，采纳了也不重判——第一次真跑 bordered_by 就
    // 是这样）。补的只是缺的那几条，通常一次 embed 调用
    if section != "map_to" {
        if let Err(e) = crate::ontology_index::refresh(state, kb_id).await {
            tracing::warn!(%kb_id, error = %e, "采纳后本体向量没补上，对齐的短名单看不见新元素");
        }
    }
    utopia_store::ontology::decide_proposal(pool, kb_id, section, key, "adopted", actor).await?;
    if edited {
        utopia_store::ontology::mark_proposal_edited(pool, kb_id, section, key).await?;
    }
    let _ = utopia_store::audit::record(
        pool,
        Some(kb_id),
        actor,
        "ontology_proposal.adopted",
        section.trim_end_matches('s'),
        Some(id),
        json!({ "key": key, "label": label, "proposed_by": p.proposed_by, "serves": p.serves }),
    )
    .await;
    state.emit_graph(kb_id);
    Ok(id)
}

/// map_to 的采纳：目标是属性，提案里的每条形状写成人的绑定（属性 + 方向），投影跟着
/// 排（同审核页的 `decide_alignment_phrase`）；目标是类，每个类别词归到它。返回目标的 id
async fn adopt_map_to(
    state: &AppState,
    kb_id: Uuid,
    p: &utopia_store::ontology::StoredProposal,
    classes: &[utopia_core::models::EntityType],
) -> Result<Uuid, AppError> {
    let pool = &state.pool;
    let kind = p
        .payload
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("relation");
    if kind == "class" {
        let class = classes
            .iter()
            .find(|c| c.key == p.key)
            .map(|c| c.id)
            .ok_or_else(|| AppError::invalid("unknown_class", "no class with that key"))?;
        for w in strings(p.signatures.get("kind_words")) {
            type_bindings::decide_and_apply_human(
                pool,
                kb_id,
                &w,
                Some(class),
                &json!({ "person": p.key, "via": "ontology_agent" }),
            )
            .await?;
        }
        return Ok(class);
    }
    let property = utopia_store::ontology::relation_type_views(pool, kb_id)
        .await?
        .into_iter()
        .find(|r| r.key == p.key)
        .map(|r| r.id)
        .ok_or_else(|| AppError::invalid("unknown_property", "no property with that key"))?;
    let sigs = phrase_bindings::signatures(pool, kb_id).await?;
    for shape in p
        .signatures
        .get("phrases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        // 此刻已经没有陈述的形状跳过。方向是这条形状自己的
        let Some(sig) = sigs.iter().find(|s| same_shape(&shape_of(s), shape)) else {
            continue;
        };
        let direction = match shape.get("direction").and_then(Value::as_str) {
            Some("reverse") => "reverse",
            _ => "forward",
        };
        let votes = json!({ "person": { "property": p.key, "direction": direction, "via": "ontology_agent" } });
        phrase_bindings::decide_with_delivery(
            pool,
            kb_id,
            sig,
            phrase_bindings::Decision {
                relation_type_id: Some(property),
                direction: Some(direction),
                status: "bound",
                votes: &votes,
                decided_by: "person",
                basis: None,
            },
        )
        .await?;
    }
    state.emit_review(kb_id);
    Ok(property)
}

/// 任务入口：给库提问题（决定 1 的"没有问题的库不被卡住"）。界面上点、或本体代理发现库里
/// 一条问题都没有时走这里
pub async fn propose_questions(state: &AppState, kb_id: Uuid) -> anyhow::Result<usize> {
    let pool = &state.pool;
    let kb = utopia_store::kbs::get(pool, kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot propose questions"))?;
    let client = llm_util::chat_client_thinking(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot propose questions"))?;
    propose_questions_with(state, kb_id, &settings, &client).await
}

#[derive(sqlx::FromRow)]
struct EntityDegree {
    name: String,
    class: Option<String>,
    degree: i64,
}

async fn propose_questions_with(
    state: &AppState,
    kb_id: Uuid,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
) -> anyhow::Result<usize> {
    use utopia_extract::question_agent as qa;
    let pool = &state.pool;
    let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
    let props = utopia_store::ontology::relation_type_views(pool, kb_id).await?;
    let class_key: HashMap<Uuid, &str> = classes.iter().map(|c| (c.id, c.key.as_str())).collect();
    let prop_key: HashMap<Uuid, &str> = props.iter().map(|p| (p.id, p.key.as_str())).collect();
    let keys_of = |ids: &[Uuid]| -> Vec<&str> {
        ids.iter()
            .filter_map(|id| class_key.get(id).copied())
            .collect()
    };
    let glossary = agent::Glossary {
        classes: classes
            .iter()
            .map(|c| (c.key.as_str(), c.label.as_str(), c.description.as_str()))
            .collect(),
        properties: props
            .iter()
            .map(|p| {
                (
                    p.key.as_str(),
                    p.label.as_str(),
                    p.kind.as_str(),
                    keys_of(&p.domains),
                    keys_of(&p.ranges),
                    p.description.as_str(),
                )
            })
            .collect(),
    };
    // 说得最多的形状，带上对齐绑到的属性
    let mut sigs = phrase_bindings::signatures(pool, kb_id).await?;
    sigs.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.phrase.cmp(&b.phrase)));
    sigs.truncate(TOP_SIGNATURES);
    let bindings: HashMap<_, _> = phrase_bindings::bindings(pool, kb_id)
        .await?
        .into_iter()
        .map(|b| (b.key(), b))
        .collect();
    let top_sigs: Vec<qa::TopSignature<'_>> = sigs
        .iter()
        .map(|s| qa::TopSignature {
            phrase: &s.phrase,
            subject_class: s.subject_type_key.as_deref(),
            object_class: s.object_type_key.as_deref(),
            object_is_value: s.object_is_value,
            statement_count: s.count,
            property: bindings
                .get(&s.key())
                .filter(|b| b.status == "bound")
                .and_then(|b| b.relation_type_id)
                .and_then(|id| prop_key.get(&id).copied()),
            example: s.quotes.first().map(String::as_str),
        })
        .collect();
    let mut words = type_bindings::signatures(pool, kb_id).await?;
    words.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.kind_word.cmp(&b.kind_word))
    });
    words.truncate(TOP_KIND_WORDS);
    let top_words: Vec<qa::TopKindWord<'_>> = words
        .iter()
        .map(|w| qa::TopKindWord {
            kind_word: &w.kind_word,
            count: w.count,
        })
        .collect();
    let degrees: Vec<EntityDegree> = sqlx::query_as(
        "SELECT e.canonical_name AS name, t.key AS class, count(f.id) AS degree
           FROM entities e
           LEFT JOIN entity_types t ON t.id = e.type_id
           JOIN facts f ON f.kb_id = e.kb_id AND (f.subject_id = e.id OR f.object_id = e.id)
                        AND f.invalidated_at IS NULL
          WHERE e.kb_id = $1
          GROUP BY e.id, e.canonical_name, t.key
          ORDER BY count(f.id) DESC, e.canonical_name
          LIMIT $2",
    )
    .bind(kb_id)
    .bind(TOP_ENTITIES as i64)
    .fetch_all(pool)
    .await?;
    let top_entities: Vec<qa::TopEntity<'_>> = degrees
        .iter()
        .map(|d| qa::TopEntity {
            name: &d.name,
            class: d.class.as_deref(),
            degree: d.degree,
        })
        .collect();
    let have = competency_questions::list(pool, kb_id).await?;
    let existing: Vec<&str> = have
        .iter()
        .filter(|q| q.status != "rejected")
        .map(|q| q.question.as_str())
        .collect();
    if top_sigs.is_empty() && top_words.is_empty() && top_entities.is_empty() {
        tracing::info!(%kb_id, "提问题：库里还没有东西可问");
        return Ok(0);
    }
    let messages = qa::build_question_messages(
        &top_sigs,
        &top_words,
        &top_entities,
        &glossary,
        &existing,
        QUESTIONS_PER_ROUND,
    );
    let reply = chat_retrying_rate_limits_at(state, settings, client, &messages, Some(0.0)).await?;
    let class_keys: HashSet<&str> = classes.iter().map(|c| c.key.as_str()).collect();
    let prop_keys: HashSet<&str> = props.iter().map(|p| p.key.as_str()).collect();
    let (proposed, malformed) = qa::parse_question_response(&reply.text, &class_keys, &prop_keys)?;
    let mut written = 0usize;
    for q in proposed.iter().take(QUESTIONS_PER_ROUND) {
        if competency_questions::exists_text(pool, kb_id, &q.question).await? {
            continue;
        }
        let needs = json!({ "classes": q.classes, "properties": q.properties });
        competency_questions::create(
            pool,
            kb_id,
            competency_questions::NewQuestion {
                question: &q.question,
                expected_answer: None,
                needs: Some(&needs),
                origin: "agent",
                status: "proposed",
                created_by: None,
            },
        )
        .await?;
        written += 1;
    }
    tracing::info!(%kb_id, proposed = proposed.len(), written, malformed, "提问题结束");
    if written > 0 {
        state.emit_pending(kb_id);
    }
    Ok(written)
}

#[cfg(test)]
#[path = "ontology_agent_tests.rs"]
mod tests;
