//! OWL 导入：原文保真 → 投影 → 预览 → 落库。
//!
//! **预览与落库走同一个计划**。两条独立的代码路径迟早分叉，而分叉的后果是
//! 用户点确认之后发生的事与他刚看过的不一样——那比没有预览更糟。
//!
//! 匹配按 **IRI** 不按 key：上游改一次 `rdfs:label`，派生出的 key 就变了，
//! 按 key 匹配会把同一个类当新类建出来，实体全留在孤儿上（见 0001 P2）。

use std::collections::HashMap;

use utopia_core::AppResult;
use utopia_ingest::ontology_rdf::{self, OwlProjection, RdfFormat};
use uuid::Uuid;

use crate::state::AppState;

/// 一个类/属性在这次导入里的去向。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// 本体里没有这个 IRI → 新建
    Create,
    /// IRI 已在 → 更新标签与描述（key 不动：它可能已被引用）
    Update,
    /// key 被另一个 IRI 占着 → 跳过并报告，不悄悄改名也不覆盖
    KeyTaken,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PlannedItem {
    pub iri: String,
    pub key: String,
    pub label: String,
    pub has_description: bool,
    pub disposition: Disposition,
    /// 关系专用：导入声明它是函数性的 —— 预览必须把这些单独列出来
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub functional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict_with: Option<String>,
}

/// 一次导入的完整计划。预览返回它，落库也执行它。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportPlan {
    pub format: String,
    pub triples: usize,
    pub classes: Vec<PlannedItem>,
    pub relations: Vec<PlannedItem>,
    pub attributes: Vec<PlannedItem>,
    /// 出现过但今天不消费的公理 → 次数。**不是"已跳过"，是"暂未投影"**
    pub unprojected: Vec<(String, usize)>,
    /// 没有 `rdfs:comment` 的类数。它们在抽取里质量会明显偏低——
    /// description 逐字进提示词，是模型判断"什么算这个类"的唯一依据
    pub classes_without_description: usize,
    /// 声明为函数性的关系数。**这是 part_of 那个坑的企业版**：本体声明唯一，
    /// 数据不遵守，导完就是一队假冲突
    pub functional_relations: usize,
}

/// 解析文件并对着现有本体算出计划。不写任何东西。
pub async fn plan(
    state: &AppState,
    kb_id: Uuid,
    filename: &str,
    bytes: &[u8],
) -> AppResult<(ImportPlan, OwlProjection, RdfFormat)> {
    let format = RdfFormat::detect(filename, bytes);
    let proj = ontology_rdf::project(bytes, format).map_err(|e| {
        utopia_core::AppError::Validation(format!("Could not parse this ontology file: {e}"))
    })?;

    // 现有本体：按 IRI 与按 key 各建一份索引，两种冲突分别判
    let etypes = utopia_store::graph::entity_types(&state.pool, kb_id).await?;
    let rtypes = utopia_store::graph::relation_types(&state.pool, kb_id).await?;
    let e_by_iri: HashMap<&str, &_> = etypes
        .iter()
        .filter_map(|t| t.iri.as_deref().map(|i| (i, t)))
        .collect();
    let e_by_key: HashMap<&str, &_> = etypes.iter().map(|t| (t.key.as_str(), t)).collect();
    let r_by_iri: HashMap<&str, &_> = rtypes
        .iter()
        .filter_map(|t| t.iri.as_deref().map(|i| (i, t)))
        .collect();
    let r_by_key: HashMap<&str, &_> = rtypes.iter().map(|t| (t.key.as_str(), t)).collect();

    let mut classes = Vec::new();
    for c in &proj.classes {
        let (disposition, conflict_with) = if e_by_iri.contains_key(c.iri.as_str()) {
            (Disposition::Update, None)
        } else if let Some(existing) = e_by_key.get(c.key.as_str()) {
            // key 撞了但 IRI 不同：这是两个不同的东西争一个短标签。
            // 不自动加后缀——那会让重导入认不出自己上次建的是哪个
            // 占位者可能是手工建的（没有 IRI）——那就返回 None，
            // 由界面决定怎么措辞。服务端不产出展示文案
            (Disposition::KeyTaken, existing.iri.clone())
        } else {
            (Disposition::Create, None)
        };
        classes.push(PlannedItem {
            iri: c.iri.clone(),
            key: c.key.clone(),
            label: c.label.clone(),
            has_description: !c.description.trim().is_empty(),
            disposition,
            functional: false,
            conflict_with,
        });
    }

    let mut relations = Vec::new();
    let mut attributes = Vec::new();
    for p in &proj.properties {
        let (disposition, conflict_with) = if r_by_iri.contains_key(p.iri.as_str()) {
            (Disposition::Update, None)
        } else if let Some(existing) = r_by_key.get(p.key.as_str()) {
            (Disposition::KeyTaken, existing.iri.clone())
        } else {
            (Disposition::Create, None)
        };
        let item = PlannedItem {
            iri: p.iri.clone(),
            key: p.key.clone(),
            label: p.label.clone(),
            has_description: !p.description.trim().is_empty(),
            disposition,
            functional: p.functional,
            conflict_with,
        };
        if p.is_datatype {
            attributes.push(item);
        } else {
            relations.push(item);
        }
    }

    let mut unprojected: Vec<(String, usize)> = proj
        .unprojected
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    unprojected.sort_by_key(|(_, n)| std::cmp::Reverse(*n));

    let plan = ImportPlan {
        format: format!("{format:?}").to_lowercase(),
        triples: proj.triples,
        classes_without_description: classes.iter().filter(|c| !c.has_description).count(),
        functional_relations: relations.iter().filter(|r| r.functional).count(),
        classes,
        relations,
        attributes,
        unprojected,
    };
    Ok((plan, proj, format))
}

