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

use crate::{ontology_index, AppState};
use utopia_core::AppResult;
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
    let _ = ontology_index::refresh(state, kb_id).await;
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
