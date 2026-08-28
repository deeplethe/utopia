//! 图谱抽取任务：逐分块调用 LLM → 实体消解 v2 → 事实 + 证据写入账本。
//! 与摄入管道分离（两段式可用）：索引完成即可搜可问，抽取慢慢跑。
//! 消解灰区只入审核队列并触发独立的攒批裁决任务——LLM 裁决永不阻塞本任务。

use crate::llm_util;
use crate::state::AppState;
use std::collections::HashMap;
use utopia_store::graph::FALLBACK_RELATION_KEY;
use uuid::Uuid;

const MIN_CONFIDENCE: f32 = 0.6;

/// 记一条丢弃信号。抽取器有七处 `continue`，每一处都是"事实抽出来了、被挡掉、
/// 什么都不说"。信号写失败绝不能带垮整篇文档的抽取，所以这里吞掉错误。
async fn drop_signal(
    state: &AppState,
    kb_id: Uuid,
    document_id: Uuid,
    reason: &str,
    detail: &str,
    example: Option<&str>,
) {
    let _ = utopia_store::extraction_drops::record(
        &state.pool,
        kb_id,
        document_id,
        reason,
        detail,
        example,
    )
    .await;
}

pub async fn extract_document(state: &AppState, document_id: Uuid) -> anyhow::Result<()> {
    match run(state, document_id).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // 原因随状态落库：只进日志的错误等于没有错误
            let _ = utopia_store::documents::set_graph_failed(
                &state.pool,
                document_id,
                &format!("{e:#}"),
            )
            .await;
            if let Ok(doc) = utopia_store::documents::get(&state.pool, document_id).await {
                state.emit_document(doc.kb_id, document_id);
            }
            Err(e)
        }
    }
}

/// mention → 实体 id。同一文档内同名同类型直接复用（单文档语境里罕有同名不同人，
/// 也把消解调用摊薄到每个名字一次）；跨文档歧义由 resolve_mention 的画像比对处理。
async fn resolve(
    state: &AppState,
    kb_id: Uuid,
    type_id: Uuid,
    name: &str,
    ctx: Option<&[f32]>,
    doc_cache: &mut HashMap<(Uuid, String), Uuid>,
    needs_adjudication: &mut bool,
) -> anyhow::Result<Uuid> {
    let key = (
        type_id,
        utopia_store::resolution::normalize_name(name).to_lowercase(),
    );
    if let Some(id) = doc_cache.get(&key) {
        return Ok(*id);
    }
    let r =
        utopia_store::resolution::resolve_mention(&state.pool, kb_id, type_id, name, ctx).await?;
    // 疑似重复对（同名灰区 / 类型漂移）入审核队列，攒批裁决任务收尾统一触发
    for review in &r.reviews {
        utopia_store::resolution::create_review(
            &state.pool,
            kb_id,
            r.entity_id,
            review.other_id,
            review.score,
            &review.reason,
        )
        .await?;
        *needs_adjudication = true;
    }
    doc_cache.insert(key, r.entity_id);
    Ok(r.entity_id)
}

