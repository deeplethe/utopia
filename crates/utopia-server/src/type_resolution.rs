//! 类型消解：把挂在倾倒场类下的实体，精化到本体里具体的类。
//!
//! **为什么能事后做，而谓词不能**（0001 的那个不对称）：类型是挂在节点上的
//! 注解，可以等证据攒够了再贴；谓词就是事实本身，`(NVIDIA, ?, Mellanox)`
//! 根本不是一条事实。所以谓词走"先记原词、后映射"，类型走"先粗后精"。
//!
//! 消解手里的东西跟抽取当场完全不同：
//!
//! 1. **`proposed_type`**——抽取时模型自己报的类型名，词表里没有才留下来。
//!    最强的一条，因为任务从"读懂这是什么"变成了"本体里哪个类叫这个意思"。
//! 2. **实体画像**——它参与的全部谓词，跨文档累积。
//! 3. **证据引文**——同一批句子，但聚在一起，且不必跟几十个别的实体抢注意力。
//!
//! 候选**两路来**，取并集不合分数：一路拿画像搜类的描述，一路拿语境向量搜
//! 已定类的实体、把它们的类当票投。第二路绕开了第一路的软肋（中文画像对
//! 英文样板描述），而且库越大越准。两路的距离一个在类空间一个在实体空间，
//! 合成一个排序是自欺——何况实测同一路的距离都不能跨实体比。
//!
//! 粗类的后代**排在前面**，但不是唯一能选的——两套本体的分类轴常常不重合
//! （schema.org 把软件挂在 CreativeWork 下，而抽取给的粗类是 product）。
//! 该说不的是裁决那一步，它看得到描述也看得到粗类。

use crate::{llm_util, ontology_index, AppState};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 倾倒场类的 key。挂在它下面的实体是"没判出来"，不是"判成了这个"。
const DUMPING_GROUND: &str = "concept";
/// 一轮看多少个实体。够一次人工过目，也够看出检索准不准。
const BATCH: i64 = 60;
/// 每个实体检索几个候选。给裁决看的，不是让它在一堆勉强相关里硬挑一个。
const CANDIDATES: i64 = 8;
/// 看几个已定类的近邻。少了凑不出票数，多了尾巴上全是噪音。
const NEIGHBOURS: i64 = 10;

/// 一个实体的消解建议（preview 用，不写库）。
#[derive(Debug, serde::Serialize)]
pub struct TypeSuggestion {
    pub entity_id: Uuid,
    pub name: String,
    pub coarse: String,
    pub proposed_type: Option<String>,
    pub specific_type: Option<String>,
    pub fact_count: i64,
    /// 送去检索的那段字。**回给调用方**——检索找不着的时候，
    /// 第一个要看的就是"我们拿什么去找的"
    pub profile: String,
    /// 第一路候选：画像 → 类的描述
    pub candidates: Vec<utopia_core::models::TypeCandidate>,
    /// 第二路候选：语境相似的已定类实体，按类投票
    pub neighbours: Vec<NeighbourVote>,
}

/// 近邻投出来的一个类。
///
/// **不跟 `candidates` 合成一个排序**：两路的距离一个在类空间、一个在实体空间，
/// 本来就不可比——何况实测同一路的距离都不能跨实体比。并集交给裁决，
/// 各自标明来源。附带的好处是这一路的证据人能读懂：
///「像 Milvus，而 Milvus 标的是 software_application」比「余弦 0.49」有用得多。
#[derive(Debug, serde::Serialize)]
pub struct NeighbourVote {
    pub key: String,
    /// 有几个近邻是这个类
    pub votes: usize,
    /// 最近的那个近邻有多近
    pub best_distance: f64,
    /// 投票的实体名，给人看的证据
    pub examples: Vec<String>,
    /// 这些近邻是不是**全部**来自同一批文档。
    /// 是的话这一票要打折：只出现一次的实体，语境向量就是那一块的向量，
    /// 同文档的实体自然互相成为近邻，而那不是类型证据
    pub same_document_only: bool,
}

