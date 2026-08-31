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

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::api::ontology_routes;
use crate::predicate_match::{merge_key, PredicateIndex};
use crate::state::AppState;

/// 少于这么多个够格的信号（谓词 + 类型）就不折腾——凑不出像样的提案，
/// 白烧一次 LLM 调用。
const MIN_SIGNALS: usize = 3;
/// 只采纳出现在这么多篇文档里的说法。**只在一篇里出现过的是那篇文档的用词，
/// 不是这个组织的词汇**——而本体会反馈进抽取提示词，一次偶然会变成长期指令。
/// 副作用正好：只有一篇文档时什么都够不着门槛，于是什么也不做，下一篇再试。
const MIN_DOCS: i64 = 2;

/// 一组按屈折基归并的说法——采纳后它们是同一个关系。
struct RelationGroup {
    /// 规范 key：组里事实最多的那个说法。**不问模型取名**——组里每个说法都是
    /// 原文真实出现过的措辞，挑最常见的那个比编一个新词更贴语料
    key: String,
    /// 组里全部说法，采纳时一并改写
    forms: Vec<String>,
    facts: i64,
    docs: usize,
    /// 本体里已经有等价关系时的落点：`(关系 id, 主宾要不要对调)`。
    /// `None` = 本体里确实没有，按票数新建
    existing: Option<(Uuid, bool)>,
}