/// 执行计划。**属性暂不落库**：它们需要 domain 才能存（store 层强制），
/// 而 domain 要等类先建好并解析 IRI → id，那是下一层的事。
pub async fn apply(
    state: &AppState,
    kb_id: Uuid,
    actor: Uuid,
    filename: &str,
    bytes: &[u8],
) -> AppResult<(Uuid, ImportPlan)> {
    let (plan, proj, format) = plan(state, kb_id, filename, bytes).await?;

    // 第一层：原文按内容寻址进 blob。**先存原文再改本体**——投影失败可以重来，
    // 原文丢了就永远回不去
    let sha = sha256_hex(bytes);
    state
        .blob
        .put(&sha, bytes)
        .await
        .map_err(utopia_core::AppError::Other)?;

    let by_iri: HashMap<&str, &_> = proj.classes.iter().map(|c| (c.iri.as_str(), c)).collect();
    let mut created_classes = 0usize;
    let mut updated_classes = 0usize;
    // IRI → 本体里的 id，父类解析要用
    let mut id_of: HashMap<String, Uuid> = HashMap::new();

    for item in &plan.classes {
        let Some(c) = by_iri.get(item.iri.as_str()) else {
            continue;
        };
        match item.disposition {
            Disposition::Create => {
                let id = utopia_store::ontology::create_entity_type_with_iri(
                    &state.pool,
                    kb_id,
                    &c.key,
                    &c.label,
                    &c.description,
                    &c.iri,
                )
                .await?;
                id_of.insert(c.iri.clone(), id);
                created_classes += 1;
            }
            Disposition::Update => {
                if let Some(id) = utopia_store::ontology::update_type_from_import(
                    &state.pool,
                    kb_id,
                    &c.iri,
                    &c.label,
                    &c.description,
                )
                .await?
                {
                    id_of.insert(c.iri.clone(), id);
                    updated_classes += 1;
                }
            }
            // key 被占：报告过了，不动
            Disposition::KeyTaken => {}
        }
    }

    // 父类第二遍解析：第一遍时父类可能还没建出来
    for c in &proj.classes {
        let (Some(&child), Some(parent_iri)) = (id_of.get(&c.iri), c.parents.first()) else {
            continue;
        };
        // 多继承暂只投影主父（第一个）——`entity_type_parents` 关联表是下一层。
        // 少投影一个父分支不会静默出错，只是那分支的属性 domain 判定暂时够不到
        if let Some(&parent) = id_of.get(parent_iri) {
            let _ = utopia_store::ontology::set_parent(&state.pool, kb_id, child, parent).await;
        }
    }

    let summary = serde_json::json!({
        "classes_created": created_classes,
        "classes_updated": updated_classes,
        "classes_key_taken": plan.classes.iter().filter(|c| c.disposition == Disposition::KeyTaken).count(),
        "relations_seen": plan.relations.len(),
        "attributes_seen": plan.attributes.len(),
        "classes_without_description": plan.classes_without_description,
        "functional_relations": plan.functional_relations,
        "unprojected": plan.unprojected.iter().take(30).collect::<Vec<_>>(),
        "triples": plan.triples,
    });
    let import_id = utopia_store::ontology::record_import(
        &state.pool,
        kb_id,
        &sha,
        filename,
        &format!("{format:?}").to_lowercase(),
        bytes.len() as i64,
        &summary,
        actor,
    )
    .await?;

    let _ = utopia_store::audit::record(
        &state.pool,
        Some(kb_id),
        actor,
        "ontology.imported",
        "kb",
        Some(kb_id),
        summary,
    )
    .await;
    Ok((import_id, plan))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}
