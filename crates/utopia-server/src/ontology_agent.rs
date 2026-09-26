//! 本体代理（0061 决定 2、3）：对齐判成「没有」「没定」的短语形状与类别词，对着现有本体和
//! 能力问题交给模型，让它提该建的类与属性。提案进 `ontology_proposals`，**人采纳才成为本体**
//! （0012）；采纳建出元素后排一次对齐，那些形状按 0053 的指纹自己重判。
//!
//! 形状的身份是短语加两端的类键加宾语是不是字面值——同 `phrase_bindings` 的键，只是用键
//! 而不是 id 存进提案，好让人读得懂

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde_json::{json, Value};
use utopia_core::models::RelationAxioms;
use utopia_core::AppError;
use utopia_extract::ontology_agent as agent;
use utopia_store::phrase_bindings::PhraseSignature;
use utopia_store::type_bindings::KindWordSignature;
use utopia_store::{competency_questions, phrase_bindings, type_bindings};
use uuid::Uuid;

use crate::extraction::chat_retrying_rate_limits_at;
use crate::llm_util;
use crate::state::AppState;

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

    // 已经提过的（不论表态与否）不再提：采纳的等对齐重判；拒绝的人已经说过不要；
    // 还开着的人还没看
    let mut proposed_shapes: HashSet<String> = HashSet::new();
    let mut proposed_words: HashSet<String> = HashSet::new();
    for s in utopia_store::ontology::signatures_already_proposed(pool, kb_id).await? {
        for p in s
            .get("phrases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            proposed_shapes.insert(p.to_string());
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
        .filter(|s| !proposed_shapes.contains(&shape_of(s).to_string()))
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
        .filter(|w| !proposed_words.contains(&w.kind_word))
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
                    shapes: Vec::new(),
                    phrases: Vec::new(),
                    words: Vec::new(),
                    quotes: Vec::new(),
                    serves: Vec::new(),
                }
            });
            for id in p.signatures {
                let Some(s) = open.get(id as usize) else {
                    continue;
                };
                let shape = shape_of(s);
                if entry.shapes.contains(&shape) {
                    continue;
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
            for id in p.kind_words {
                let Some(w) = open_words.get(id as usize) else {
                    continue;
                };
                if !entry.words.contains(&w.kind_word) {
                    entry.words.push(w.kind_word.clone());
                    for e in w.examples.iter().take(1) {
                        if entry.quotes.len() < 3 && !entry.quotes.contains(e) {
                            entry.quotes.push(e.clone());
                        }
                    }
                }
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
    }

    let items: Vec<utopia_store::ontology::AgentProposal> = order
        .iter()
        .filter_map(|slot| merged.get(slot))
        .filter(|m| !m.shapes.is_empty() || !m.words.is_empty())
        .map(|m| {
            let p = &m.proposal;
            let section = match (p.kind.as_str(), p.value) {
                ("class", _) => "entity_types",
                (_, true) => "attribute_types",
                _ => "relation_types",
            };
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
    utopia_store::ontology::save_agent_proposals(pool, kb_id, &items).await?;
    tracing::info!(%kb_id, proposals = items.len(), failed, malformed, skipped_existing, "本体代理结束");
    if !items.is_empty() {
        state.emit_pending(kb_id);
    }
    Ok(())
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
                "section 只能是 entity_types、relation_types 或 attribute_types",
            ))
        }
    };
    utopia_store::ontology::decide_proposal(pool, kb_id, section, key, "adopted", actor).await?;
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

#[cfg(test)]
#[path = "ontology_agent_tests.rs"]
mod tests;
