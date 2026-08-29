//! 自动扩本体：抽取遇到本体外的说法时，把它补进本体并改写等它的那些事实。
//!
//! 新建的库只有 10 个默认关系——那不是任何人选的，是种子数据。传进第一批
//! 文档，里面的关系大半不在这 10 个里，于是大量事实降级成 related_to，
//! 图基本没法用，直到有人坐下来点 Suggest、看提案、逐条 Add。
//!
//! 敢这么做的前提是采纳可撤销（见 graph::unadopt）：错了点一下就回去，
//! 旧事实从来没被销毁过。所以判断轴不是"有多确信"而是"错了有多贵"。
//!
//! **要不要替人做，由人在知识库设置里声明**（`auto_extend_ontology`，缺省开）。
//! 曾试过从行为推断——"本体有没有被碰过"——那是猜，而且猜错的后果荒唐：在提案上
//! 点一次 Add 就会永久关掉建议功能。开关一来，猜测没有了，冻结也没有了。
//!
//! 关掉它不影响"留意"：未匹配统计照常累积、照常在 Unmatched 面板可见，
//! 只是变成你点一下的提案，信息一条不少。
//!
//! 唯独 `functional` 永不自动，开关开着也不行：它驱动时态引擎自动闭合事实、
//! 生成冲突，等发现时那些闭合本身已是一串 supersede 链——不属于"错了很便宜"那类。

use uuid::Uuid;

use crate::api::ontology_routes;
use crate::state::AppState;

/// 少于这么多个够格的信号（谓词 + 类型）就不折腾——凑不出像样的提案，
/// 白烧一次 LLM 调用。
const MIN_SIGNALS: usize = 3;
/// 只采纳出现在这么多篇文档里的说法。**只在一篇里出现过的是那篇文档的用词，
/// 不是这个组织的词汇**——而本体会反馈进抽取提示词，一次偶然会变成长期指令。
/// 副作用正好：只有一篇文档时什么都够不着门槛，于是什么也不做，下一篇再试。
const MIN_DOCS: i64 = 2;

