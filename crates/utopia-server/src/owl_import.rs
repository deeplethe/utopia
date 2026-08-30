//! OWL 导入：原文保真 → 投影 → 预览 → 落库。
//!
//! **预览与落库走同一个计划**。两条独立的代码路径迟早分叉，而分叉的后果是
//! 用户点确认之后发生的事与他刚看过的不一样——那比没有预览更糟。
//!
//! 匹配按 **IRI** 不按 key：上游改一次 `rdfs:label`，派生出的 key 就变了，
//! 按 key 匹配会把同一个类当新类建出来，实体全留在孤儿上（见 0001 P2）。

use std::collections::{BTreeMap, HashMap, HashSet};

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
    /// key 被另一个 IRI 占着，但对齐表说那两个是**同一个东西** → 跳过且不算冲突。
    /// 与 KeyTaken 分开，是因为它不需要人裁：少建一个重复的类正是想要的结果
    Aligned,
}

/// 一个属性在这次导入里会不会被建出来，以及为什么。
///
/// **预览必须说得出为什么**。上一版只报了"解析到 54 个属性"，读者无从判断
/// 那是"都会建"还是"一个都不建"——而实际上是后者。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "detail")]
pub enum AttrNote {
    /// 会建，用这个 datatype
    Datatype(&'static str),
    /// 会建成 text：没写 range，词汇表没做声明，text 是诚实的超集
    NoRange,
    /// 会建成 text：range 是可读字面量但我们表达不了它的类型（`xsd:time` 等）。
    /// 报出原 IRI，人可以改成更合适的类型；但值先落下来，不能因为类型糙就丢知识
    DegradedToText(String),
    /// 不建：抽取器不可能从散文里读出这种值（二进制、XML 片段、XML 内部标识）。
    /// 建了也永远填不上，只会给每个文本块的提示词多一行死噪音
    UnusableRange(String),
    /// 不建：没写 domain。属性必须挂在一个类上，这是 store 层的硬约束
    NoDomain,

    /// 不建：domain 指向的类**在这个文件里，但被跳过了**（多半是 key 撞车）。
    /// 与 UnknownDomain 分开报，因为处置不同：这个能通过给现有的类改名解开
    DomainSkipped(String),
    /// 不建：domain 指向一个这个文件里压根没有的类（外部词汇表）
    UnknownDomain(String),
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
    /// 属性专用：会以什么 datatype 建，或为什么建不了
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attr: Option<AttrNote>,
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

/// 属性的去向。**domain 先判**：没有 domain 的属性根本建不出来，
/// 这时它的 range 是什么已经不重要了。
fn attr_note(
    p: &ontology_rdf::OwlProperty,
    vocab: &ontology_rdf::VocabDatatypes,
    resolvable: &HashSet<&str>,
    in_file: &HashSet<&str>,
) -> AttrNote {
    if p.domains.is_empty() {
        return AttrNote::NoDomain;
    }
    // **一个都解析不出来才算失败**：多个 domain 里有一部分被跳过时，
    // 属性仍然建，只是少挂几个类——比整条丢掉强，而被跳过的那些类
    // 在计划里各自报告过 key 撞车
    if !p.domains.iter().any(|d| resolvable.contains(d.as_str())) {
        let first = &p.domains[0];
        return if in_file.contains(first.as_str()) {
            AttrNote::DomainSkipped(first.clone())
        } else {
            AttrNote::UnknownDomain(first.clone())
        };
    }
    match ontology_rdf::map_range_of(p, vocab) {
        ontology_rdf::RangeMapping::Datatype(dt) => AttrNote::Datatype(dt),
        ontology_rdf::RangeMapping::Absent => AttrNote::NoRange,
        ontology_rdf::RangeMapping::Degraded(iri) => AttrNote::DegradedToText(iri),
        ontology_rdf::RangeMapping::Unusable(iri) => AttrNote::UnusableRange(iri),
    }
}

/// 属性会不会被建出来，以及用什么 datatype。落库与统计共用它，
/// 免得"预览说会建"和"实际建了"两处各判一次而分叉。
fn attr_datatype(note: &AttrNote) -> Option<&'static str> {
    match note {
        AttrNote::Datatype(dt) => Some(dt),
        // 没写 range，或写了但类型表达不了：值仍是字面量，
        // text 是拦不住任何东西的诚实超集
        AttrNote::NoRange | AttrNote::DegradedToText(_) => Some("text"),
        _ => None,
    }
}

impl ImportPlan {
    /// 建不出来的属性，按原因分组计数。**预览里报总数是不够的**——
    /// "54 个属性"读不出那是都会建还是一个都不建，而实际上取决于原因。
    pub fn attr_skips(&self) -> BTreeMap<&str, usize> {
        let mut out: BTreeMap<&str, usize> = BTreeMap::new();
        for a in &self.attributes {
            if a.disposition == Disposition::KeyTaken {
                *out.entry("key_taken").or_default() += 1;
                continue;
            }
            let reason = match a.attr.as_ref() {
                Some(AttrNote::Datatype(_))
                | Some(AttrNote::NoRange)
                | Some(AttrNote::DegradedToText(_)) => continue,
                Some(AttrNote::UnusableRange(_)) => "unusable_range",
                Some(AttrNote::NoDomain) => "no_domain",
                Some(AttrNote::DomainSkipped(_)) => "domain_skipped",
                Some(AttrNote::UnknownDomain(_)) => "unknown_domain",
                None => continue,
            };
            *out.entry(reason).or_default() += 1;
        }
        out
    }
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
        utopia_core::AppError::invalid_detail(
            "bad_ontology_file",
            "Could not parse this ontology file",
            e.to_string(),
        )
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