async fn run(state: &AppState, document_id: Uuid) -> anyhow::Result<()> {
    let doc = utopia_store::documents::get(&state.pool, document_id).await?;
    let kb = utopia_store::kbs::get(&state.pool, doc.kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot extract"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot extract"))?;

    // 所有权凭证：重抽会自增 epoch，本任务据此察觉自己已被接管（见分块循环）
    let my_epoch = utopia_store::documents::extract_epoch(&state.pool, document_id).await?;
    utopia_store::documents::set_graph_status(&state.pool, document_id, "extracting").await?;
    state.emit_document(doc.kb_id, document_id);
    utopia_store::graph::ensure_default_ontology(&state.pool, doc.kb_id).await?;

    let etypes = utopia_store::graph::entity_types(&state.pool, doc.kb_id).await?;
    let rtypes = utopia_store::graph::relation_types(&state.pool, doc.kb_id).await?;
    let type_pairs: Vec<(String, String, String)> = etypes
        .iter()
        .map(|t| (t.key.clone(), t.label.clone(), t.description.clone()))
        .collect();
    // 关系与属性分道：属性走字面值通道，不进关系清单。
    //
    // related_to 刻意不列给模型：它在本体里是代码层的兜底，但摆进提示词就成了
    // 逃生舱——模型读到说不清的关系时不会去写原文说法，直接挑这个万能选项，
    // 而它什么都没说。实测 359 条 related_to 里只有 38 条是词表外降级，
    // 其余 321 条是模型自己挑的。撤掉之后模型要么用真关系、要么写出原文说法，
    // 兜底改由下面的代码执行，原词落进 fact_evidence.surface_predicate 留待消解。
    let rel_pairs: Vec<(String, String, String)> = rtypes
        .iter()
        .filter(|r| r.kind != "attribute" && r.key != FALLBACK_RELATION_KEY)
        .map(|r| (r.key.clone(), r.label.clone(), r.description.clone()))
        .collect();
    let type_ids: HashMap<&str, Uuid> = etypes.iter().map(|t| (t.key.as_str(), t.id)).collect();
    let rel_ids: HashMap<&str, Uuid> = rtypes.iter().map(|r| (r.key.as_str(), r.id)).collect();
    // 时态对账只对带唯一性约束的状态关系生效（本体元数据）：(functional, inverse_functional, temporal)
    let rel_meta: HashMap<Uuid, (bool, bool, String)> = rtypes
        .iter()
        .map(|r| {
            (
                r.id,
                (r.functional, r.inverse_functional, r.temporal.clone()),
            )
        })
        .collect();
    // 属性元数据（按 key 查）与提示词清单（"person.salary (number, CNY): 月薪"）。
    // 没定义属性时清单为空，提示词一字不变
    let type_key_by_id: HashMap<Uuid, &str> =
        etypes.iter().map(|t| (t.id, t.key.as_str())).collect();
    let type_parent: HashMap<Uuid, Option<Uuid>> =
        etypes.iter().map(|t| (t.id, t.parent_id)).collect();
    let attr_meta: HashMap<&str, &utopia_core::models::RelationType> = rtypes
        .iter()
        .filter(|r| r.kind == "attribute")
        .map(|r| (r.key.as_str(), r))
        .collect();
    let attr_lines: Vec<String> = rtypes
        .iter()
        .filter(|r| r.kind == "attribute")
        .filter_map(|r| {
            let class_key = type_key_by_id.get(&r.domain_type_id?)?;
            let dt = r.datatype.as_deref().unwrap_or("text");
            let spec = match &r.unit {
                Some(u) if !u.is_empty() => format!("{dt}, {u}"),
                _ => dt.to_string(),
            };
            let d = r.description.trim();
            Some(if d.is_empty() {
                format!("- {class_key}.{} ({spec})", r.key)
            } else {
                format!("- {class_key}.{} ({spec}): {d}", r.key)
            })
        })
        .collect();
    // 属性 domain 允许子类：主语类型沿 parent 链上溯命中 domain 即可
    let type_matches_domain = |mut ty: Uuid, domain: Uuid| -> bool {
        for _ in 0..10 {
            if ty == domain {
                return true;
            }
            match type_parent.get(&ty).copied().flatten() {
                Some(p) => ty = p,
                None => return false,
            }
        }
        false
    };
    let concept_type = *type_ids
        .get("concept")
        .ok_or_else(|| anyhow::anyhow!("Ontology missing the 'concept' type"))?;
    let related_rel = rel_ids.get(FALLBACK_RELATION_KEY).copied();
    // 本轮要从头讲一遍这篇文档的故事，旧信号先清掉（重抽自动作数）
    let _ = utopia_store::extraction_drops::clear_for_document(&state.pool, document_id).await;

    let doc_time = doc.doc_time.map(|t| t.format("%Y-%m-%d").to_string());
    let chunks = utopia_store::documents::chunks_for_extraction(&state.pool, document_id).await?;

    let mut doc_cache: HashMap<(Uuid, String), Uuid> = HashMap::new();
    let mut needs_adjudication = false;
    let mut conflicts_found = false;
    let mut fact_count = 0usize;
    // 不设分块上限：静默截断等于丢知识，长文档的成本由部署者自己权衡
    // （成本优化走 prompt 前缀缓存与更新时 chunk 级跳过，而非丢数据）
    for chunk in chunks.iter() {
        // 被接管则安静退场：不写 failed、不碰状态，舞台留给新任务。
        // 检查放在调用 LLM 之前——取消粒度即一个分块，不必等整篇跑完
        if utopia_store::documents::extract_epoch(&state.pool, document_id).await? != my_epoch {
            tracing::info!(%document_id, "抽取任务已被新一轮接管，退出");
            return Ok(());
        }
        let ctx: Option<&[f32]> = chunk.embedding.as_ref().map(|v| v.as_slice());
        let messages = utopia_extract::build_messages(
            &type_pairs,
            &rel_pairs,
            &attr_lines,
            doc_time.as_deref(),
            &doc.filename,
            &chunk.text,
        );
        let reply = match client.chat(&messages).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%document_id, seq = chunk.seq, error = %e, "抽取调用失败，跳过该分块");
                continue;
            }
        };
        let extraction = match utopia_extract::parse_response(&reply) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(%document_id, seq = chunk.seq, error = %e, "抽取结果解析失败，跳过该分块");
                continue;
            }
        };

        // 实体消解：名称 → 实体 id（本分块的事实按原文名字连线）
        let mut entity_ids: HashMap<String, Uuid> = HashMap::new();
        // 名称 → 声明类型（属性 domain 校验用：salary 不能挂在 Organization 上）
        let mut entity_type_of: HashMap<String, Uuid> = HashMap::new();
        for e in &extraction.entities {
            let name = e.name.trim();
            if name.is_empty() || name.chars().count() > 100 {
                continue;
            }
            let type_id = match type_ids.get(e.type_key.as_str()) {
                Some(id) => *id,
                None => {
                    // 白名单外类型：降级 concept，并记入未匹配统计（本体扩展的信号）
                    let _ = utopia_store::ontology::record_miss(
                        &state.pool,
                        doc.kb_id,
                        "entity_type",
                        &e.type_key,
                        Some(name),
                    )
                    .await;
                    concept_type
                }
            };
            let id = resolve(
                state,
                doc.kb_id,
                type_id,
                name,
                ctx,
                &mut doc_cache,
                &mut needs_adjudication,
            )
            .await?;
            entity_ids.insert(name.to_string(), id);
            entity_type_of.insert(name.to_string(), type_id);
        }

        for f in &extraction.facts {
            let confidence = f.confidence.unwrap_or(0.7).clamp(0.0, 1.0);
            if confidence < MIN_CONFIDENCE {
                // 设计上的阈值，但用户同样无从知道"抽到了，只是不够自信"
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::LOW_CONFIDENCE,
                    &f.predicate,
                    Some(&format!("{} ({:.0}%)", f.subject, confidence * 100.0)),
                )
                .await;
                continue;
            }
            let from = f.valid_from.as_deref().and_then(utopia_extract::parse_time);
            let to = f.valid_to.as_deref().and_then(utopia_extract::parse_time);
            let precision = from.map(|(_, p)| p).unwrap_or("day");

            // 属性事实：谓词命中属性 → 字面值通道。datatype 校验失败宁缺勿脏；
            // domain 校验（含子类上溯）挡住"把 salary 挂到 Organization"这类张冠李戴。
            // 模型偶尔照抄清单里的 "person.salary" 全限定名——剥掉类前缀再查一次
            let attr_hit = attr_meta.get(f.predicate.as_str()).or_else(|| {
                f.predicate
                    .rsplit_once('.')
                    .and_then(|(_, k)| attr_meta.get(k))
            });
            if let Some(attr) = attr_hit {
                let subject_name = f.subject.trim();
                // 主语没在 entities 里声明：类型不明，domain 无从校验，属性不落。
                // 关系路径遇到同样的缺失会兜底按 concept 消解——这里学不来，
                // 按 concept 解出来 domain 照样不匹配，只是从这里掉进下面那一档
                let Some((&subject_id, &subject_type)) = entity_ids
                    .get(subject_name)
                    .zip(entity_type_of.get(subject_name))
                else {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::SUBJECT_NOT_DECLARED,
                        &attr.key,
                        Some(subject_name),
                    )
                    .await;
                    continue;
                };
                // 不可达：store 层强制 attribute 必有 domain，且 kind/domain 不可改，
                // 无 domain 的属性连提示词都进不去。留着是防御，不需要信号
                let Some(domain) = attr.domain_type_id else {
                    continue;
                };
                if !type_matches_domain(subject_type, domain) {
                    let subj_key = type_key_by_id.get(&subject_type).copied().unwrap_or("?");
                    let dom_key = type_key_by_id.get(&domain).copied().unwrap_or("?");
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::ATTR_DOMAIN_MISMATCH,
                        &format!("{}@{subj_key} (wants {dom_key})", attr.key),
                        Some(subject_name),
                    )
                    .await;
                    continue;
                }
                let raw = match (&f.value, &f.object) {
                    (Some(v), _) => v.clone(),
                    // 模型偶尔把值放进 object：宽容接住
                    (None, Some(o)) if !o.trim().is_empty() => {
                        serde_json::Value::String(o.trim().to_string())
                    }
                    _ => {
                        drop_signal(
                            state,
                            doc.kb_id,
                            document_id,
                            utopia_store::extraction_drops::reason::ATTR_NO_VALUE,
                            &attr.key,
                            Some(subject_name),
                        )
                        .await;
                        continue;
                    }
                };
                let datatype = attr.datatype.as_deref().unwrap_or("text");
                let Some(normalized) = utopia_extract::normalize_attr_value(datatype, &raw) else {
                    tracing::debug!(%document_id, attr = attr.key, ?raw, "属性值不合 datatype，跳过");
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::ATTR_DATATYPE,
                        &format!("{} ({datatype})", attr.key),
                        Some(&format!("{subject_name} → {raw}")),
                    )
                    .await;
                    continue;
                };
                let mut object_value = serde_json::json!({ "value": normalized });
                if let Some(u) = attr.unit.as_deref().filter(|u| !u.is_empty()) {
                    // 单位随事实落笔：类型上的单位以后改了，旧值仍按记录时的单位读
                    object_value["unit"] = serde_json::json!(u);
                }
                let (fact_id, created) = utopia_store::graph::insert_value_fact(
                    &state.pool,
                    doc.kb_id,
                    subject_id,
                    attr.id,
                    &object_value,
                    from.map(|(t, _)| t),
                    to.map(|(t, _)| t),
                    precision,
                    confidence,
                )
                .await?;
                // 属性谓词也留原词：模型偶尔照抄 "person.salary" 全限定名，
                // 命中的是剥掉前缀后的 key，原样是什么值得留着
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk.id,
                    f.quote.as_deref(),
                    Some(f.predicate.as_str()),
                )
                .await?;
                if !created {
                    continue;
                }
                fact_count += 1;
                // 单值属性 = functional：新值闭合旧值（属性历史由此而来）
                if attr.functional && attr.temporal == "state" {
                    let report = utopia_store::temporal::reconcile_new_fact(
                        &state.pool,
                        doc.kb_id,
                        fact_id,
                        subject_id,
                        attr.id,
                        None,
                        Some(&object_value),
                        utopia_store::temporal::Uniqueness::SubjectSide,
                        from.map(|(t, _)| t),
                        to.map(|(t, _)| t),
                        confidence,
                    )
                    .await?;
                    if report.conflicts > 0 {
                        conflicts_found = true;
                    }
                }
                continue;
            }

            // 关系事实：宾语必填
            let Some(object_name) = f.object.as_deref().map(str::trim).filter(|s| !s.is_empty())
            else {
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::OBJECT_MISSING,
                    &f.predicate,
                    Some(f.subject.trim()),
                )
                .await;
                continue;
            };
            // 主宾未在 entities 中声明时，按 concept 兜底消解（模型偶尔漏报）
            let subject_id = match entity_ids.get(f.subject.trim()) {
                Some(id) => *id,
                None => {
                    resolve(
                        state,
                        doc.kb_id,
                        concept_type,
                        f.subject.trim(),
                        ctx,
                        &mut doc_cache,
                        &mut needs_adjudication,
                    )
                    .await?
                }
            };
            let object_id = match entity_ids.get(object_name) {
                Some(id) => *id,
                None => {
                    resolve(
                        state,
                        doc.kb_id,
                        concept_type,
                        object_name,
                        ctx,
                        &mut doc_cache,
                        &mut needs_adjudication,
                    )
                    .await?
                }
            };
            if subject_id == object_id {
                continue;
            }
            // 未知关系降级为 related_to，并记入未匹配统计。
            // 降级会把原意抹平成"有关联"——原词写进证据行的 surface_predicate，
            // 是这条事实身上唯一还留着原意的地方（谓词消解据此把它映射回本体）
            let predicate_id = match rel_ids.get(f.predicate.as_str()) {
                Some(id) => *id,
                None => {
                    let _ = utopia_store::ontology::record_miss(
                        &state.pool,
                        doc.kb_id,
                        "relation_type",
                        &f.predicate,
                        Some(&format!("{} → {}", f.subject, object_name)),
                    )
                    .await;
                    match related_rel {
                        Some(id) => id,
                        // 本体里连兜底关系都被删了 → 整条事实消失，这个必须说出来
                        None => {
                            drop_signal(
                                state,
                                doc.kb_id,
                                document_id,
                                utopia_store::extraction_drops::reason::FALLBACK_RELATION_MISSING,
                                &f.predicate,
                                Some(&format!("{} → {object_name}", f.subject)),
                            )
                            .await;
                            continue;
                        }
                    }
                }
            };

            {
                let (fact_id, created) = utopia_store::graph::insert_fact(
                    &state.pool,
                    doc.kb_id,
                    subject_id,
                    predicate_id,
                    object_id,
                    from.map(|(t, _)| t),
                    to.map(|(t, _)| t),
                    precision,
                    confidence,
                )
                .await?;
                // 重复观察也要挂证据：多来源相互印证，任一来源删除后事实不孤儿化。
                // 表层谓词随每次观察落笔——甲块说 "runs on"、乙块说 "optimized for"
                // 会并进同一条事实，放事实上就是先写者胜，放证据上两个都留着
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk.id,
                    f.quote.as_deref(),
                    Some(f.predicate.as_str()),
                )
                .await?;
                if !created {
                    continue;
                }
                fact_count += 1;
                // 时态对账：带唯一性约束的状态关系落新事实即检测矛盾（纯规则点查，
                // 自动闭合走"作废+改写"，拿不准进 fact_conflicts 人裁）
                if let Some((func, inv_func, temporal)) = rel_meta.get(&predicate_id) {
                    if temporal == "state" {
                        let mut directions = Vec::new();
                        if *func {
                            directions.push(utopia_store::temporal::Uniqueness::SubjectSide);
                        }
                        if *inv_func {
                            directions.push(utopia_store::temporal::Uniqueness::ObjectSide);
                        }
                        for dir in directions {
                            let report = utopia_store::temporal::reconcile_new_fact(
                                &state.pool,
                                doc.kb_id,
                                fact_id,
                                subject_id,
                                predicate_id,
                                Some(object_id),
                                None,
                                dir,
                                from.map(|(t, _)| t),
                                to.map(|(t, _)| t),
                                confidence,
                            )
                            .await?;
                            if report.conflicts > 0 {
                                conflicts_found = true;
                            }
                        }
                    }
                }
            }
        }

        // 本块抽取完成即打标：更新时被认领的块携带标记跳过；中断的抽取可续跑
        // （LLM 调用/解析失败的块在上方 continue 掉，不打标，下次重试）
        utopia_store::documents::mark_chunk_extracted(&state.pool, chunk.id).await?;
    }

    // 消歧后缀在实体创建时算会早于其事实写入——收尾时对本文档涉及的名字统一刷新
    let names: std::collections::HashSet<&str> =
        doc_cache.keys().map(|(_, name)| name.as_str()).collect();
    for name in names {
        utopia_store::resolution::refresh_disambiguators(&state.pool, doc.kb_id, name).await?;
    }

    // 出口再验一次：接管可能发生在最后一个分块之后，那时循环里的检查已经跑完。
    // 漏掉这里，被顶替的任务会把 done 写在一篇 extracted_at 刚被清空的文档上——
    // 界面显示"已完成"，实则一条都没抽，要等新任务开跑才纠正回来。
    if utopia_store::documents::extract_epoch(&state.pool, document_id).await? != my_epoch {
        tracing::info!(%document_id, "抽取任务已被新一轮接管，收尾时退出");
        return Ok(());
    }
    utopia_store::documents::set_graph_status(&state.pool, document_id, "done").await?;
    state.emit_document(doc.kb_id, document_id);

    // 灰区对进了审核队列 → 触发攒批裁决任务（独立后台跑，抽取本身到此已完成）
    if needs_adjudication {
        utopia_store::jobs::enqueue(
            &state.pool,
            "adjudicate_entities",
            serde_json::json!({ "kb_id": doc.kb_id }),
        )
        .await?;
    }
    if needs_adjudication || conflicts_found {
        state.emit_review(doc.kb_id);
    }

    tracing::info!(%document_id, facts = fact_count, "图谱抽取完成");
    Ok(())
}