/// 只算不写：每个待消解实体的画像与候选。
///
/// 独立成一步是有意的——在花力气建裁决之前，先回答"检索到底找不找得到"。
/// 找不到的话，裁决做得再好也没有用。
pub async fn preview(state: &AppState, kb_id: Uuid) -> AppResult<Vec<TypeSuggestion>> {
    // 只补类那一半：这一步用不到关系，而一份大本体的关系有一千多条
    let _ = ontology_index::refresh_scoped(
        state,
        kb_id,
        Some(utopia_store::ontology::TypeKind::Entity),
    )
    .await;
    let subjects = utopia_store::resolution::entities_for_type_resolution(
        &state.pool,
        kb_id,
        DUMPING_GROUND,
        BATCH,
    )
    .await?;
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let profiles: Vec<String> = subjects.iter().map(profile_of).collect();
    let per_entity = ontology_index::nearest_for_each(
        state,
        kb_id,
        &profiles,
        // 多取一些再按祖先过滤：过滤在检索之后，所以要留出被滤掉的余量
        CANDIDATES * 4,
        ontology_index::Target::Class,
    )
    .await
    .unwrap_or_default();

    let mut out = Vec::with_capacity(subjects.len());
    for (i, s) in subjects.iter().enumerate() {
        // 粗类的后代**排前面，但不是唯一能选的**。
        //
        // 起初这里是硬闸门（只许往后代走），实测 17 个实体里挡掉 4 个正确答案：
        // schema.org 把 SoftwareApplication 与 Periodical 都挂在 CreativeWork 下，
        // 而抽取给的粗类是 product / organization——两套分类的轴根本不重合，
        // 硬拦就是把正确答案永久锁在门外。跟 part_of 那次同一条教训：
        // **签名是导向，不是闸门**；系统性丢数据比偶尔判错贵得多。
        //
        // 该说不的是裁决那一步：它看得到描述、看得到粗类，能说出"这不是"。
        let descendants: std::collections::HashSet<_> =
            utopia_store::resolution::descendants_of(&state.pool, kb_id, s.coarse_id)
                .await?
                .into_iter()
                .collect();
        let mut ranked: Vec<_> = per_entity
            .get(i)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.id != s.coarse_id)
            .collect();
        // 稳定排序：后代提前，同一档内保持检索给的距离序
        ranked.sort_by_key(|c| !descendants.contains(&c.id));
        let candidates: Vec<_> = ranked.into_iter().take(CANDIDATES as usize).collect();
        // 第二路：语境相似的已定类实体，按类投票
        let raw = utopia_store::resolution::nearest_typed_entities(
            &state.pool,
            kb_id,
            s.id,
            DUMPING_GROUND,
            NEIGHBOURS,
        )
        .await?;
        let mut votes: std::collections::BTreeMap<String, (usize, f64, Vec<String>, bool)> =
            std::collections::BTreeMap::new();
        for (name, _tid, key, distance, same_doc) in raw {
            let slot = votes
                .entry(key)
                .or_insert_with(|| (0, distance, Vec::new(), true));
            slot.0 += 1;
            slot.1 = slot.1.min(distance);
            if slot.2.len() < 3 {
                slot.2.push(name);
            }
            slot.3 &= same_doc;
        }
        let mut neighbours: Vec<NeighbourVote> = votes
            .into_iter()
            .filter(|(key, _)| *key != s.coarse_key)
            .map(
                |(key, (votes, best_distance, examples, same_document_only))| NeighbourVote {
                    key,
                    votes,
                    best_distance,
                    examples,
                    same_document_only,
                },
            )
            .collect();
        neighbours.sort_by(|a, b| {
            b.votes
                .cmp(&a.votes)
                .then(a.best_distance.total_cmp(&b.best_distance))
        });

        out.push(TypeSuggestion {
            entity_id: s.id,
            name: s.canonical_name.clone(),
            coarse: s.coarse_key.clone(),
            proposed_type: s.proposed_type.clone(),
            specific_type: s.specific_type.clone(),
            fact_count: s.fact_count,
            profile: profiles[i].clone(),
            candidates,
            neighbours,
        });
    }
    Ok(out)
}