    // 这次结束后能解析出 id 的类 IRI：文件里新建或更新的，加上库里已有同 IRI 的。
    // key 撞车被跳过的**不在其列**——它不会被建出来，挂在它上面的属性也就无处可挂
    // 本次导入内部也会撞 key：不同 IRI 派生出同一个短标签
    //（FOAF 的 familyName 与 family_name 都成了 family_name）。
    // 只对着库里查是不够的——那样第二个在预览里显示"会新建"，
    // 落库时被 ON CONFLICT 悄悄丢掉，预览就说了假话
    //
    // **两个命名空间，不是一个**：类进 entity_types、关系与属性进 relation_types，
    // 各有各的 (kb_id, key) 唯一约束。合成一张表就是凭空多出一条约束——
    // 而且代价具体：schema.org 的 location / address 是属性，却先被
    // OMG Commons 的 Location / Address 两个**类**占了名字（类先处理），
    // 于是抽取时模型点名要 location，库里偏偏没有
    let mut claimed_class: HashMap<&str, &str> = HashMap::new();
    let mut claimed_prop: HashMap<&str, &str> = HashMap::new();

    let mut classes = Vec::new();
    for c in &proj.classes {
        // 对齐表判为"同名不同义"时，用它声明的 key 顶替派生出来的那个
        let mut renamed_key: Option<&'static str> = None;
        let (disposition, conflict_with) = if let Some(prev) = claimed_class.get(c.key.as_str()) {
            (Disposition::KeyTaken, Some((*prev).to_string()))
        } else if e_by_iri.contains_key(c.iri.as_str()) {
            (Disposition::Update, None)
        } else if let Some(existing) = e_by_key.get(c.key.as_str()) {
            match existing.iri.as_deref() {
                // **占位者没有 IRI：认领它。**
                //
                // 没有 IRI 意味着这个类是本地起的名字（种子本体、或者手工建的），
                // 不是另一个词汇表的同名词。导入说的是"这个 IRI 就是那个类"，
                // 而这一步不做，整棵树就是断的：schema.org 的 Organization
                // 撞上内置的 organization 被跳过，于是 Corporation 的父类指向
                // 一个没建出来的东西——内置那几个基类一个子类都挂不上，
                // 类型精化无从谈起。
                None => (Disposition::Update, None),
                // 已经有一个**不同的** IRI：两个词汇表争同一个短标签。
                // 先查预制包的对齐表——那是**声明**的处置，重导入结果一样，
                // 所以下面那条"不自动加后缀"的理由对它不适用
                Some(other) => match crate::pack_alignment::lookup(&c.iri, other) {
                    // 同义：已有的那个就是它，少建一个重复的类正是想要的
                    Some(crate::pack_alignment::Alignment::SameAs) => {
                        (Disposition::Aligned, existing.iri.clone())
                    }
                    // 同名不同义：换一个声明好的 key 建出来
                    Some(crate::pack_alignment::Alignment::Rename(k)) => {
                        renamed_key = Some(k);
                        (Disposition::Create, None)
                    }
                    // 表里没有：这是真冲突。不自动加后缀——那会让重导入
                    // 认不出自己上次建的是哪个
                    None => (Disposition::KeyTaken, existing.iri.clone()),
                },
            }
        } else {
            (Disposition::Create, None)
        };
        // 改名与原名都是借用（前者 'static，后者借自 proj），统一成一个引用用完再落地
        let key: &str = renamed_key.unwrap_or(c.key.as_str());
        if !matches!(disposition, Disposition::KeyTaken | Disposition::Aligned) {
            claimed_class.insert(key, c.iri.as_str());
        }
        classes.push(PlannedItem {
            iri: c.iri.clone(),
            key: key.to_string(),
            label: c.label.clone(),
            has_description: !c.description.trim().is_empty(),
            disposition,
            functional: false,
            conflict_with,
            attr: None,
        });
    }

