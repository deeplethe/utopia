//! 自动扩本体：抽取遇到本体外的说法时，把它补进本体并改写等它的那些事实。
//!
//! 新建的库不种任何关系，起点是用户挑的本体包（0008）或者空着。包再大也装不下
//! 一份语料的全部说法：落不上本体的事实谓词留空、原词记在证据上（0010），
//! 图上这些边只有原文措辞、没有词表语义，直到有人坐下来点 Suggest、看提案、逐条 Add。
//! 这个模块把那件事自动化：说法够普遍就建成关系，并把等着它的事实改写过去。
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
    let mut rtypes = utopia_store::graph::relation_types(&state.pool, kb_id).await?;
    rtypes.retain(|r| !utopia_store::names::is_name_attribute(r));
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
    fold_by_meaning(state, kb_id, &mut out).await;
    out.sort_by(|a, b| b.facts.cmp(&a.facts).then(a.key.cmp(&b.key)));
    Ok(out)
}

/// 拼写对不上、意思对得上的，也不新建（#560）。
///
/// `PredicateIndex` 只认写法、屈折与被动；`comprised` 对 `has_member`、`ceo` 对
/// `chief_executive` 它看不出来，于是本体里长出第二个关系，图上同一件事两种谓词，
/// 查询按谓词分组时它们各站一边。这里把没落地的说法嵌一次，与已有关系的向量比，
/// 够近的就落到那一条上。阈值取得紧：折错一次是把两个关系永久并成一个，
/// 而不折只是多一个关系，后者便宜得多。每个候选的距离都进日志，好在真语料上校
const FOLD_DISTANCE: f32 = 0.20;

async fn fold_by_meaning(state: &AppState, kb_id: Uuid, groups: &mut [RelationGroup]) {
    let unmatched: Vec<usize> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.existing.is_none())
        .map(|(i, _)| i)
        .collect();
    if unmatched.is_empty() {
        return;
    }
    let Ok(kb) = utopia_store::kbs::get(&state.pool, kb_id).await else {
        return;
    };
    let Ok(Some(settings)) = utopia_store::settings::get(&state.pool, kb.workspace_id).await else {
        return;
    };
    let Some(client) = crate::llm_util::embed_client(&settings) else {
        return;
    };
    let texts: Vec<String> = unmatched
        .iter()
        .map(|i| groups[*i].key.replace('_', " "))
        .collect();
    let vectors = match client.embed(&texts).await {
        Ok(v) if v.len() == texts.len() => v,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!(%kb_id, error = %e, "说法嵌入失败，按拼写采纳");
            return;
        }
    };
    for (i, vec) in unmatched.into_iter().zip(vectors) {
        let near = utopia_store::ontology::nearest_relation_types(
            &state.pool,
            kb_id,
            &vec,
            3,
            Some("relation"),
        )
        .await
        .unwrap_or_default();
        for c in &near {
            tracing::debug!(%kb_id, form = %groups[i].key, key = %c.key, distance = c.distance, "同义候选");
        }
        if let Some(hit) = fold_pick(&near, FOLD_DISTANCE) {
            tracing::info!(
                %kb_id, form = %groups[i].key, onto = %hit.key, distance = hit.distance,
                "说法按意思落到已有关系上"
            );
            groups[i].existing = Some((hit.id, false));
        }
    }
}