/// 实体画像：送去做向量检索的那段字。
///
/// **模型自己报的类型名放最前面。** 它是最强的信号，而检索匹配的是类的
/// `label + description`——一个类名对一段类定义，比一串谓词对一段类定义近得多。
///
/// 名字、别名其次；谓词与引文垫后当语境。
fn profile_of(s: &utopia_store::resolution::TypeCandidateSubject) -> String {
    let mut parts: Vec<String> = Vec::new();
    // 模型自己的说法放最前面，两个都有就都写。它们是**名字**，而检索的目标
    //（类的 label）也是名字——名字对名字，正是这个索引擅长的形状
    for named in [s.specific_type.as_deref(), s.proposed_type.as_deref()] {
        if let Some(p) = named.map(str::trim).filter(|x| !x.is_empty()) {
            parts.push(p.to_string());
        }
    }
    parts.push(s.canonical_name.clone());
    if !s.aliases.is_empty() {
        parts.push(s.aliases.join(", "));
    }
    if !s.roles.is_empty() {
        parts.push(s.roles.join(" "));
    }
    // 引文垫后当语境，且只有两句：它们是关于这个实体的句子没错，但也带着
    // 一堆跟类型无关的东西（时间、数字、别的实体），放多了会把画像的重心
    // 从"这是什么"拖到"这段文字讲了什么"。查询侧已经保证只取主语位的
    for q in s.quotes.iter().take(2) {
        parts.push(q.chars().take(120).collect());
    }
    parts.join(". ")
}

/// 一条裁决结果。
#[derive(Debug, serde::Deserialize)]
struct Verdict {
    /// 实体名，用来对回 preview 里的那一条
    name: String,
    /// 选中的类 key；判不出来时为空
    #[serde(default)]
    choice: Option<String>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct VerdictReply {
    #[serde(default)]
    verdicts: Vec<Verdict>,
}

/// 高于这条线才自动改类，其余进提案让人点。
///
/// 定在 0.85 而不是更低：改类**不进时间轴**（它是 entities 上的一次 UPDATE 加
/// 一行账），所以错了不会在实体历史里留下一条刺眼的记录，也就没那么容易被
/// 发现。可撤销不等于会被撤销——没人看见的错不会有人去撤。
const AUTO_THRESHOLD: f32 = 0.85;

/// 一次消解的结果，给调用方交代清楚三档各去了哪里。
#[derive(Debug, serde::Serialize)]
pub struct ResolutionOutcome {
    pub batch: Option<Uuid>,
    /// 自动改掉的
    pub retyped: u32,
    /// 有候选但置信度不够，留给人的
    pub for_review: Vec<ReviewItem>,
    /// 裁决说"都不是"的。**这一档必须报出来**：它跟"没跑"长得一样，
    /// 而两者的含义完全相反
    pub left_alone: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct ReviewItem {
    pub entity_id: Uuid,
    pub name: String,
    pub coarse: String,
    pub choice: String,
    pub confidence: f32,
    pub reason: Option<String>,
}

/// 跑一遍类型消解：检索候选 → 裁决 → 三档处置。
pub async fn resolve(state: &AppState, kb_id: Uuid) -> AppResult<ResolutionOutcome> {
    let items = preview(state, kb_id).await?;
    let items: Vec<_> = items
        .into_iter()
        .filter(|i| !i.candidates.is_empty() || !i.neighbours.is_empty())
        .collect();
    if items.is_empty() {
        return Ok(ResolutionOutcome {
            batch: None,
            retyped: 0,
            for_review: Vec::new(),
            left_alone: 0,
        });
    }

    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| AppError::invalid("no_chat_model", "Chat model not configured"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| AppError::invalid("no_chat_model", "Chat model not configured"))?;

    let reply = client
        .chat(&[utopia_llm::ChatMessage {
            role: "user".into(),
            content: adjudication_prompt(&items),
        }])
        .await
        .map_err(AppError::Other)?;
    let block = utopia_extract::json_block(&reply).map_err(AppError::Other)?;
    let parsed: VerdictReply =
        serde_json::from_str(&block).map_err(|e| AppError::Other(e.into()))?;

    // 名字对回实体，同时把每个实体允许的 key 收起来——模型偶尔会答一个
    // 候选之外的 key（本体里可能根本没有），那种不该落库
    let by_name: std::collections::HashMap<&str, &TypeSuggestion> =
        items.iter().map(|i| (i.name.as_str(), i)).collect();
    let mut picks: Vec<(Uuid, Uuid)> = Vec::new();
    let mut for_review: Vec<ReviewItem> = Vec::new();
    let mut left_alone = 0usize;
    for v in &parsed.verdicts {
        let Some(item) = by_name.get(v.name.as_str()) else {
            continue;
        };
        let Some(choice) = v.choice.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
            left_alone += 1;
            continue;
        };
        // 只认候选清单里的 key。清单之外的答案不是"更好的判断"，
        // 是模型在凭记忆写一个 schema.org 里的名字——本体里未必有
        let Some(target) = item.candidates.iter().find(|c| c.key == choice) else {
            left_alone += 1;
            continue;
        };
        let confidence = v.confidence.unwrap_or(0.0);
        if confidence >= AUTO_THRESHOLD {
            picks.push((item.entity_id, target.id));
        } else {
            for_review.push(ReviewItem {
                entity_id: item.entity_id,
                name: item.name.clone(),
                coarse: item.coarse.clone(),
                choice: choice.to_string(),
                confidence,
                reason: v.reason.clone(),
            });
        }
    }
    // 裁决压根没提到的实体也算"没动"，否则三档加起来对不上总数
    left_alone += items.len().saturating_sub(
        parsed
            .verdicts
            .iter()
            .filter(|v| by_name.contains_key(v.name.as_str()))
            .count(),
    );