pub async fn bootstrap_ontology(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    // 并发的两个抽取任务可能都看到"空闲"而各入队一次；开关也可能刚被关掉
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    if !kb.auto_extend_ontology {
        tracing::debug!(%kb_id, "自动扩本体已关闭，跳过");
        return Ok(());
    }
    // 门槛看的是"够不够一次 LLM 调用的量"，**谓词与类型合起来算**。
    // 此前只数谓词，于是一个只缺实体类型、不缺关系的语料会被整个跳过——
    // proposed_type 里明明堆着 platform ×2、inference_engine ×2 在等。
    let forms: Vec<_> = utopia_store::graph::proposed_predicates(&state.pool, kb_id)
        .await?
        .into_iter()
        .filter(|f| f.doc_count >= MIN_DOCS)
        .collect();
    let types = utopia_store::resolution::proposed_types(&state.pool, kb_id).await?;
    if forms.len() + types.len() < MIN_SIGNALS {
        tracing::debug!(
            %kb_id, predicates = forms.len(), types = types.len(),
            "够格的信号太少，跳过自动扩本体"
        );
        return Ok(());
    }

    // 自动那条路没有人类调用者，reason 也不会被展示（lastAutoExtension 不回它），
    // 所以 reason 的语言无所谓——description 的语言才要紧，那个跟库走
    let proposals = ontology_routes::build_proposals(state, kb_id, "en").await?;
    let relations = proposals
        .get("relation_types")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let classes = proposals
        .get("entity_types")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut added_relations = Vec::new();
    let mut added_classes = Vec::new();
    let mut moved_total = 0u32;
    let mut batches = Vec::new();

    for p in &classes {
        let (Some(key), Some(label)) = (str_of(p, "key"), str_of(p, "label")) else {
            continue;
        };
        match utopia_store::ontology::create_entity_type(
            &state.pool,
            kb_id,
            key,
            label,
            "#8ea5bd",
            "circle",
            // 冷启动建的类不挂父：提案里没有层级信息，猜一个父类比不挂更糟
            &[],
            // 描述进抽取提示词，reason 只是给人看的理由——喂错了这个类就成新的倾倒场
            str_of(p, "description")
                .or_else(|| str_of(p, "reason"))
                .unwrap_or(""),
        )
        .await
        {
            Ok(type_id) => {
                added_classes.push(key.to_string());
                // 建类之后要把等它的实体搬过去——只建类型不动实体，
                // 本体长大了图没变好，那些提议过 model 的实体继续挂在 concept 下
                let forms: Vec<String> = p
                    .get("forms")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(String::from))
                            .collect()
                    })
                    // 提案没给 forms 时，至少认领与 key 同名的那些提议
                    .unwrap_or_else(|| vec![key.to_string()]);
                match utopia_store::resolution::adopt_proposed_types(
                    &state.pool,
                    kb_id,
                    type_id,
                    &forms,
                )
                .await
                {
                    Ok((batch, n)) => {
                        moved_total += n;
                        if n > 0 {
                            batches.push(batch);
                        }
                    }
                    Err(e) => tracing::warn!(%kb_id, key, error = %e, "实体改类失败"),
                }
                for form in &forms {
                    let _ =
                        utopia_store::ontology::clear_miss(&state.pool, kb_id, "entity_type", form)
                            .await;
                }
            }
            // key 撞车之类的：跳过这一条，别带垮整批
            Err(e) => tracing::warn!(%kb_id, key, error = %e, "冷启动建类失败"),
        }
    }

    for p in &relations {
        let (Some(key), Some(label)) = (str_of(p, "key"), str_of(p, "label")) else {
            continue;
        };
        let forms: Vec<String> = p
            .get("forms")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let temporal = str_of(p, "temporal").unwrap_or("state");
        let predicate_id = match utopia_store::ontology::create_relation_type(
            &state.pool,
            kb_id,
            key,
            label,
            temporal,
            // 建议方不替时态引擎做决定，见文件头
            false,
            false,
            // 描述进抽取提示词，reason 只是给人看的理由——喂错了这个类就成新的倾倒场
            str_of(p, "description")
                .or_else(|| str_of(p, "reason"))
                .unwrap_or(""),
            "relation",
            // 提案与冷启动只建关系，不声明 domain/range —— 留空 = 不限主宾类型
            &[],
            &[],
            None,
            None,
        )
        .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(%kb_id, key, error = %e, "冷启动建关系失败");
                continue;
            }
        };
        added_relations.push(key.to_string());
        if !forms.is_empty() {
            let (batch, moved) = utopia_store::graph::adopt_proposed_predicates(
                &state.pool,
                kb_id,
                predicate_id,
                &forms,
            )
            .await?;
            moved_total += moved;
            if moved > 0 {
                batches.push(batch);
            }
            for form in &forms {
                let _ =
                    utopia_store::ontology::clear_miss(&state.pool, kb_id, "relation_type", form)
                        .await;
            }
        }
    }

    // 属性那一档：宾语是字面值的说法。
    //
    // domain 不从提案里读——它从这些事实的主语类型里取，见 adopt_attribute。
    // 值换不动的那些不改写，继续挂在兜底谓词上等下一次
    let attrs = proposals
        .get("attribute_types")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for p in &attrs {
        let (Some(key), Some(label)) = (str_of(p, "key"), str_of(p, "label")) else {
            continue;
        };
        let forms: Vec<String> = p
            .get("forms")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if forms.is_empty() {
            continue;
        }
        match ontology_routes::adopt_attribute_auto(
            state,
            kb_id,
            key,
            label,
            str_of(p, "description")
                .or_else(|| str_of(p, "reason"))
                .unwrap_or(""),
            str_of(p, "datatype").unwrap_or("text"),
            str_of(p, "unit"),
            &forms,
        )
        .await
        {
            Ok((batch, moved)) => {
                added_relations.push(key.to_string());
                moved_total += moved;
                if moved > 0 {
                    batches.push(batch);
                }
            }
            Err(e) => tracing::warn!(%kb_id, key, error = %e, "冷启动建属性失败"),
        }
    }

    // **映射到已有类型**：本体里已经有这个意思了，只改写事实、不动本体。
    //
    // 自动执行是安全的一档：它不让本体长大，只把一批 related_to 挂到一个
    // 早就存在的谓词上，而且跟新建那条路走同一个批次机制，同样可撤销。
    // 反过来说，漏掉这一档才是危险的——检索告诉模型"已经有 founding_date 了"，
    // 模型答"这些说法就是它"，我们却什么都不做，那批事实继续是"有关联"。
    let mapped = proposals
        .get("map_to")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for m in &mapped {
        let Some(key) = str_of(m, "key") else {
            continue;
        };
        let forms: Vec<String> = m
            .get("forms")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if forms.is_empty() {
            continue;
        }
        // 目标是属性时改写走另一条路：值要按它的 datatype 换算。
        // kind 由服务端在解析提案时标上（模型只答得出一个 key）
        if str_of(m, "kind") == Some("attribute") {
            match ontology_routes::adopt_attribute_existing(state, kb_id, key, &forms).await {
                Ok((batch, moved)) => {
                    moved_total += moved;
                    if moved > 0 {
                        batches.push(batch);
                    }
                }
                Err(e) => tracing::warn!(%kb_id, key, error = %e, "映射到已有属性失败"),
            }
            continue;
        }
        // 模型偶尔会把候选清单之外的 key 抄进来（或者干脆编一个）。
        // 找不到就跳过——**不新建**：这条路的前提就是"它已经存在"
        let Some(predicate_id) =
            utopia_store::ontology::relation_type_id_by_key(&state.pool, kb_id, key).await?
        else {
            tracing::warn!(%kb_id, key, "映射目标不在本体里，跳过");
            continue;
        };
        let (batch, moved) = utopia_store::graph::adopt_proposed_predicates(
            &state.pool,
            kb_id,
            predicate_id,
            &forms,
        )
        .await?;
        moved_total += moved;
        if moved > 0 {
            batches.push(batch);
        }
        for form in &forms {
            let _ =
                utopia_store::ontology::clear_miss(&state.pool, kb_id, "relation_type", form).await;
        }
    }

    // 类先建好、实体后抽出来是常态：把等着已存在类型的那些也收走
    match utopia_store::resolution::sweep_proposed_types(&state.pool, kb_id).await {
        Ok(swept) => {
            for (batch, n) in swept {
                moved_total += n;
                batches.push(batch);
            }
        }
        Err(e) => tracing::warn!(%kb_id, error = %e, "已有类型的实体收尾失败"),
    }

    if added_relations.is_empty() && added_classes.is_empty() && moved_total == 0 {
        return Ok(());
    }
    // actor 为 NULL：这是系统的动作，不是谁的决定。台账里查得到做了什么、
    // 改了多少条、以及撤销要用的批次号
    let _ = utopia_store::audit::record_opt(
        &state.pool,
        Some(kb_id),
        None,
        "ontology.bootstrapped",
        "kb",
        Some(kb_id),
        serde_json::json!({
            "relations": added_relations,
            "classes": added_classes,
            "facts_remapped": moved_total,
            "batches": batches,
        }),
    )
    .await;
    state.emit_review(kb_id);
    tracing::info!(
        %kb_id,
        relations = added_relations.len(),
        classes = added_classes.len(),
        facts = moved_total,
        "冷启动自动扩本体完成"
    );
    Ok(())
}

fn str_of<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}
