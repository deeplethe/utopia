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

/// 一个实体的消解建议（preview 用，不写库）。
#[derive(Debug, serde::Serialize)]
pub struct TypeSuggestion {
    pub entity_id: Uuid,
    pub name: String,
    pub coarse: String,
    pub proposed_type: Option<String>,
    pub fact_count: i64,
    /// 送去检索的那段字。**回给调用方**——检索找不着的时候，
    /// 第一个要看的就是"我们拿什么去找的"
    pub profile: String,
    pub candidates: Vec<utopia_core::models::TypeCandidate>,
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
        out.push(TypeSuggestion {
            entity_id: s.id,
            name: s.canonical_name.clone(),
            coarse: s.coarse_key.clone(),
            proposed_type: s.proposed_type.clone(),
            fact_count: s.fact_count,
            profile: profiles[i].clone(),
            candidates,
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
    if let Some(p) = s
        .proposed_type
        .as_deref()
        .map(str::trim)
        .filter(|x| !x.is_empty())
    {
        parts.push(p.to_string());
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