/// 最近的一个在阈值内就是它；候选按距离升序来
fn fold_pick(
    near: &[utopia_core::models::TypeCandidate],
    max_distance: f32,
) -> Option<&utopia_core::models::TypeCandidate> {
    near.iter()
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
        .filter(|c| c.distance <= max_distance)
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
    /* **字面值那一档也要算进来。**
    `proposed_predicates` 只数宾语是实体的事实（它数的是"采纳要改写的东西"），
    于是一个满是数额的语料在这里算出 predicates=0，整个自动扩本体被跳过，
    下面那段建属性的代码一次都跑不到。实测：12 条带着 `valuation`、
    `investment_amount` 等说法的值事实在库里等着，日志里只有一行
    「够格的信号太少」。它们同样是"够不够一次 LLM 调用的量"的信号 */
    let value_forms: Vec<_> = utopia_store::graph::proposed_attributes(&state.pool, kb_id)
        .await?
        .into_iter()
        .filter(|f| f.doc_count >= MIN_DOCS)
        .collect();
    if forms.len() + types.len() + value_forms.len() < MIN_SIGNALS {
        tracing::debug!(
            %kb_id, predicates = forms.len(), types = types.len(),
            values = value_forms.len(),
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
    let mut left_off_total = 0u32;
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
    /* **temporal 从提案里取，不再一律 state。**
    这里原来写死 state，理由是「它只在 functional / inverse_functional 为真时
    驱动时态引擎，而这条路永不自动设那两位」。那个理由**在 0031（#486）之后
    已经不成立**：`Validity::under` 现在按谓词的 temporal 规整每一次写入，
    event 会把 `valid_to` 收到与 `valid_from` 同一刻。写死 state 的后果是
    `Meridian invested Kestrel 2025-05-09` 被记成「从那天起一直在投」，
    时间轴上读出来就是这样——而它是一件发生过的事，不是一个持续的状态。

    答案本来就在手边：提案那一步问模型要的 JSON 里就有
    `"temporal":"state|event|eternal"`，而下面的类、属性、map_to 都在用同一份
    提案，唯独关系这一档把它丢了。**采纳与否仍旧只由票数决定**（0007），
    模型只回答「这个关系是哪一种」——那是计数答不了的意义问题。 */
    let temporal_of = proposed_temporal(&proposals);
    for g in &counted {
        // 本体里已经有等价关系就不新建，直接把事实改写过去。
        // 被动形（`produced_by` 对上 `produces`）改写时主宾对调
        let (predicate_id, swap) = match g.existing {
            Some(hit) => hit,
            None => {
                let label = g.key.replace('_', " ");
                let temporal = temporal_of(&g.key, &g.forms);
                match utopia_store::ontology::create_relation_type(
                    &state.pool,
                    kb_id,
                    &g.key,
                    &label,
                    temporal,
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
        let utopia_store::graph::Adopted {
            batch_id: batch,
            moved,
            left_off,
            corrected,
        } = utopia_store::graph::adopt_proposed_predicates(
            &state.pool,
            kb_id,
            predicate_id,
            &g.forms,
            swap,
        )
        .await?;
        tracing::info!(
            %kb_id, key = %g.key, forms = ?g.forms, docs = g.docs, facts = g.facts, moved, left_off,
            corrected, reused = g.existing.is_some(), swap,
            temporal = temporal_of(&g.key, &g.forms),
            "按票数采纳关系"
        );
        moved_total += moved;
        left_off_total += left_off;
        if moved > 0 {
            batches.push(batch);
        }
        for form in &g.forms {
            let _ =
                utopia_store::ontology::clear_miss(&state.pool, kb_id, "relation_type", form).await;
        }
    }

    // 新建的关系要有向量：抽取按分块检索候选靠它，谓词对齐（#558）靠它，下一次
    // 同义归并也靠它。从前 `embed_ontology` 只在导入时跑一次，冷启动建的关系
    // 永远没有向量——这个库里 76 个，`comprised` 是其中之一
    if !added_relations.is_empty() {
        if let Err(e) = crate::ontology_index::refresh_scoped(
            state,
            kb_id,
            Some(utopia_store::ontology::TypeKind::Relation),
        )
        .await
        {
            tracing::warn!(%kb_id, error = %e, "新建关系的向量没补上");
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
        let utopia_store::graph::Adopted {
            batch_id: batch,
            moved,
            left_off,
            corrected,
        } = utopia_store::graph::adopt_proposed_predicates(
            &state.pool,
            kb_id,
            predicate_id,
            &forms,
            false,
        )
        .await?;
        tracing::info!(
            %kb_id, key, forms = ?forms, moved, left_off, corrected,
            "按映射提案采纳关系"
        );
        left_off_total += left_off;
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
            // 签名两边都对不上、没挂上谓词的（#190）。不报这个数，「改写了 N 条」就是报喜不报忧
            "facts_left_off": left_off_total,
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

/// 从提案里查一个说法的 temporal，查不到给 `state`。
///
/// 按 key 查，再按 forms 查——计数那条路的规范 key 取的是组里事实最多的那个说法，
/// 模型提案挑的未必是同一个（`acquires` 对 `acquired`），但它们同组，说的是一件事。
///
/// **查不到就 state**，与从前一致：这一档只在有明确答案时才偏离缺省，
/// 而 state 是三者里最保守的——它不会像 event 那样把 `valid_to` 收成一个点。
fn proposed_temporal(proposals: &serde_json::Value) -> impl Fn(&str, &[String]) -> &'static str {
    let mut by_word: HashMap<String, &'static str> = HashMap::new();
    if let Some(list) = proposals.get("relation_types").and_then(|v| v.as_array()) {
        for p in list {
            let t = match str_of(p, "temporal") {
                Some("event") => "event",
                Some("eternal") => "eternal",
                // 模型偶尔编第四个值；不认识的一律当没说
                _ => continue,
            };
            let mut words: Vec<String> = str_of(p, "key").map(str::to_string).into_iter().collect();
            if let Some(forms) = p.get("forms").and_then(|v| v.as_array()) {
                words.extend(forms.iter().filter_map(|f| f.as_str()).map(str::to_string));
            }
            for w in words {
                by_word.insert(w.to_lowercase(), t);
            }
        }
    }
    move |key: &str, forms: &[String]| {
        std::iter::once(key)
            .chain(forms.iter().map(String::as_str))
            .find_map(|w| by_word.get(&w.to_lowercase()).copied())
            .unwrap_or("state")
    }
}

fn str_of<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use utopia_core::models::TypeCandidate;

    /// 一个关系是发生过一次，还是持续成立。
    ///
    /// 从前这里写死 `state`，于是「Meridian 2025-05-09 投了 Kestrel」被记成
    /// 从那天起一直在投。答案本来就在提案里，只是没人读。
    #[test]
    fn a_relation_takes_the_temporal_it_was_proposed_with() {
        let proposals = serde_json::json!({
            "relation_types": [
                { "key": "acquired", "temporal": "event", "forms": ["acquires", "acquisition_of"] },
                { "key": "capital_of", "temporal": "eternal", "forms": [] },
                { "key": "employs", "temporal": "state", "forms": ["employed_by"] },
                // 模型偶尔编一个第四值：当没说，落回缺省
                { "key": "sponsors", "temporal": "ongoing", "forms": [] },
            ]
        });
        let t = proposed_temporal(&proposals);

        assert_eq!(t("acquired", &[]), "event");
        // 规范 key 取的是组里事实最多的说法，未必是模型挑的那个——按 forms 也要查得到
        assert_eq!(t("acquisition_of", &[]), "event");
        assert_eq!(t("Acquires", &[]), "event", "大小写不该影响");
        assert_eq!(
            t("bought", &["acquires".into()]),
            "event",
            "本名查不到时看同组说法"
        );
        assert_eq!(t("capital_of", &[]), "eternal");
        assert_eq!(t("employs", &[]), "state");

        // **查不到一律 state**：三者里最保守的，不会像 event 那样把 valid_to 收成一个点
        assert_eq!(t("sponsors", &[]), "state", "不认识的值当没说");
        assert_eq!(t("never_proposed", &[]), "state");
        assert_eq!(
            proposed_temporal(&serde_json::json!({}))("anything", &[]),
            "state"
        );
    }

    fn cand(key: &str, distance: f32) -> TypeCandidate {
        TypeCandidate {
            id: Uuid::now_v7(),
            key: key.into(),
            label: key.into(),
            description: String::new(),
            kind: Some("relation".into()),
            distance,
        }
    }

    /// 只认阈值内最近的那个；差一点也不折——折错比不折贵
    #[test]
    fn a_form_folds_only_onto_a_close_enough_relation() {
        let near = vec![cand("has_member", 0.18), cand("member_of", 0.31)];
        assert_eq!(
            fold_pick(&near, 0.20).map(|c| c.key.as_str()),
            Some("has_member")
        );
        assert!(fold_pick(&near, 0.15).is_none());
        assert!(fold_pick(&[], 0.20).is_none());
    }
}
