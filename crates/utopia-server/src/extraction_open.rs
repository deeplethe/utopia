//! 开放图谱的写入路径（0044 第 1 刀，#729）：文档说了什么，就按它自己的话记下来。
//!
//! 和 `extraction::run` 的分别只有一处：**提示词里没有本体**，模型不选关系、不选类、
//! 不算日期。回复里是块自己的句子（`q`）、它提到的东西（`e`，有名字的和只被描述的）、
//! 它做的陈述（`s`，关系短语照抄）、它写的时间词（`t`）和别名（`n`）。落库时陈述成
//! `layer = 'open'` 的事实行，短语留在行上；限定按文档自己的角色词挂在
//! `statement_qualifiers`；时间词原样进 `time_mentions`，谁也不把它算成日期——那是
//! 0045 的事。类型化的事实由对齐（第 2 刀）从这些行算出来，不在这里写。
//!
//! 复用的是身份那一段：名字照旧走 `resolve_handle`（同一回复里两个同名的东西不会
//! 塌成一个，0041 的名字事实照记），被描述的东西建成没有名字事实的实体
//! （`create_described`），免得「一家医院」成了召回的桥。
//!
//! 两根时间轴（0022 / #714）：`attested_from` 永远是此刻；`attested_at` 只在文档日期
//! 来自内容或来源系统时才用它——上传时刻与文件修改时间都不是文档说的日期。

use crate::extraction::{
    chat_retrying_rate_limits, drop_signal, incomplete_reason, origin_ceiling, resolve_handle,
    span_in_quote,
};
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
use utopia_core::models::{Document, KnowledgeBase, LlmSettings};
use utopia_extract::open::{OpenTime, QualifierValue, Ref};
use utopia_store::extraction_drops::reason;
use utopia_store::graph::FactObject;
use uuid::Uuid;

/// 文档日期只在它来自内容或来源系统时才算证据日期（与 `temporal::DATED_AT` 同一口径）。
fn dated_at(doc: &Document) -> Option<chrono::DateTime<chrono::Utc>> {
    if matches!(doc.doc_time_source.as_str(), "content" | "source") {
        doc.doc_time
    } else {
        None
    }
}

/// 一段字在块里的**字符**偏移（起、止）。字符不是字节：界面和 SQL 的 `substr` 都按字符数，
/// 中文一个字三个字节，按字节存的偏移到界面上就错位。找不到原样的就 `None`——
/// 偏移只能由服务端从原文算出来，模型报的数字不算数
fn locate(hay: &str, needle: &str) -> Option<(i32, i32)> {
    let needle = needle.trim();
    if needle.is_empty() {
        return None;
    }
    let byte = hay.find(needle)?;
    let start = hay[..byte].chars().count();
    let end = start + needle.chars().count();
    Some((start as i32, end as i32))
}

/// 时间词在块里的字符起点：先在它所属的那句引文里找（同一个词块里可能出现两次，
/// 要的是这句里的那个），引文没定位到就在整块里找
fn locate_time(chunk: &str, quote: Option<(&str, Option<(i32, i32)>)>, words: &str) -> Option<i32> {
    if let Some((q, Some((start, _)))) = quote {
        if let Some((inner, _)) = locate(q, words) {
            return Some(start + inner);
        }
    }
    locate(chunk, words).map(|(s, _)| s)
}

/// 已知句柄 `k3` → 本文档已认下的第 3 个实体。
fn known_index(handle: &str) -> Option<usize> {
    handle
        .strip_prefix('k')
        .and_then(|n| n.parse::<usize>().ok())
        .and_then(|n| n.checked_sub(1))
}