/// 按票数决定采纳哪些关系。**不调模型。**
///
/// 三步：把说法按屈折基归并（`sued` 与 `sues` 是同一个关系）、算文档并集、过门槛。
///
/// 并集不能用相加：同一篇文档完全可能两种写法都用过，相加会让一篇文档把一个说法
/// 顶过「≥2 篇」。所以要 `proposed_predicate_documents` 拿到真正的文档 id。
///
/// 输出**排过序**——这条路的价值有一半在于确定性，而 HashMap 的遍历顺序不是。
async fn counted_relation_groups(
    state: &AppState,
    kb_id: Uuid,
) -> anyhow::Result<Vec<RelationGroup>> {
    let forms = utopia_store::graph::proposed_predicates(&state.pool, kb_id).await?;
    let pairs = utopia_store::graph::proposed_predicate_documents(&state.pool, kb_id).await?;
    // **建之前先问本体。**
    //
    // 少了这一步，采纳只按票数建，从不检查「是不是已经有等价的了」。实测后果：
    // demo-b3 那个库里 `produced_by` 与 `produces`、`developed_by` 与 `develops`
    // 各成一个关系，同一件事的两个方向永久分家。
    //
    // 而 `produces` 有 265 条、`produced_by` 只有 15 条——票多的先进本体，
    // 票少的那个本该被 `predicate_match` 的 `_by` 规则接住，却因为**采纳路径压根
    // 没走匹配器**而长成了独立关系。匹配器只在抽取时用过，这里是它缺席的第二处。
    let rtypes = utopia_store::graph::relation_types(&state.pool, kb_id).await?;
    let index = PredicateIndex::build(&rtypes);

    let mut docs_of: HashMap<String, HashSet<Uuid>> = HashMap::new();
    for (form, doc) in pairs {
        docs_of.entry(form).or_default().insert(doc);
    }

    let mut grouped: HashMap<Vec<String>, Vec<utopia_core::models::ProposedPredicate>> =
        HashMap::new();
    for f in forms {
        grouped.entry(merge_key(&f.form)).or_default().push(f);
    }

    let mut out = Vec::new();
    for (_, mut members) in grouped {
        // 事实多的在前，同数按字典序——规范 key 的选取不能依赖 HashMap 顺序
        members.sort_by(|a, b| b.fact_count.cmp(&a.fact_count).then(a.form.cmp(&b.form)));
        let mut docs: HashSet<Uuid> = HashSet::new();
        for m in &members {
            if let Some(d) = docs_of.get(&m.form) {
                docs.extend(d);
            }
        }
        if (docs.len() as i64) < MIN_DOCS {
            continue;
        }
        // 组里任一说法能落到本体已有关系上，整组就落过去。同组说法共享屈折基，
        // 结尾有没有 `by` 也必然一致（`produced_by` 与 `produces` 不同组），
        // 所以「要不要对调」是**整组一致**的，不必逐条判
        let existing = members.iter().find_map(|m| index.lookup(&m.form));
        out.push(RelationGroup {
            key: members[0].form.clone(),
            facts: members.iter().map(|m| m.fact_count).sum(),
            forms: members.into_iter().map(|m| m.form).collect(),
            docs: docs.len(),
            existing,
        });
    }
    out.sort_by(|a, b| b.facts.cmp(&a.facts).then(a.key.cmp(&b.key)));
    Ok(out)
}

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

    // **关系不问模型，按票数采纳。**
    //
    // 从前这一步把候选交给 LLM，让它回答"哪些值得建成关系"。那个问题数据已经
    // 答了——`runs_on` 出现在 8 篇文档、13 条事实里，不是判断题。而模型实测答错：
    // 它漏掉了 `runs_on`，却采纳了只在一篇里出现过的 `pledged_capital`。
    //
    // 换成计数还有一个副作用是关键的：**这一段变成确定性的**。同一份语料重跑得到
    // 同一个本体，测量台第一次能对它做对照。之前 B 与 B3 两组差 3 个百分点，
    // 到底是修复起了作用还是跑次方差，答不上来，就因为中间夹着一次 LLM 调用。
    //
    // 模型没有被撤走，只是换了个问题：见下方的同义归并——"这批新关系里哪些跟
    // 已有的是同一个意思"。那个才需要理解意义，且答错了 unadopt 一键回退。
    let counted = counted_relation_groups(state, kb_id).await?;
    let proposals = ontology_routes::build_proposals(state, kb_id, "en", MIN_DOCS).await?;
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
            utopia_store::palette::color_for_key(key),
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
                    // 系统的动作,不是谁的决定——与本文件下方审计写 NULL 同一条
                    None,
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

    // 关系：按票数采纳，一组一个关系。
    //
    // key 与 label 都来自语料自己的措辞，description 留空——它进抽取提示词当语义
    // 指引，而这里没有可信的来源可写。编一句反而是往提示词里塞一个没人负责的断言，
    // 而 key 本身（`runs_on`、`available_on`）已经说清楚了。
    //
    // temporal 一律 state：它只在 functional / inverse_functional 为真时驱动时态
    // 引擎，而这条路**永不**自动设那两位（见文件头），所以此处它没有行为后果。
    for g in &counted {
        // 本体里已经有等价关系就不新建，直接把事实改写过去。
        // 被动形（`produced_by` 对上 `produces`）改写时主宾对调
        let (predicate_id, swap) = match g.existing {
            Some(hit) => hit,
            None => {
                let label = g.key.replace('_', " ");
                match utopia_store::ontology::create_relation_type(
                    &state.pool,
                    kb_id,
                    &g.key,
                    &label,
                    "state",
                    // 冷启动不替人声明任何公理：推理机的判据必须是人写下来的
                    Default::default(),
                    "",
                    "relation",
                    &[],
                    &[],
                    None,
                    None,
                )
                .await
                {
                    Ok(id) => {
                        added_relations.push(g.key.clone());
                        (id, false)
                    }
                    Err(e) => {
                        tracing::warn!(%kb_id, key = %g.key, error = %e, "冷启动建关系失败");
                        continue;
                    }
                }
            }
        };
        let (batch, moved) = utopia_store::graph::adopt_proposed_predicates(
            &state.pool,
            kb_id,
            predicate_id,
            &g.forms,
            swap,
        )
        .await?;
        tracing::info!(
            %kb_id, key = %g.key, forms = ?g.forms, docs = g.docs, facts = g.facts, moved,
            reused = g.existing.is_some(), swap,
            "按票数采纳关系"
        );
        moved_total += moved;
        if moved > 0 {
            batches.push(batch);
        }
        for form in &g.forms {
            let _ =
                utopia_store::ontology::clear_miss(&state.pool, kb_id, "relation_type", form).await;
        }
    }

    // 属性那一档：宾语是字面值的说法。
    //
    // domain 不从提案里读——它从这些事实的主语类型里取，见 adopt_attribute。
    // 值换不动的那些不改写，继续没有谓词，等下一次
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
        // LLM 的 map_to 提案同样可能混着主动与被动，一个 swap 标志伺候不了
        //（同人工路径，见 ontology_routes 里那段注释）
        let (batch, moved) = utopia_store::graph::adopt_proposed_predicates(
            &state.pool,
            kb_id,
            predicate_id,
            &forms,
            false,
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
    match utopia_store::resolution::sweep_proposed_types(&state.pool, kb_id, None).await {
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
