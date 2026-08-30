//! 类型消解：把实体的类型改对——**往细里走，也把走错的掰回来**。
//!
//! 起初只有前半句：抽取给粗类，消解往它的后代精化。抽取开始按分块检索候选
//! 之后，它自己就会挑细类，也就会挑错（实测 `绍兴 → address`、
//! `慢病管理小程序 → entry_point`）。而正确答案是错类的**兄弟**不是它的后代，
//! 于是被"候选必须更细"那条规则挡在门外——那条规则是我自己写进提示词的。
//!
//! 现在两个方向都认。纠正天然判为跨轴，所以一定进人工：推翻抽取的判断比
//! 细化它风险大，不该自动发生。
//!
//! 0009 之后还有第三种输入，而且是最常见的那种：**根本还没有类**。删掉兜底类
//! 之后，本体装不下的实体不再被塞进 `concept`，而是 `type_id IS NULL`。这一档
//! 身上没有"抽取的判断"要推翻，给它定类是补齐不是重新分类，所以**不判跨轴**、
//! 高置信度直接落库——否则删掉哨兵的代价就是每个实体都要人看一眼。
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
    /// 现在挂着的类，**可能没有**（0009）
    pub coarse: Option<String>,
    /// 现类的描述。裁决要判"现在这个类对不对"，光看 key 不够
    pub coarse_description: Option<String>,
    /// 粗类的 id。裁决要拿它跟目标类配成一对，去查"这一对人认可过没有"
    #[serde(skip)]
    pub coarse_id: Option<Uuid>,
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
    /// 粗类的全部后代。裁决据此分档：选中的类在里面 = 往下走一格，
    /// 不在 = 换了分类轴。**不序列化**——它是给分档用的，不是给人看的
    #[serde(skip)]
    pub descendants: std::collections::HashSet<Uuid>,
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
    let subjects =
        utopia_store::resolution::entities_for_type_resolution(&state.pool, kb_id, BATCH).await?;
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    // **每个实体两个查询，不是一个。**
    //
    // 画像里模型自己的说法排在最前，但后面整段是语境，一样进向量。实测
    //「杭州拱墅区」的画像是 `district. 杭州拱墅区. located_in by 仁和堂连锁药房.
    // 仁和堂连锁药房在杭州拱墅区开设了第 40 家门店`——整段讲的是药房，
    // 于是候选给回 pharmacy、store，而 administrative_area 一次都没上来。
    // 名字被稀释进了段落。
    //
    // 所以名字单独发一次：短查询对短标签，正是这个索引擅长的形状；
    // 画像那一次仍然发，它照顾没有 specific_type 的实体和需要语境才判得出的。
    // 两次结果取并集——多一次嵌入，换掉一整类漏检。
    let profiles: Vec<String> = subjects.iter().map(profile_of).collect();
    let names: Vec<Option<String>> = subjects.iter().map(name_query_of).collect();
    let mut queries: Vec<String> = Vec::with_capacity(subjects.len() * 2);
    // 每个实体在 queries 里占的下标：(画像, 名字)
    let mut slots: Vec<(usize, Option<usize>)> = Vec::with_capacity(subjects.len());
    for (p, n) in profiles.iter().zip(&names) {
        let pi = queries.len();
        queries.push(p.clone());
        let ni = n.as_ref().map(|q| {
            queries.push(q.clone());
            queries.len() - 1
        });
        slots.push((pi, ni));
    }
    let hits = ontology_index::nearest_for_each(
        state,
        kb_id,
        &queries,
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
        // 还没有类的实体没有"粗类的后代"这个轴可用（0009），整张类表都是候选，
        // 排序就纯按检索顺序来
        let descendants: std::collections::HashSet<_> = match s.coarse_id {
            Some(c) => utopia_store::resolution::descendants_of(&state.pool, kb_id, c)
                .await?
                .into_iter()
                .collect(),
            None => std::collections::HashSet::new(),
        };
        // **两路交替取，不按距离合并。**
        //
        // 距离在两路之间不可比：短查询（"医药集团"）产生的距离系统性地小于
        // 一整段画像，按距离排就等于让名字那一路独占前几名。实测这么做之后
        // 仁和医药集团、国家药监局、中华医学会全被挤掉了正确候选——
        // 上一轮它们是自动通过的。跟 A/B 两路那条"取并集不合分数"同一条，
        // 我在这儿违反了它。
        //
        // 交替之后两路各占一半席位，谁的距离数值大小不再影响谁被看见。
        let (pi, ni) = slots[i];
        let lists: Vec<Vec<_>> = [Some(pi), ni]
            .into_iter()
            .flatten()
            .map(|idx| hits.get(idx).cloned().unwrap_or_default())
            .collect();
        let mut seen_ids: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
        let mut ranked: Vec<utopia_core::models::TypeCandidate> = Vec::new();
        let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
        for rank in 0..longest {
            for list in &lists {
                let Some(c) = list.get(rank) else { continue };
                if Some(c.id) == s.coarse_id || !seen_ids.insert(c.id) {
                    continue;
                }
                ranked.push(c.clone());
            }
        }
        // 稳定排序：后代提前，同一档内保持距离序
        ranked.sort_by_key(|c| !descendants.contains(&c.id));
        let candidates: Vec<_> = ranked.into_iter().take(CANDIDATES as usize).collect();
        // 第二路：语境相似的已定类实体，按类投票
        let raw =
            utopia_store::resolution::nearest_typed_entities(&state.pool, kb_id, s.id, NEIGHBOURS)
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
            .filter(|(key, _)| Some(key) != s.coarse_key.as_ref())
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
            coarse_description: s.coarse_description.clone(),
            coarse_id: s.coarse_id,
            proposed_type: s.proposed_type.clone(),
            specific_type: s.specific_type.clone(),
            fact_count: s.fact_count,
            profile: profiles[i].clone(),
            candidates,
            neighbours,
            descendants,
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
///
/// 这一段是**语境查询**，跟 [`name_query_of`] 那个短查询各发一次、结果取并集。
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
    /// 提示词里那条的编号——**对回 preview 的钥匙**。
    ///
    /// 从前是靠 `name`。0009 之后同名的未分类实体可以并存（NULL ≠ NULL，见该篇
    /// 的唯一索引一节），实测一个库里 4 个「张伟」：按名字对回来会把它们塌成
    /// 同一条，同一个 entity_id 被推进 picks 四次,落库时撞 (batch_id, entity_id)
    /// 主键；而另外三个永远不会被定类——它们对这条路根本不可见
    #[serde(default)]
    id: Option<usize>,
    /// 实体名。模型漏给 id 时的退路,且名字不重复时它足够
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

/// 低于这条线的一律进人工，无论它落在哪。
///
/// **它不是灰区的主判据**：实测模型自报的 confidence 是双峰的——15 条全 ≥0.85、
/// 4 条 null，中间一个都没有。自报置信度是文风不是概率，模型挑的是一个
/// 跟自己语气相称的数字。拿它当闸门，闸门什么也拦不住。
/// 真正分档的是下面那条"跨没跨分类轴"，这条只兜住偶尔出现的低分。
const AUTO_THRESHOLD: f32 = 0.85;

/// 一次消解的结果，给调用方交代清楚三档各去了哪里。
#[derive(Debug, serde::Serialize)]
pub struct ResolutionOutcome {
    pub batch: Option<Uuid>,
    /// 自动改掉的
    pub retyped: u32,
    /// 跨了分类轴、或置信度不够，留给人的
    pub for_review: Vec<ReviewItem>,
    /// 裁决说"都不是"的，**连同它给的理由**。
    ///
    /// 只报一个数是不够的：这一步的整个设计押在"选择都不是是个体面答案"上，
    /// 而那就是最大的一档——不记理由，最大的那一档就是不透明的。
    /// 跟本体导入预览那条"必须说得出为什么"是同一条。
    pub left_alone: Vec<DeclineNote>,
}

#[derive(Debug, serde::Serialize)]
pub struct DeclineNote {
    pub name: String,
    pub coarse: Option<String>,
    pub specific_type: Option<String>,
    /// 模型给的理由；它压根没提到这个实体时为空
    pub reason: Option<String>,
    /// 检索给的头一个候选。理由说不通时，看这个就知道是检索没找着
    /// 还是裁决没看上
    pub top_candidate: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ReviewItem {
    pub entity_id: Uuid,
    pub name: String,
    pub coarse: Option<String>,
    /// 配对的两端。认可这一条时认可的是**这一对类**，不是这一个实体。
    /// 起点可能没有（0009）——那时没有"一对类"可认可，只能一个个改
    pub from_type_id: Option<Uuid>,
    pub to_type_id: Uuid,
    pub choice: String,
    pub confidence: f32,
    pub reason: Option<String>,
    /// 选中的类**不在**粗类的子树里——它换的是分类轴，不是往下走一格
    pub crosses_axis: bool,
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
            left_alone: Vec::new(),
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

    // 名字 → 下标**列表**，不是单条。同名的未分类实体可以并存（0009），
    // 塌成一条会让其中几个永远拿不到裁决。裁决优先按 id 对回来，
    // 名字只是模型漏给 id 时的退路
    let mut by_name: std::collections::HashMap<&str, Vec<usize>> = std::collections::HashMap::new();
    for (i, it) in items.iter().enumerate() {
        by_name.entry(it.name.as_str()).or_default().push(i);
    }
    // 人认可过的配对：同一对不再进人工。跨轴是类与类之间的事，
    // 实体只是碰巧撞上它——第二个城市不该再问一遍
    let approved = utopia_store::resolution::approved_refinements(&state.pool, kb_id).await?;
    let mut picks: Vec<(Uuid, Uuid)> = Vec::new();
    let mut for_review: Vec<ReviewItem> = Vec::new();
    let mut left_alone: Vec<DeclineNote> = Vec::new();
    let mut decided: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let decline = |item: &TypeSuggestion, reason: Option<String>| DeclineNote {
        name: item.name.clone(),
        coarse: item.coarse.clone(),
        specific_type: item.specific_type.clone(),
        reason,
        top_candidate: item.candidates.first().map(|c| c.key.clone()),
    };
    for v in &parsed.verdicts {
        // id 优先；漏给时退回名字，取该名字下**还没裁决过的**第一条。
        // 越界的 id 当没给——模型偶尔会编一个
        let idx = v.id.filter(|i| *i < items.len()).or_else(|| {
            by_name
                .get(v.name.as_str())
                .and_then(|ids| ids.iter().find(|i| !decided.contains(i)).copied())
        });
        let Some(idx) = idx else { continue };
        // 同一条只认第一份裁决。模型重复作答时，第二份会把同一个 entity_id
        // 再推进 picks 一次，落库撞主键
        if !decided.insert(idx) {
            continue;
        }
        let item = &items[idx];
        // **字符串 "null" 也是 null。** 模型时而给 JSON null、时而给这四个字母，
        // 而当成 key 去查候选必然查不到，于是这条被记成"选了个候选之外的 key"——
        // 一条编造的拒绝理由盖掉了模型真正给的那条。拒绝的理由是这一步最要紧的
        // 输出，被自己的解析弄脏比没有更糟
        let Some(choice) = v.choice.as_deref().map(str::trim).filter(|c| {
            !c.is_empty() && !c.eq_ignore_ascii_case("null") && !c.eq_ignore_ascii_case("none")
        }) else {
            left_alone.push(decline(item, v.reason.clone()));
            continue;
        };
        // 只认候选清单里的 key。清单之外的答案不是"更好的判断"，
        // 是模型在凭记忆写一个 schema.org 里的名字——本体里未必有
        let Some(target) = item.candidates.iter().find(|c| c.key == choice) else {
            left_alone.push(decline(
                item,
                Some(format!("chose {choice}, which is not among the candidates")),
            ));
            continue;
        };
        let confidence = v.confidence.unwrap_or(0.0);
        // **分档看跨没跨分类轴，不看模型自报的那个数字。**
        //
        // 在粗类的子树里 = 往下走一格，抽取的判断没被推翻，自动改。
        // 不在 = 换了一条分类轴（判成 product 的东西落到了 CreativeWork 底下），
        // 那是重新分类而不是精化，值得一个人看一眼。实测那一轮唯一明确的错
        //（《中国数据智能》→ publication_issue）正是这一类。
        //
        // **纠正也走这里，而且天然如此**：抽取挑错细类之后（绍兴 → address），
        // 正确答案是它的兄弟而不是它的后代，所以一定判为跨轴、一定进人工。
        // 这正是想要的——推翻抽取的判断比细化它风险大，不该自动发生。
        //
        // **还没有类的实体不算跨轴**（0009）：它身上没有一个"抽取的判断"要被
        // 推翻，第一次给它定类是补齐而不是重新分类。这里若判成跨轴，等于
        // 删掉兜底类之后每一个实体都要人看一遍，整条自动化就废了
        let crosses_axis = match item.coarse_id {
            Some(from) => {
                !item.descendants.contains(&target.id) && !approved.contains(&(from, target.id))
            }
            None => false,
        };
        if confidence >= AUTO_THRESHOLD && !crosses_axis {
            picks.push((item.entity_id, target.id));
        } else {
            for_review.push(ReviewItem {
                entity_id: item.entity_id,
                name: item.name.clone(),
                coarse: item.coarse.clone(),
                from_type_id: item.coarse_id,
                to_type_id: target.id,
                choice: choice.to_string(),
                confidence,
                reason: v.reason.clone(),
                crosses_axis,
            });
        }
    }
    // 裁决压根没提到的实体也算"没动"，否则三档加起来对不上总数
    for (i, item) in items.iter().enumerate() {
        if !decided.contains(&i) {
            left_alone.push(decline(item, None));
        }
    }

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
    // **编号是钥匙,名字不是**（0009）。同名的未分类实体可以并存,一个库里
    // 实测有 4 个「张伟」——按名字对回来会把它们塌成同一条
    for (i, it) in items.iter().enumerate() {
        // 现类连描述一起给：要判"现在这个类对不对"，光看 key 不够——
        // 导入本体的 key 常常自解释不了（`entry_point` 是什么？）
        //
        // 没有类的实体直说没有（0009）。这一行从前一定填着 concept，模型读到的是
        // 「它已经是个概念」——一个错误的先验；现在读到的是「还没定」，正是实情
        let current = match (&it.coarse, it.coarse_description.as_deref().map(str::trim)) {
            (Some(k), Some(d)) if !d.is_empty() => format!("{k} ({d})"),
            (Some(k), _) => k.clone(),
            (None, _) => "not yet typed".into(),
        };
        let mut lines = vec![format!(
            "### [{}] {}\ncurrently: {}\nthe extractor called it: {}\nseen as: {}",
            i,
            it.name,
            current,
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
        "You are fixing entity types in a knowledge graph. Each entity below has a type it was \
         given during extraction, and a list of candidate types retrieved from the ontology.\n\
         \n\
         For each entity choose ONE candidate key, or null. There are two reasons to choose \
         a candidate, and they are different:\n\
         \n\
         **Narrowing** — the current type is right but broad, and a candidate says the same \
         thing more precisely (organization → hospital).\n\
         \n\
         **Correcting** — the current type is simply wrong, and a candidate is right. This \
         happens because extraction picks from a retrieved shortlist and can pick badly: a city \
         typed as an address, an app typed as an entry point. A correcting candidate is \
         usually a sibling of the current type rather than a narrower version of it, so do not \
         withhold it on the grounds that it is not more specific. Say plainly in the reason \
         that the current type is wrong; a person will see this one before it is applied.\n\
         \n\
         Choose null whenever any of these hold, and expect null to be a common answer:\n\
         - no candidate actually means the thing (the list is retrieved by similarity, so it \
           usually contains near-misses and sometimes contains nothing right at all);\n\
         - the current type is already right and no candidate is more precise;\n\
         - the entity is not a thing of that kind at all — a quantity, a capability, a phrase.\n\
         Keeping the current type loses nothing: the entity and its facts stay exactly as they \
         are. Picking a wrong type is worse than picking none, because it reads as a decided \
         fact.\n\
         \n\
         confidence is your own 0~1: use above 0.85 only when the candidate's definition \
         plainly describes this entity, not when it is merely the closest of a weak list.\n\
         \n\
         reason is required on every verdict, including the nulls — especially the nulls. \
         When you choose null, say which candidate came closest and what it got wrong \
         (\"nearest was publication_issue, but that is one issue of a journal, not the \
         journal\"). A refusal without a reason cannot be acted on: nobody can tell whether \
         the ontology is missing the class, or the search failed to surface it, or you read \
         the entity differently.\n\
         \n\
         {}\n\
         \n\
         id is the number in the heading, and it is what identifies the verdict — not the \
         name. Two entries can carry the same name and still be different things (two people \
         called Zhang Wei, each with their own facts); judge each one on its own block and \
         give one verdict per id you answer. Never merge two ids into one verdict.\n\
         \n\
         Output exactly one JSON object:\n\
         {{\"verdicts\":[{{\"id\":0,\"name\":\"entity name exactly as given\",\"choice\":\"candidate key or null\",\"confidence\":0.0,\"reason\":\"one short clause\"}}]}}",
        blocks.join("\n\n")
    )
}

/// 只有名字的那个查询：模型自己的说法，别的一概不放。
///
/// **分开发的理由是稀释。** 语境查询里这几个词排在最前，但后面整段一样进向量，
/// 而语境讲的往往是别人：「杭州拱墅区」的引文在讲一家药房，于是候选回来的是
/// pharmacy、store。短查询对短标签没有这个问题——检索目标（类的 label）
/// 本来就是名字。
///
/// 两个说法都没有就返回 `None`：只剩实体名的查询跟语境查询的开头一模一样，
/// 再发一次是白花一次嵌入。
fn name_query_of(s: &utopia_store::resolution::TypeCandidateSubject) -> Option<String> {
    let parts: Vec<&str> = [s.specific_type.as_deref(), s.proposed_type.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join(". "))
}
