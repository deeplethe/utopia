//! 开放图谱的写入路径（0044 第 1 刀，#729）：文档说了什么，就按它自己的话记下来。
//!
//! 和 `extraction::run` 的分别只有一处：**提示词里没有本体**，模型不选关系、不选类、
//! 不算日期。回复里是它提到的东西（`e`，有名字的和只被描述的）、它做的陈述（`s`，关系
//! 短语照抄，主宾按名字写，时间词照抄，引文整句照抄）和别名（`n`）。落库时陈述成
//! `layer = 'open'` 的事实行，短语留在行上；限定按文档自己的角色词挂在
//! `statement_qualifiers`；时间词原样进 `time_mentions`，谁也不把它算成日期——那是
//! 0045 的事。类型化的事实由对齐（第 2 刀）从这些行算出来，不在这里写。
//!
//! **陈述按名字指东西，不按编号。** 第一版让 `e` 带编号、陈述写编号、引文按句号索引，
//! 省的是输出 token；实测 deepseek-v4-flash 在密的段落里会把编号对错——同一块两次回复
//! 一次「NVIDIA has reached AI」一次「tokens are tokens」，原文说的是「AI has reached its
//! inflection point」。原型按名字写主宾、每条陈述抄引文，333 条里编造 0 条。忠实先于省钱
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

/// 时间词在块里的字符起点。**必须在这条陈述自己的那句引文里**：模型会把一个时间词挂到
/// 好几条陈述上（FDA 语料实测「week 4」挂到了「不应由过敏患者服用」上，六条带时间的陈述错了三条），
/// 整块里搜得到不等于这句说了它。引文里有、但引文本身没在块里定位到的，起点退回整块里的第一处
fn locate_time(chunk: &str, quote: Option<(&str, Option<(i32, i32)>)>, words: &str) -> Option<i32> {
    let (q, span) = quote?;
    let (inner, _) = locate(q, words)?;
    match span {
        Some((start, _)) => Some(start + inner),
        None => locate(chunk, words).map(|(s, _)| s),
    }
}

/// 名字的查找键：空白折叠、小写。陈述里写的名字和 `e` 里列的名字要一字不差，
/// 差的只许是空白和大小写
fn name_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

    // 本文档已认下的**有名字的**实体，按首次出现排序，送进后续分块的提示词；
    // 陈述按名字指它们。被描述的东西不进清单：「一家医院」在下一块里指的未必是同一家
    let mut doc_entities: Vec<(Uuid, String, String)> = Vec::new();
    let mut known_by_name: HashMap<String, Uuid> = HashMap::new();
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

        // ---- 东西：有名字的走身份消解，被描述的建成没有名字事实的实体 ----
        let mut response_claims: HashMap<String, Vec<Uuid>> = HashMap::new();
        let mut local: HashMap<String, Uuid> = HashMap::new();
        for e in &extraction.entities {
            let name = e.name.trim();
            if name.is_empty() {
                continue;
            }
            let kind = e.kind.trim();
            let key = name_key(name);
            if local.contains_key(&key) {
                continue;
            }
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
                    known_by_name.entry(key.clone()).or_insert(id);
                }
                id
            } else {
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
                        described.insert(key.clone(), id);
                        id
                    }
                }
            };
            local.insert(key, id);
        }
        // 陈述里写的名字 → 实体：先看这一块列出的，再看本文档前面认下的（提示词里的清单）
        let resolve_name = |name: &str| -> Option<Uuid> {
            let key = name_key(name);
            local.get(&key).or_else(|| known_by_name.get(&key)).copied()
        };

        // ---- 陈述 ----
        for s in &extraction.statements {
            let phrase = s.phrase.trim();
            if phrase.is_empty() {
                continue;
            }
            let Some(subject) = resolve_name(&s.subject) else {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::UNKNOWN_REF,
                    "a statement's subject is not a listed thing",
                    Some(&s.subject),
                )
                .await;
                continue;
            };
            // 宾语写了名字但没在清单上：模型漏列了它。陈述照落，宾语落成字面值——
            // 不凭空建实体，也不丢这条话（#559 的那一档）
            let mut value = s.value.as_deref().map(str::trim).filter(|v| !v.is_empty());
            let object = match s.object.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
                Some(name) => match resolve_name(name) {
                    Some(id) => Some(id),
                    None => {
                        drop_signal(
                            state,
                            kb_id,
                            document_id,
                            reason::OBJECT_UNDECLARED,
                            "a statement's object is not a listed thing; written as a value",
                            Some(name),
                        )
                        .await;
                        value = value.or(Some(name));
                        None
                    }
                },
                None => None,
            };
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
            // 引文定位。定位不到的那句照样当证据文字记，只是没有偏移，并记一笔——
            // 模型没照抄的句子是「不是原文说的」那一类的苗头，量它
            let quote_text = s.quote.as_deref().map(str::trim).filter(|q| !q.is_empty());
            let quote: Option<(&str, Option<(i32, i32)>)> =
                quote_text.map(|q| (q, locate(&chunk.text, q)));
            if let Some((q, None)) = quote {
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
            // 限定：值是清单上某个东西的名字就挂实体，否则挂文字
            for (role, text) in &s.qualifiers {
                let (role, text) = (role.trim(), text.trim());
                if role.is_empty() || text.is_empty() {
                    continue;
                }
                match resolve_name(text) {
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
                        utopia_store::graph::add_statement_qualifier(
                            pool,
                            fact_id,
                            role,
                            Some(&serde_json::json!(text)),
                            None,
                        )
                        .await?
                    }
                }
            }
            // 时间词：起与止各是一条提及，必须原样在这条陈述的引文里
            for words in [s.when.as_deref(), s.ended.as_deref()]
                .into_iter()
                .flatten()
                .map(str::trim)
                .filter(|w| !w.is_empty())
            {
                match locate_time(&chunk.text, quote, words) {
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
                            "a time mention's words are not in the statement's own sentence",
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
            let Some(id) = resolve_name(&n.entity) else {
                drop_signal(
                    state,
                    kb_id,
                    document_id,
                    reason::UNKNOWN_REF,
                    "a name's thing is not a listed thing",
                    Some(&n.entity),
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
            let quote = n
                .quote
                .as_deref()
                .map(str::trim)
                .filter(|q| !q.is_empty())
                .unwrap_or(&chunk.text);
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
        // 引文里有、引文自己没定位到：起点退回整块里的第一处
        assert_eq!(
            locate_time(text, Some(("it closed in 2019", None)), "2019"),
            Some(3)
        );
        // 不在这句引文里的时间不算这条陈述的，哪怕块里别处有
        assert_eq!(
            locate_time(text, Some(("the plant opened.", Some((8, 25)))), "2019"),
            None
        );
        assert_eq!(locate_time(text, None, "2019"), None);
    }

    #[test]
    fn names_match_up_to_whitespace_and_case() {
        assert_eq!(name_key("  Harbor   Bridge "), "harbor bridge");
        assert_eq!(name_key("harbor bridge"), name_key("HARBOR BRIDGE"));
        assert_ne!(name_key("Harbor Bridge"), name_key("Harbour Bridge"));
    }
}
