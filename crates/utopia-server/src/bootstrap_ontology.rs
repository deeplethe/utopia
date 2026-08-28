//! 冷启动自动扩本体：本体从没被人碰过时，第一批文档抽完后自己把词表建起来。
//!
//! 新建的库只有 10 个默认关系——那不是任何人选的，是种子数据。传进第一批
//! 文档，里面的关系大半不在这 10 个里，于是大量事实降级成 related_to，
//! 图基本没法用，直到有人坐下来点 Suggest、看提案、逐条 Add。
//! **没有精心建立的词表要保护时，坚持人工审批保护的是个还不存在的东西。**
//!
//! 敢这么做的前提是采纳可撤销（见 graph::unadopt）：错了点一下就回去，
//! 旧事实从来没被销毁过。所以判断轴不是"有多确信"而是"错了有多贵"。
//!
//! 唯独 `functional` 永不自动：它驱动时态引擎自动闭合事实、生成冲突，
//! 等发现时那些闭合本身已是一串 supersede 链——不属于"错了很便宜"那类。

use uuid::Uuid;

use crate::api::ontology_routes;
use crate::state::AppState;

/// 少于这么多个待认领的说法就不折腾——凑不出像样的提案，白烧一次 LLM 调用。
const MIN_FORMS: usize = 3;

pub async fn bootstrap_ontology(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    // 并发的两个抽取任务可能都看到"空闲"而各入队一次；这里重查一遍，
    // 先跑的那个建出关系后，后跑的这个就是 no-op
    if !utopia_store::graph::ontology_untouched(&state.pool, kb_id).await? {
        tracing::debug!(%kb_id, "本体已被扩展过，跳过冷启动");
        return Ok(());
    }
    let forms = utopia_store::graph::surface_predicates(&state.pool, kb_id).await?;
    if forms.len() < MIN_FORMS {
        tracing::debug!(%kb_id, n = forms.len(), "待认领的说法太少，跳过冷启动");
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
