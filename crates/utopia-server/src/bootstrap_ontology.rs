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

/// 少于这么多个够格的说法就不折腾——凑不出像样的提案，白烧一次 LLM 调用。
const MIN_FORMS: usize = 3;
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
    let forms: Vec<_> = utopia_store::graph::surface_predicates(&state.pool, kb_id)
        .await?
        .into_iter()
        .filter(|f| f.doc_count >= MIN_DOCS)
        .collect();
    if forms.len() < MIN_FORMS {
        tracing::debug!(%kb_id, n = forms.len(), "够格的说法太少，跳过自动扩本体");
        return Ok(());
    }

    let proposals = ontology_routes::build_proposals(state, kb_id).await?;
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
            None,
            str_of(p, "reason").unwrap_or(""),
        )
        .await
        {
            Ok(_) => added_classes.push(key.to_string()),
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
            str_of(p, "reason").unwrap_or(""),
            "relation",
            None,
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
            let (batch, moved) = utopia_store::graph::adopt_surface_predicates(
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

    if added_relations.is_empty() && added_classes.is_empty() {
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