    let (batch, retyped) = if picks.is_empty() {
        (None, 0)
    } else {
        let (b, n) = utopia_store::resolution::retype_entities(&state.pool, kb_id, &picks).await?;
        (Some(b), n)
    };
    Ok(ResolutionOutcome {
        batch,
        retyped,
        for_review,
        left_alone,
    })
}

/// 裁决提示词。
///
/// **"都不是"必须是个体面的答案。** 这跟 `related_to` 那个逃生舱正好相反：
/// 那里兜底选项销毁信息，所以撤掉；这里保持粗类什么也不损失——实体照样在图上、
/// 事实照样挂着，只是没变得更具体。硬逼模型从候选里挑一个，换来的是一批
/// 自信的错误，而且它们不进时间轴、不容易被看见。
fn adjudication_prompt(items: &[TypeSuggestion]) -> String {
    let mut blocks = Vec::new();
    for it in items {
        let mut lines = vec![format!(
            "### {}\ncurrently: {}\nthe extractor called it: {}\nseen as: {}",
            it.name,
            it.coarse,
            it.specific_type.as_deref().unwrap_or("-"),
            it.profile.chars().take(200).collect::<String>()
        )];
        lines.push("candidates:".into());
        for c in &it.candidates {
            let d = c.description.trim();
            lines.push(if d.is_empty() {
                format!("- {} ({})", c.key, c.label)
            } else {
                format!("- {}: {d}", c.key)
            });
        }
        if !it.neighbours.is_empty() {
            let n: Vec<String> = it
                .neighbours
                .iter()
                .take(3)
                .map(|n| format!("{} (like {})", n.key, n.examples.join(", ")))
                .collect();
            lines.push(format!("similar entities are typed: {}", n.join("; ")));
        }
        blocks.push(lines.join("\n"));
    }
    format!(
        "You are refining entity types in a knowledge graph. Each entity below already has a \
         broad type and a list of candidate narrower types retrieved from the ontology.\n\
         \n\
         For each entity choose ONE candidate key, or null.\n\
         \n\
         Choose null whenever any of these hold, and expect null to be a common answer:\n\
         - no candidate actually means the thing (the list is retrieved by similarity, so it \
           usually contains near-misses and sometimes contains nothing right at all);\n\
         - the candidate is not narrower than what it already has;\n\
         - the entity is not a thing of that kind at all — a quantity, a capability, a phrase.\n\
         Keeping the broad type loses nothing: the entity and its facts stay exactly as they \
         are, merely less specific. Picking a wrong narrow type is worse than picking none, \
         because it reads as a decided fact.\n\
         \n\
         confidence is your own 0~1: use above 0.85 only when the candidate's definition \
         plainly describes this entity, not when it is merely the closest of a weak list.\n\
         \n\
         {}\n\
         \n\
         Output exactly one JSON object:\n\
         {{\"verdicts\":[{{\"name\":\"entity name exactly as given\",\"choice\":\"candidate key or null\",\"confidence\":0.0,\"reason\":\"one short clause\"}}]}}",
        blocks.join("\n\n")
    )
}
