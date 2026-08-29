//! 图谱抽取任务：逐分块调用 LLM → 实体消解 v2 → 事实 + 证据写入账本。
//! 与摄入管道分离（两段式可用）：索引完成即可搜可问，抽取慢慢跑。
//! 消解灰区只入审核队列并触发独立的攒批裁决任务——LLM 裁决永不阻塞本任务。

use crate::llm_util;
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
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
    let kb_lang = utopia_store::kbs::get(&state.pool, doc.kb_id)
        .await?
        .ontology_lang;
    utopia_store::graph::ensure_default_ontology(&state.pool, doc.kb_id, &kb_lang).await?;

    let etypes = utopia_store::graph::entity_types(&state.pool, doc.kb_id).await?;
    let rtypes = utopia_store::graph::relation_types(&state.pool, doc.kb_id).await?;
    // 关系与属性分道：属性走字面值通道，不进关系清单。
    //
    // related_to 刻意不列给模型：它在本体里是代码层的兜底，但摆进提示词就成了
    // 逃生舱——模型读到说不清的关系时不会去写原文说法，直接挑这个万能选项，
    // 而它什么都没说。实测 359 条 related_to 里只有 38 条是词表外降级，
    // 其余 321 条是模型自己挑的。撤掉之后模型要么用真关系、要么写出原文说法，
    // 兜底改由下面的代码执行，原词落进 fact_evidence.proposed_predicate 留待消解。
    let type_key_by_id: HashMap<Uuid, &str> =
        etypes.iter().map(|t| (t.id, t.key.as_str())).collect();
    let attr_meta: HashMap<&str, &utopia_core::models::RelationType> = rtypes
        .iter()
        .filter(|r| r.kind == "attribute")
        .map(|r| (r.key.as_str(), r))
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
    let type_parents: HashMap<Uuid, &[Uuid]> = etypes
        .iter()
        .map(|t| (t.id, t.parents.as_slice()))
        .collect();

    // **本体全铺还是按分块检索。**
    //
    // 全铺是今天的行为，小本体下它对且便宜：40 个类约 2k 字符，检索反而是多余的
    // 往返。大本体下它是灾难——schema.org 实测每个分块 108k tokens，而同一份语料
    // 只给种子类时抽到 25 个实体、给全量时只剩 18 个。**多给的那 959 个类
    // 吃掉了 7 个实体。**
    //
    // 所以按预算切换：装得下就全铺，装不下就每块检索。判据量的是**实际要排的
    // 那段字**（build_lists 自己数），不是另写一个估算公式——公式会跟排版分叉。
    let full = build_lists(&etypes, &rtypes, None, None);
    let budget = utopia_store::access::ontology_prompt_budget(&state.pool).await?;
    let retrieve_per_chunk = full.chars() > budget;
    if retrieve_per_chunk {
        tracing::info!(
            %document_id, chars = full.chars(), budget,
            classes = etypes.len(),
            "本体超出提示词预算，改为按分块检索候选"
        );
    }
    // 内置类恒在：检索漏掉的分块仍要有地方落脚，否则模型无类可选
    let seed_classes: HashSet<Uuid> = etypes.iter().filter(|t| t.builtin).map(|t| t.id).collect();

    // 属性 domain 允许子类：主语类型沿 parent 链上溯命中 domain 即可
    // 沿 subClassOf 上溯。**广度优先 + 访问集**，不是单链循环：
    // 一个类可以有多个父（FOAF 的 Person 同时是 Agent 与 SpatialThing），
    // 而菱形继承会从两条路到达同一个祖先，没有访问集就会重复展开。
    //
    // 深度上限换成了访问集：写入侧 set_parents 已经查环，这里再靠"最多走十层"
    // 兜底既挡不住宽的图，也会悄悄放过深的层级。
    let type_matches_domain = |ty: Uuid, domain: Uuid| -> bool {
        let mut seen: HashSet<Uuid> = HashSet::new();
        let mut queue = vec![ty];
        while let Some(cur) = queue.pop() {
            if cur == domain {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            if let Some(ps) = type_parents.get(&cur) {
                queue.extend(ps.iter().copied());
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
    // 本文档已经认下的实体，按首次出现排序，送进后续分块的提示词。
    //
    // **按 entity_id 去重，不按名字**：第 3 块写"上海研究院"若消解到了第 1 块的
    // "星云科技上海研究院"，那它不该以第二个名字进清单——清单里每个实体只有
    // 一个展示形态，就是这篇文档第一次用的那个。中文里全称先出现，所以这也是较全的那个。
    let mut doc_entities: Vec<(Uuid, String, String)> = Vec::new();
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
        // 本体装得下就用全量那份；装不下就拿**这一块自己的向量**检索候选。
        // 向量是现成的——实体消解本来就在用它（上面那个 ctx），检索一次
        // 嵌入都不用加。检索不出来（没配嵌入模型、或这块没向量）就退回全量：
        // 提示词大是慢，没有类可选是抽不出东西
        let lists = if retrieve_per_chunk {
            match ctx {
                Some(v) => chunk_lists(state, doc.kb_id, v, &etypes, &rtypes, &seed_classes)
                    .await
                    .unwrap_or(None),
                None => None,
            }
        } else {
            None
        };
        let lists = lists.as_ref().unwrap_or(&full);
        let known: Vec<(String, String)> = doc_entities
            .iter()
            .map(|(_, k, n)| (k.clone(), n.clone()))
            .collect();
        let messages = utopia_extract::build_messages(
            &lists.types,
            &lists.relations,
            &lists.attributes,
            doc_time.as_deref(),
            &doc.filename,
            &known,
            &chunk.text,
        );
        // 按模型限流：许可持有到调用结束，超出限额的分块在这里排队而不是打爆供应商
        let _permit = llm_util::acquire_chat(state, &settings).await;
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
            // 降级时记住模型提议的那个词：本体装不下不等于它说错了。
            // 只留计数的话，日后想加 model 类就找不出那 43 个实体——它们混在
            // concept 里面，唯一的出路是整库重抽
            let mut proposed: Option<&str> = None;
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
                    proposed = Some(e.type_key.as_str());
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
            if let Some(p) = proposed {
                let _ = utopia_store::resolution::set_proposed_type(&state.pool, id, p).await;
            }
            // 模型自己的说法。**跟 proposed_type 分开存**：那一列的含义是
            // "本体里没有"，增长回路靠它的稀有性设门槛；这一列每个实体都有。
            // 与粗类同名的不记——那不是更具体的说法，只是把清单抄了一遍
            if let Some(st) = e
                .specific_type
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter(|s| !s.eq_ignore_ascii_case(&e.type_key))
            {
                let _ = utopia_store::resolution::set_specific_type(&state.pool, id, st).await;
            }
            // 只记模型自己声明过类型的：主宾兜底那条路一律按 concept 消解，
            // 把一个猜出来的类型放进清单等于让后续分块照着猜的抄
            if !doc_entities.iter().any(|(eid, _, _)| *eid == id) {
                let tk = type_key_by_id.get(&type_id).copied().unwrap_or("concept");
                doc_entities.push((id, tk.to_string(), name.to_string()));
            }
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
                // 不可达：store 层强制 attribute 必有 domain，且 domain 不可改，
                // 无 domain 的属性连提示词都进不去。留着是防御，不需要信号
                if attr.domains.is_empty() {
                    continue;
                }
                // **任一 domain 命中即可**：属性挂在多个类下时，主语属于其中之一就算数
                if !attr
                    .domains
                    .iter()
                    .any(|d| type_matches_domain(subject_type, *d))
                {
                    let subj_key = type_key_by_id.get(&subject_type).copied().unwrap_or("?");
                    let dom_key = attr
                        .domains
                        .iter()
                        .filter_map(|d| type_key_by_id.get(d).copied())
                        .collect::<Vec<_>>()
                        .join("|");
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

            // **词表外的字面值：既不丢，也不给它编一个实体。**
            //
            // 走到这里说明谓词既不是已知属性也还没查关系表。它带着字面值时
            // 有两种走法，从前两种都不好：
            //   value 有而 object 空 → 掉进下面的"宾语必填"，整条静默消失；
            //   object 里塞着字面值 → 按 concept 消解，凭空造出一个叫「2015」
            //     的实体，图里多一个假节点，事后再修还得改事实的形状。
            // 现在都存成 object_value 挂在兜底谓词上：值在图里、有证据、有时态，
            // 原词进 proposed_predicate，消解那一遍只需换谓词，形状已经是对的。
            //
            // object 里的东西算不算字面值，判据从严：**模型没把它声明成实体**，
            // 且**它本身解得出数字或日期**。"杭州"两条都不满足，"2015"都满足。
            // 文本值的属性（schema.org 里 323 个）在这一档仍会变成实体——
            // 那里没有可靠判据，猜错会吃掉真实体，不猜
            let literal = match (&f.value, f.object.as_deref().map(str::trim)) {
                (Some(v), None | Some("")) if !rel_ids.contains_key(f.predicate.as_str()) => {
                    Some(v.clone())
                }
                (_, Some(o))
                    if !o.is_empty()
                        && !rel_ids.contains_key(f.predicate.as_str())
                        && !entity_ids.contains_key(o)
                        && looks_literal(o) =>
                {
                    Some(serde_json::Value::String(o.to_string()))
                }
                _ => None,
            };
            if let Some(value) = literal {
                let subject_name = f.subject.trim();
                let Some(&subject_id) = entity_ids.get(subject_name) else {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::SUBJECT_NOT_DECLARED,
                        &f.predicate,
                        Some(subject_name),
                    )
                    .await;
                    continue;
                };
                // 兜底谓词被删掉时就地报出来——跟关系那一档同理
                let Some(fallback) = related_rel else {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::FALLBACK_RELATION_MISSING,
                        &f.predicate,
                        Some(subject_name),
                    )
                    .await;
                    continue;
                };
                let _ = utopia_store::ontology::record_miss(
                    &state.pool,
                    doc.kb_id,
                    "attribute_type",
                    &f.predicate,
                    Some(&format!("{subject_name} → {value}")),
                )
                .await;
                let (fact_id, created) = utopia_store::graph::insert_value_fact(
                    &state.pool,
                    doc.kb_id,
                    subject_id,
                    fallback,
                    &serde_json::json!({ "value": value }),
                    from.map(|(t, _)| t),
                    to.map(|(t, _)| t),
                    precision,
                    confidence,
                )
                .await?;
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk.id,
                    f.quote.as_deref(),
                    Some(f.predicate.as_str()),
                )
                .await?;
                if created {
                    fact_count += 1;
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
            // 降级会把原意抹平成"有关联"——原词写进证据行的 proposed_predicate，
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
    // 自动扩本体：开关开着、且这一批都抽完了，由最后一篇触发。
    // 判据是显式开关而不是"本体有没有被碰过"——后者是从行为推断意图，
    // 推错的后果很荒唐（在提案上点一次 Add 就永久关掉建议），而且一旦为假
    // 就永不再真，本体会冻结在第一批文档碰巧包含的词汇上。
    // 并发下可能入队两次，任务自己会重查开关与状态
    if kb.auto_extend_ontology
        && utopia_store::documents::extraction_idle(&state.pool, doc.kb_id).await?
    {
        utopia_store::jobs::enqueue(
            &state.pool,
            "bootstrap_ontology",
            serde_json::json!({ "kb_id": doc.kb_id }),
        )
        .await?;
    }

    tracing::info!(%document_id, facts = fact_count, "图谱抽取完成");
    Ok(())
}

/// 宾语位上的这串东西，是不是一个字面值而不是实体的名字。
///
/// **只认数字与日期。** 这是个会吃掉真实体的判断，所以宁可漏认：
/// 漏了不过是维持今天的行为（造一个 concept 实体），认错了却是把一个
/// 真实体降成一段文本，图里少一个节点。
///
/// "2015"、"2023-03"、"6" 认；"杭州"、"首席技术官"、"3M"、"V3" 不认。
/// 调用方还额外要求模型**没有**把它声明成实体——两道门一起过才算数。
fn looks_literal(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // 纯数字（含小数与正负号）。用 f64 解而不是自己扫字符：
    // "3M"、"V3"、"２０１５"（全角）都会失败，正是想要的
    if s.parse::<f64>().is_ok() {
        return true;
    }
    // 日期：复用抽取侧那个解析器，它认 2015 / 2015-03 / 2015-03-01 等
    utopia_extract::parse_time(s).is_some()
}

/// 提示词里那三段清单：类、关系、属性。
///
/// **抽出来是为了让"全给"和"按分块检索"共用同一段排版逻辑。**两条路各排一份
/// 的话迟早分叉，而分叉在这里的后果是提示词说的与代码认的不是一回事。
struct PromptLists {
    types: Vec<(String, String, String)>,
    relations: Vec<utopia_extract::PromptRelation>,
    attributes: Vec<String>,
}

impl PromptLists {
    /// 这三段铺进提示词有多长。budget 判据用它——**量的是实际要排的那段字**，
    /// 不是另写一个估算公式（公式会跟排版分叉）。
    fn chars(&self) -> usize {
        self.types
            .iter()
            .map(|(k, l, d)| k.len() + l.len() + d.len() + 6)
            .sum::<usize>()
            + self
                .relations
                .iter()
                .map(|r| r.key.len() + r.label.len() + r.description.len() + r.signature.len() + 8)
                .sum::<usize>()
            + self.attributes.iter().map(|a| a.len() + 1).sum::<usize>()
    }
}

/// 从一个**选择集**排出三段清单。`None` = 全给（本体小于预算时的老路）。
///
/// 三处细节都是选择带来的，全给时它们不会触发：
///
/// 1. **签名只能提到选中的类**。`works_at (person → organization)` 里那两个 key
///    必须是模型看得见的——写一个没铺出去的类名，等于教它输出一个不存在的类型。
///    整侧都没选中就退回 `*`。
/// 2. **属性跟着 domain 走**。属性行是 `class.attr`，它的类没铺出去这行就没意义。
///    这也顺带解决了属性段（占提示词 28%）的裁剪，不用单独处理。
/// 3. **内置类恒在**。检索漏掉的分块仍然要有地方落脚，否则模型无类可选。
fn build_lists(
    etypes: &[utopia_core::models::EntityType],
    rtypes: &[utopia_core::models::RelationType],
    classes: Option<&HashSet<Uuid>>,
    rels: Option<&HashSet<Uuid>>,
) -> PromptLists {
    let picked_class = |id: &Uuid| classes.is_none_or(|s| s.contains(id));
    let picked_rel = |id: &Uuid| rels.is_none_or(|s| s.contains(id));
    let key_of: HashMap<Uuid, &str> = etypes
        .iter()
        .filter(|t| picked_class(&t.id))
        .map(|t| (t.id, t.key.as_str()))
        .collect();

    let types = etypes
        .iter()
        .filter(|t| picked_class(&t.id))
        .map(|t| (t.key.clone(), t.label.clone(), t.description.clone()))
        .collect();

    // 一侧的类一个都没铺出去就写 `*`：签名是导向，指向看不见的类只会误导
    let sig_of = |ids: &[Uuid]| -> String {
        let mut keys: Vec<&str> = ids
            .iter()
            .filter_map(|id| key_of.get(id).copied())
            .collect();
        if keys.is_empty() {
            return "*".into();
        }
        keys.sort_unstable();
        keys.join("|")
    };
    let relations = rtypes
        .iter()
        .filter(|r| r.kind != "attribute" && r.key != FALLBACK_RELATION_KEY)
        .filter(|r| picked_rel(&r.id))
        .map(|r| {
            let signature = if r.domains.is_empty() && r.ranges.is_empty() {
                String::new()
            } else {
                format!("{} → {}", sig_of(&r.domains), sig_of(&r.ranges))
            };
            utopia_extract::PromptRelation {
                key: r.key.clone(),
                label: r.label.clone(),
                description: r.description.clone(),
                signature,
            }
        })
        .collect();

    let attributes = rtypes
        .iter()
        .filter(|r| r.kind == "attribute" && picked_rel(&r.id))
        .flat_map(|r| r.domains.iter().map(move |d| (r, d)))
        .filter_map(|(r, domain_id)| {
            let class_key = key_of.get(domain_id)?;
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

    PromptLists {
        types,
        relations,
        attributes,
    }
}

/// 每块检索多少个类 / 关系 / 属性。**待测**——跟预算一样，定它们要那条曲线。
const PER_CHUNK_CLASSES: i64 = 40;
const PER_CHUNK_RELATIONS: i64 = 30;
const PER_CHUNK_ATTRIBUTES: i64 = 30;

/// 按这一块的向量检索候选，排出这一块专用的三段清单。
///
/// 检索失败返回 `Ok(None)` 而不是错误：调用方会退回全量。提示词大是慢，
/// 没有类可选是抽不出东西——两者之间选前者。
async fn chunk_lists(
    state: &AppState,
    kb_id: Uuid,
    embedding: &[f32],
    etypes: &[utopia_core::models::EntityType],
    rtypes: &[utopia_core::models::RelationType],
    seed_classes: &HashSet<Uuid>,
) -> anyhow::Result<Option<PromptLists>> {
    let mut classes: HashSet<Uuid> = seed_classes.clone();
    classes.extend(
        utopia_store::ontology::nearest_entity_type_ids(
            &state.pool,
            kb_id,
            embedding,
            PER_CHUNK_CLASSES,
        )
        .await?,
    );
    let mut rels: HashSet<Uuid> = HashSet::new();
    // 关系与属性分开检索：两段在提示词里是分开的，混在一起取会让其中一段
    // 被另一段挤空
    rels.extend(
        utopia_store::ontology::nearest_relation_type_ids(
            &state.pool,
            kb_id,
            embedding,
            PER_CHUNK_RELATIONS,
            Some("relation"),
        )
        .await?,
    );
    rels.extend(
        utopia_store::ontology::nearest_relation_type_ids(
            &state.pool,
            kb_id,
            embedding,
            PER_CHUNK_ATTRIBUTES,
            Some("attribute"),
        )
        .await?,
    );
    // 一个候选都没检索到 = 索引还没建好，退回全量而不是给一份空清单
    if classes.len() <= seed_classes.len() && rels.is_empty() {
        return Ok(None);
    }
    Ok(Some(build_lists(
        etypes,
        rtypes,
        Some(&classes),
        Some(&rels),
    )))
}

#[cfg(test)]
mod tests {
    use super::looks_literal;

    #[test]
    fn only_numbers_and_dates_count_as_literals() {
        // 认：这些出现在宾语位上时是值，不是实体
        for yes in ["2015", "2023-03", "2024-01-15", "1200", "62.5", "-3"] {
            assert!(looks_literal(yes), "{yes} 该认成字面值");
        }
        // 不认：判错的代价是把一个真实体降成一段文本，所以宁可漏
        for no in [
            "杭州",
            "首席技术官",
            "3M",
            "V3",
            "深蓝存储",
            "",
            "   ",
            "２０１５", // 全角数字：不是我们要处理的形态，交给实体路径
        ] {
            assert!(!looks_literal(no), "{no} 不该认成字面值");
        }
    }
}