pub(crate) async fn run_open(
    state: &AppState,
    doc: &Document,
    kb: &KnowledgeBase,
    settings: &LlmSettings,
    client: &utopia_llm::LlmClient,
    my_epoch: i32,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let document_id = doc.id;
    let kb_id = kb.id;
    // 本轮从头讲一遍这篇文档的故事，旧信号先清掉（重抽自动作数）
    let _ = utopia_store::extraction_drops::clear_for_document(pool, document_id).await;
    let attested_at = dated_at(doc);
    let chunks = utopia_store::documents::chunks_for_extraction(pool, document_id).await?;
    let opening_chunk = utopia_store::documents::opening_chunk(pool, document_id).await?;

    // 本文档已认下的**有名字的**实体，按首次出现排序，送进后续分块的提示词当 k 句柄。
    // 被描述的东西不进清单：「一家医院」在下一块里指的未必是同一家
    let mut doc_entities: Vec<(Uuid, String, String)> = Vec::new();
    // 被描述的东西按描述文字在本文档内复用：同一块里「the northern wing」说了三次是一个东西
    let mut described: HashMap<String, Uuid> = HashMap::new();
    let mut handled_by_name: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut ambiguous_bare_cache: HashMap<String, Uuid> = HashMap::new();
    let mut touched_names: HashSet<String> = HashSet::new();
    let mut unextracted: Vec<(i32, String)> = Vec::new();
    let mut needs_adjudication = false;
    let mut human_reviews_found = false;
    let mut statement_count = 0usize;

    for chunk in chunks.iter() {
        // 被接管则安静退场（重抽自增 epoch）：检查放在调用模型之前
        if utopia_store::documents::extract_epoch(pool, document_id).await? != my_epoch {
            tracing::info!(%document_id, "抽取任务已被新一轮接管，退出");
            return Ok(());
        }
        let ctx: Option<&[f32]> = chunk.embedding.as_ref().map(|v| v.as_slice());
        let known: Vec<utopia_extract::KnownEntity> = doc_entities
            .iter()
            .enumerate()
            .map(|(index, (_, kind, name))| utopia_extract::KnownEntity {
                handle: format!("k{}", index + 1),
                type_key: kind.clone(),
                name: name.clone(),
            })
            .collect();
        let opening = opening_chunk
            .as_ref()
            .filter(|(id, _)| *id != chunk.id)
            .map(|(_, text)| text.as_str());
        let messages =
            utopia_extract::open::build_open_messages(&doc.filename, &known, opening, &chunk.text);
        let reply = match chat_retrying_rate_limits(state, settings, client, &messages).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%document_id, seq = chunk.seq, error = %e, "开放抽取调用失败，跳过该分块");
                unextracted.push((chunk.seq, format!("调用失败：{e}")));
                continue;
            }
        };
        tracing::debug!(%document_id, seq = chunk.seq, reply = %reply, "开放抽取的原始回复");
        let extraction = match utopia_extract::open::parse_open_response(&reply) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(%document_id, seq = chunk.seq, error = %e, "开放抽取回复解析失败，跳过该分块");
                unextracted.push((chunk.seq, format!("回复解析失败：{e}")));
                continue;
            }
        };
        if extraction.truncated {
            drop_signal(
                state,
                kb_id,
                document_id,
                reason::TRUNCATED_REPLY,
                "the open reply was cut off; kept up to the last complete item",
                None,
            )
            .await;
        }
        if extraction.skipped > 0 {
            drop_signal(
                state,
                kb_id,
                document_id,
                reason::MALFORMED_ITEM,
                &format!(
                    "{} items in the open reply were malformed",
                    extraction.skipped
                ),
                None,
            )
            .await;
        }
        // 引文逐句定位。定位不到的那句照样当证据文字记，只是没有偏移，并记一笔——
        // 模型没照抄的句子是「不是原文说的」那一类的苗头，量它
        let quotes: Vec<(String, Option<(i32, i32)>)> = extraction
            .quotes
            .iter()
            .map(|q| (q.clone(), locate(&chunk.text, q)))
            .collect();
        for (q, span) in &quotes {
            if span.is_none() {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::QUOTE_NOT_IN_CHUNK,
                    "a quoted sentence is not in the chunk verbatim",
                    Some(q),
                )
                .await;
            }
        }
        let quote_at = |i: Option<usize>| -> Option<(&str, Option<(i32, i32)>)> {
            i.and_then(|i| quotes.get(i))
                .map(|(q, span)| (q.as_str(), *span))
        };

        // ---- 东西：有名字的走身份消解，被描述的建成没有名字事实的实体 ----
        let mut response_claims: HashMap<String, Vec<Uuid>> = HashMap::new();
        let mut local: HashMap<i64, Uuid> = HashMap::new();
        for e in &extraction.entities {
            let name = e.name.trim();
            if name.is_empty() {
                continue;
            }
            let kind = e.kind.trim();
            let id = if e.named {
                let id = resolve_handle(
                    pool,
                    kb_id,
                    None,
                    name,
                    ctx,
                    Some(&chunk.text),
                    &mut response_claims,
                    &mut handled_by_name,
                    &mut ambiguous_bare_cache,
                    &mut needs_adjudication,
                    &mut human_reviews_found,
                )
                .await?;
                // 模型说它是什么（「a British film」里的 film）：存成它自己的说法，
                // 类的绑定等对齐来做
                if !kind.is_empty() {
                    let _ = utopia_store::resolution::set_specific_type(pool, id, kind).await;
                }
                // 名字就在这一块原文里时，给名字事实补出处（0041）
                if span_in_quote(name, &chunk.text) {
                    let _ = utopia_store::names::record(
                        pool,
                        kb_id,
                        id,
                        name,
                        Some(utopia_store::names::NameSource {
                            chunk_id: chunk.id,
                            quote: name,
                        }),
                        attested_at,
                    )
                    .await;
                }
                touched_names.insert(utopia_store::resolution::normalize_name(name).to_lowercase());
                if !doc_entities.iter().any(|(x, _, _)| *x == id) {
                    doc_entities.push((id, kind.to_string(), name.to_string()));
                }
                id
            } else {
                let key = name
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase();
                match described.get(&key) {
                    Some(id) => *id,
                    None => {
                        let id = utopia_store::resolution::create_described(
                            pool,
                            kb_id,
                            name,
                            (!kind.is_empty()).then_some(kind),
                        )
                        .await?;
                        described.insert(key, id);
                        id
                    }
                }
            };
            local.insert(e.id, id);
        }
        // 清单里的 k 句柄指的是**这一块开抽时**清单上的那些；这一块新认下的追加在后面，
        // 前面的序号不变，所以这里直接按当前清单取
        let resolve_ref = |r: &Ref| -> Option<Uuid> {
            match r {
                Ref::Local(n) => local.get(n).copied(),
                Ref::Known(h) => known_index(h)
                    .and_then(|i| doc_entities.get(i))
                    .map(|(id, _, _)| *id),
            }
        };

        // ---- 陈述 ----
        let times: HashMap<i64, &OpenTime> = extraction.times.iter().map(|t| (t.id, t)).collect();
        for s in &extraction.statements {
            let phrase = s.phrase.trim();
            if phrase.is_empty() {
                continue;
            }
            let Some(subject) = resolve_ref(&s.subject) else {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::UNKNOWN_REF,
                    "a statement's subject points at nothing in the reply",
                    Some(phrase),
                )
                .await;
                continue;
            };
            let object = match &s.object {
                Some(r) => match resolve_ref(r) {
                    Some(id) => Some(id),
                    None => {
                        drop_signal(
                            state,
                            kb_id,
                            document_id,
                            reason::UNKNOWN_REF,
                            "a statement's object points at nothing in the reply",
                            Some(phrase),
                        )
                        .await;
                        continue;
                    }
                },
                None => None,
            };
            let value = s.value.as_deref().map(str::trim).filter(|v| !v.is_empty());
            // 宾语是实体就走边；同一件东西指向自己不是一条边
            let value_json;
            let fact_object = match (object, value) {
                (Some(o), _) if o == subject => {
                    drop_signal(
                        state,
                        kb_id,
                        document_id,
                        reason::OBJECT_MISSING,
                        "a statement points at its own subject",
                        Some(phrase),
                    )
                    .await;
                    continue;
                }
                (Some(o), _) => FactObject::Entity(o),
                (None, Some(v)) => {
                    value_json = serde_json::json!({ "value": v });
                    FactObject::Value(&value_json)
                }
                (None, None) => {
                    drop_signal(
                        state,
                        kb_id,
                        document_id,
                        reason::OBJECT_MISSING,
                        "a statement has neither an object nor a value",
                        Some(phrase),
                    )
                    .await;
                    continue;
                }
            };
            // 开放陈述没有模型自报的置信度：它说的是「文档这么说了」。看图描述出来的块
            // 照旧压上限（0040 决定 4）
            let confidence = origin_ceiling(&chunk.origin, 1.0);
            let (fact_id, _created) = utopia_store::graph::insert_open_statement(
                pool,
                kb_id,
                subject,
                phrase,
                fact_object,
                attested_at,
                confidence,
            )
            .await?;
            let quote = quote_at(s.quote);
            // 表层谓词也写短语：今天的读路径都从 `fact_surface_predicate` 取名字
            utopia_store::graph::add_evidence_located(
                pool,
                fact_id,
                chunk.id,
                quote.map(|(q, _)| q),
                Some(phrase),
                quote.and_then(|(_, span)| span),
            )
            .await?;
            for (role, qv) in &s.qualifiers {
                let role = role.trim();
                if role.is_empty() {
                    continue;
                }
                match qv {
                    QualifierValue::Entity(r) => match resolve_ref(r) {
                        Some(id) => {
                            utopia_store::graph::add_statement_qualifier(
                                pool,
                                fact_id,
                                role,
                                None,
                                Some(id),
                            )
                            .await?
                        }
                        None => {
                            drop_signal(
                                state,
                                kb_id,
                                document_id,
                                reason::UNKNOWN_REF,
                                "a qualifier points at nothing in the reply",
                                Some(role),
                            )
                            .await
                        }
                    },
                    QualifierValue::Text(t) => {
                        let t = t.trim();
                        if t.is_empty() {
                            continue;
                        }
                        utopia_store::graph::add_statement_qualifier(
                            pool,
                            fact_id,
                            role,
                            Some(&serde_json::json!(t)),
                            None,
                        )
                        .await?;
                    }
                }
            }
            for tid in &s.times {
                let Some(t) = times.get(tid) else {
                    drop_signal(
                        state,
                        kb_id,
                        document_id,
                        reason::UNKNOWN_REF,
                        "a statement points at a time mention that is not in the reply",
                        Some(phrase),
                    )
                    .await;
                    continue;
                };
                let words = t.text.trim();
                // 时间词必须原样在块里：找不到就不记——一个凭空的时间比没有更糟
                match locate_time(&chunk.text, quote_at(t.quote).or(quote), words) {
                    Some(start) => {
                        utopia_store::time_mentions::record(
                            pool, kb_id, fact_id, chunk.id, words, start,
                        )
                        .await?;
                    }
                    None => {
                        drop_signal(
                            state,
                            kb_id,
                            document_id,
                            reason::TIME_NOT_IN_QUOTE,
                            "a time mention's words are not in the chunk verbatim",
                            Some(words),
                        )
                        .await;
                    }
                }
            }
            statement_count += 1;
        }

        // ---- 别名（0041 决定 2）：服务端只核对名字确实在这一块原文里 ----
        for n in &extraction.names {
            let name = n.name.trim();
            if name.is_empty() {
                continue;
            }
            let Some(id) = resolve_ref(&n.entity) else {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::UNKNOWN_REF,
                    "a name points at nothing in the reply",
                    Some(name),
                )
                .await;
                continue;
            };
            if !span_in_quote(name, &chunk.text) {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::NAME_NOT_IN_TEXT,
                    "an extra name is not in the chunk",
                    Some(name),
                )
                .await;
                continue;
            }
            // 一个名字不会同时是两样东西的名字：这一块或本文档前面已经把它认成
            // 另一个实体的，不记
            let key = utopia_store::resolution::normalize_name(name).to_lowercase();
            let claimed = handled_by_name
                .get(&key)
                .is_some_and(|ids| ids.iter().any(|x| *x != id));
            if claimed {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::NAME_CLAIMED_BY_ANOTHER,
                    "an extra name is already another thing's name",
                    Some(name),
                )
                .await;
                continue;
            }
            let quote = quote_at(n.quote).map(|(q, _)| q).unwrap_or(&chunk.text);
            let _ = utopia_store::names::record(
                pool,
                kb_id,
                id,
                name,
                Some(utopia_store::names::NameSource {
                    chunk_id: chunk.id,
                    quote,
                }),
                attested_at,
            )
            .await;
            touched_names.insert(key);
        }

        // 本块抽完即打标：更新时被认领的块携带标记跳过；中断的抽取可续跑
        utopia_store::documents::mark_chunk_extracted(pool, chunk.id).await?;
    }

    // 消歧后缀在实体创建时算会早于其事实写入——收尾时对本文档涉及的名字统一刷新
    for name in &touched_names {
        utopia_store::resolution::refresh_disambiguators(pool, kb_id, name).await?;
    }
    if utopia_store::documents::extract_epoch(pool, document_id).await? != my_epoch {
        tracing::info!(%document_id, "抽取任务已被新一轮接管，收尾时退出");
        return Ok(());
    }
    // 有分块没抽成就不许标 done（与类型化那条路同一条规矩）
    if let Some(msg) = incomplete_reason(&unextracted, chunks.len()) {
        return Err(anyhow::anyhow!(msg));
    }
    utopia_store::documents::set_graph_status(pool, document_id, "done").await?;
    state.emit_document(kb_id, document_id);
    state.emit_graph(kb_id);

    // 灰区对进了审核队列 → 治理 / 裁决任务，同库已排着的不重复。
    // 不排类型消解、不排自动扩本体：开放图谱里没有类也没有关系可扩，那是对齐的事
    if kb.governance {
        if needs_adjudication || human_reviews_found {
            utopia_store::jobs::enqueue_unless_queued(
                pool,
                "govern",
                serde_json::json!({ "kb_id": kb_id }),
            )
            .await?;
        }
    } else if needs_adjudication {
        utopia_store::jobs::enqueue_unless_queued(
            pool,
            "adjudicate_entities",
            serde_json::json!({ "kb_id": kb_id }),
        )
        .await?;
    }
    if needs_adjudication || human_reviews_found {
        state.emit_review(kb_id);
    }
    tracing::info!(%document_id, statements = statement_count, "开放图谱抽取完成");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_in_characters_not_bytes() {
        let text = "北京的冬天很冷。去年冬天下了三场雪。";
        assert_eq!(locate(text, "去年冬天"), Some((8, 12)));
        assert_eq!(locate(text, "  去年冬天 "), Some((8, 12)));
        assert_eq!(locate(text, "前年冬天"), None);
        assert_eq!(locate(text, ""), None);
    }

    #[test]
    fn a_time_is_found_inside_its_own_sentence_first() {
        let text = "In 2019 the plant opened. In 2019 it closed again.";
        let quote = ("In 2019 it closed again.", Some((26, 50)));
        assert_eq!(locate_time(text, Some(quote), "2019"), Some(29));
        // 引文没定位到就退回整块里的第一个
        assert_eq!(locate_time(text, Some(("x", None)), "2019"), Some(3));
        assert_eq!(locate_time(text, None, "2020"), None);
    }

    #[test]
    fn known_handles_count_from_one() {
        assert_eq!(known_index("k1"), Some(0));
        assert_eq!(known_index("k12"), Some(11));
        assert_eq!(known_index("k0"), None);
        assert_eq!(known_index("e3"), None);
    }
}