    // 文件里出现过的类（含被跳过的）——用来区分它被跳过了与它压根不在这个文件里
    let in_file: HashSet<&str> = proj.classes.iter().map(|c| c.iri.as_str()).collect();
    let resolvable: HashSet<&str> = classes
        .iter()
        .filter(|c| c.disposition != Disposition::KeyTaken)
        .map(|c| c.iri.as_str())
        .chain(e_by_iri.keys().copied())
        .collect();

    let mut relations = Vec::new();
    let mut attributes = Vec::new();
    for p in &proj.properties {
        let (disposition, conflict_with) = if let Some(prev) = claimed_prop.get(p.key.as_str()) {
            (Disposition::KeyTaken, Some((*prev).to_string()))
        } else if r_by_iri.contains_key(p.iri.as_str()) {
            (Disposition::Update, None)
        } else if let Some(existing) = r_by_key.get(p.key.as_str()) {
            (Disposition::KeyTaken, existing.iri.clone())
        } else {
            (Disposition::Create, None)
        };
        if disposition != Disposition::KeyTaken {
            claimed_prop.insert(p.key.as_str(), p.iri.as_str());
        }
        let mut item = PlannedItem {
            iri: p.iri.clone(),
            key: p.key.clone(),
            label: p.label.clone(),
            has_description: !p.description.trim().is_empty(),
            disposition,
            functional: p.functional,
            conflict_with,
            attr: None,
        };
        if p.is_datatype {
            item.attr = Some(attr_note(p, &proj.vocab_datatypes, &resolvable, &in_file));
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

/// 执行计划。属性在类之后落库——它们要挂在 domain 上，而 domain 要等
/// 类先建好并解析 IRI → id（就是下面那个 `id_of`）。
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
    // 已在库里的类：IRI → id。Aligned 要靠它把"同义的那个"接上父类引用
    let existing_by_iri: HashMap<String, Uuid> =
        utopia_store::graph::entity_types(&state.pool, kb_id)
            .await?
            .into_iter()
            .filter_map(|t| t.iri.clone().map(|i| (i, t.id)))
            .collect();
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
                    // **不是 `c.key`**：对齐表判为同名不同义时用的是改过的那个
                    &item.key,
                    &c.label,
                    &c.description,
                    &c.iri,
                )
                .await?;
                id_of.insert(c.iri.clone(), id);
                created_classes += 1;
            }
            Disposition::Update => {
                // 两种 Update：这个 IRI 上次导过（按 IRI 找得到），
                // 或者它要认领一个同名的本地类（按 key 找，且那一行还没有 IRI）
                let updated = utopia_store::ontology::update_type_from_import(
                    &state.pool,
                    kb_id,
                    &c.iri,
                    &c.label,
                    &c.description,
                )
                .await?;
                let id = match updated {
                    Some(id) => Some(id),
                    None => {
                        utopia_store::ontology::adopt_iri_onto_key(
                            &state.pool,
                            kb_id,
                            &c.key,
                            &c.iri,
                        )
                        .await?
                    }
                };
                if let Some(id) = id {
                    id_of.insert(c.iri.clone(), id);
                    updated_classes += 1;
                }
            }
            // key 被占：报告过了，不动
            // 同义：不建。但要把 IRI 指向已有那个类的 id，否则以它为父类的
            // 子类会解析不到，整棵树在这里断掉
            Disposition::Aligned => {
                if let Some(target) = item.conflict_with.as_deref() {
                    if let Some(id) = existing_by_iri.get(target) {
                        id_of.insert(c.iri.clone(), *id);
                    }
                }
            }
            Disposition::KeyTaken => {}
        }
    }

    // 父类第二遍解析：第一遍时父类可能还没建出来
    for c in &proj.classes {
        let Some(&child) = id_of.get(&c.iri) else {
            continue;
        };
        // **全部父类**，不再只取第一个。FOAF 的 Person 同时是 Agent 与
        // SpatialThing，丢掉后一支就让 domain 在那支上的属性判定不过。
        // 指向没被建出来的类的那些父自然落选——少一支比整条不挂强
        let parents: Vec<Uuid> = c
            .parents
            .iter()
            .filter_map(|iri| id_of.get(iri).copied())
            .collect();
        if !parents.is_empty() {
            // 成环时报错而不是中断整次导入：环是上游词汇表的问题，
            // 而这一次导入的其余部分仍然值得落地
            let _ = utopia_store::ontology::set_parents(&state.pool, kb_id, child, &parents).await;
        }
    }

    let by_prop_iri: HashMap<&str, &_> = proj
        .properties
        .iter()
        .map(|p| (p.iri.as_str(), p))
        .collect();
    // 关系。此前 apply 完全跳过它们——预览却在关系那行写着"N new"，
    // 承诺了不会发生的事。这与今天修掉的 key 撞车缺陷是同一种。
    //
    // **functional / inverse_functional 照词汇表的声明写下去**：它们是时态引擎
    // 自动闭合事实的依据，猜错会成批造假冲突（part_of 那次 59 条）。所以不猜——
    // 词汇表说是就是，预览已经把它们单独列出来让人过目。
    let mut created_rels = 0usize;
    let mut updated_rels = 0usize;
    for item in &plan.relations {
        if item.disposition == Disposition::KeyTaken {
            continue;
        }
        let Some(p) = by_prop_iri.get(item.iri.as_str()) else {
            continue;
        };
        // domain/range 指向没被建出来的类时只丢那一个，不丢整条关系：
        // 关系不像属性那样必须挂在类上，没有 domain 就是"不限主语类型"
        let resolve = |iris: &[String]| -> Vec<Uuid> {
            iris.iter().filter_map(|i| id_of.get(i).copied()).collect()
        };
        let domains = resolve(&p.domains);
        let ranges = resolve(&p.ranges);

        if item.disposition == Disposition::Update {
            if utopia_store::ontology::update_relation_from_import(
                &state.pool,
                kb_id,
                &p.iri,
                &p.label,
                &p.description,
                &domains,
                &ranges,
            )
            .await?
            {
                updated_rels += 1;
            }
            continue;
        }
        if utopia_store::ontology::create_relation_with_iri(
            &state.pool,
            kb_id,
            &p.key,
            &p.label,
            &p.description,
            &p.iri,
            p.functional,
            p.inverse_functional,
            &domains,
            &ranges,
        )
        .await?
        .is_some()
        {
            created_rels += 1;
        }
    }

    // 属性：类建完、id_of 填好之后才轮到它们。计划里已经算出了每个属性的去向，
    // **这里只执行，不重新判断**——两处各判一次就会分叉，而分叉意味着
    // 预览说的和实际做的不是一回事
    let mut created_attrs = 0usize;
    for item in &plan.attributes {
        if item.disposition == Disposition::KeyTaken {
            continue;
        }
        let (Some(note), Some(p)) = (item.attr.as_ref(), by_prop_iri.get(item.iri.as_str())) else {
            continue;
        };
        let Some(dt) = attr_datatype(note) else {
            continue;
        };
        // 计划阶段判为可解析、落库时却没有 id 的（类被跳过或更新失败）自然落选；
        // 全落选就不建，而不是造一个挂空的属性
        let domain_ids: Vec<Uuid> = p
            .domains
            .iter()
            .filter_map(|iri| id_of.get(iri).copied())
            .collect();
        if domain_ids.is_empty() {
            continue;
        }
        if utopia_store::ontology::create_attribute_with_iri(
            &state.pool,
            kb_id,
            &p.key,
            &p.label,
            &p.description,
            &p.iri,
            &domain_ids,
            dt,
        )
        .await?
        .is_some()
        {
            created_attrs += 1;
        }
    }

    let summary = serde_json::json!({
        "classes_created": created_classes,
        "classes_updated": updated_classes,
        "classes_key_taken": plan.classes.iter().filter(|c| c.disposition == Disposition::KeyTaken).count(),
        "relations_seen": plan.relations.len(),
        "relations_created": created_rels,
        "relations_updated": updated_rels,
        "attributes_seen": plan.attributes.len(),
        "attributes_created": created_attrs,
        "attributes_skipped": plan.attr_skips(),
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
    // 本体刚变过，向量都是陈的。**排任务而不是就地跑**：这一趟要嵌几千行，
    // 放在导入请求里，用户点完确认要多等六到八分钟。排不上也不要紧——
    // 索引是自愈的，下一个用到检索的人会补上
    let _ = utopia_store::jobs::enqueue(
        &state.pool,
        "embed_ontology",
        serde_json::json!({ "kb_id": kb_id }),
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
