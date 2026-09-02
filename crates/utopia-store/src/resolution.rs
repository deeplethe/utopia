//! 实体消解 v2：同名≠同人（设计见 DESIGN.md §4 实体消解 v2）。
//!
//! 漏斗：名字候选召回（免费，含泛用后缀词干互推）→ 画像向量相似度分层（毫秒，复用摄入阶段的 chunk embedding）
//! → 灰区新建实体 + 疑似重复审核项（宁分勿合），LLM 攒批裁决在独立后台任务里跑，
//! 人工终审兜底。LLM 永远不在抽取写入的关键路径上。
//!
//! 类型漂移：同名在同类型下无候选时再查其它类型（同一团队被抽成 organization/
//! project/concept 是常态），按类型对的互斥强度分流——concept 兜底型当召回候选走
//! 画像分层，易混具体类型照建实体但入队审核对，硬互斥（person vs organization）完全分开。

use chrono::{DateTime, Utc};
use pgvector::Vector;
use sqlx::PgPool;
use std::collections::HashSet;
use utopia_core::models::{MergeLogView, ReviewItem, ReviewSide};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 上下文相似度阈值（bge-m3 类模型余弦相似度经验值，后续可调）。
/// ≥ ATTACH 归并到既有实体；< NEW 判为不同实体；中间灰区宁分勿合 + 审核项。
pub const SIM_ATTACH: f32 = 0.55;
pub const SIM_NEW: f32 = 0.35;

/// 名称规范化：全角 ASCII → 半角、全角空格 → 半角、空白折叠。
/// 返回展示形态（保留大小写）；匹配一律再套 SQL lower()。
pub fn normalize_name(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| match c {
            '\u{3000}' => ' ',
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 泛用后缀词表：中文直接拼在词干后；英文按独立词算、首尾两种语序都认
/// （"Phoenix Project" / "Project Phoenix"）。只影响召回，判定仍走画像相似度。
const GENERIC_SUFFIXES_CJK: &[&str] = &["项目", "公司", "集团", "部门", "团队"];
const GENERIC_WORDS_EN: &[&str] = &["project", "corp", "inc", "team"];

/// 词干：剥去一个泛用后缀后的 lower 形态。未命中、剥空、或词干本身就是
/// 泛用词（"项目团队"）时返回 None。输入应已过 normalize_name。
pub fn name_stem(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let strip_punct = |w: &str| w.trim_end_matches(['.', ',']).to_string();
    let generic = |s: &str| {
        GENERIC_SUFFIXES_CJK.contains(&s) || GENERIC_WORDS_EN.contains(&strip_punct(s).as_str())
    };
    for suf in GENERIC_SUFFIXES_CJK {
        if let Some(stem) = lower.strip_suffix(suf) {
            let stem = stem.trim_end();
            if !stem.is_empty() && !generic(stem) {
                return Some(stem.to_string());
            }
        }
    }
    let words: Vec<&str> = lower.split(' ').collect();
    if words.len() >= 2 {
        if generic(words[words.len() - 1]) {
            let stem = words[..words.len() - 1].join(" ");
            if !generic(&stem) {
                return Some(stem);
            }
        }
        if generic(words[0]) {
            let stem = words[1..].join(" ");
            if !generic(&stem) {
                return Some(stem);
            }
        }
    }
    None
}

/// mention 的召回键集合（全 lower）：本名 + 词干 + 词干的泛用后缀增广。
/// 增广覆盖反方向（库内是"星尘项目"、mention 只说"星尘"）；词干含 CJK 拼中文
/// 尾缀，否则拼英文词（两种语序）。键数 ≤10，走 (kb,type,lower(name)) 索引多点查。
pub fn recall_keys(name: &str) -> Vec<String> {
    let lower = name.to_lowercase();
    let base = name_stem(name).unwrap_or_else(|| lower.clone());
    let mut keys = vec![lower];
    fn add(keys: &mut Vec<String>, k: String) {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    add(&mut keys, base.clone());
    if base.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)) {
        for suf in GENERIC_SUFFIXES_CJK {
            add(&mut keys, format!("{base}{suf}"));
        }
    } else {
        for w in GENERIC_WORDS_EN {
            add(&mut keys, format!("{base} {w}"));
            add(&mut keys, format!("{w} {base}"));
        }
    }
    keys
}

// ---------------------------------------------------------------------------
// 类型漂移：同名实体被抽成了不同类型（"Orion platform team" ↔ organization/project）
// ---------------------------------------------------------------------------

/// 易混具体类型：抽取常在这几类间摇摆（一个团队算组织还是项目？平台算项目还是产品？）。
/// 同名跨这组类型 → 照建实体（宁分勿合），但入队审核对交 LLM/人工裁决。
///
/// **这张表该从本体读，今天还没有。** `owl:disjointWith` 已经落库（`entity_type_disjoint`，
/// 有导入有编辑），但消费者只有推理机的本体自检；消解这边仍按这三个硬编码的 key 判。
/// 没装包的库里这三个 key 不存在，于是这一档永不命中，所有跨类型同名判 `Disjoint`——
/// 变严不变松，不会错合。改从本体读是 0016 的 B3。
pub const CONFUSABLE_TYPE_KEYS: &[&str] = &["organization", "project", "product"];

/// 单次消解最多入队的漂移审核对（防同名大组刷爆审核队列）。
const MAX_DRIFT_REVIEWS: usize = 4;

/// 跨类型同名的处置分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeDrift {
    /// 一侧是兜底类型：当召回候选，走画像相似度分层（可 ATTACH）
    Recall,
    /// 两个易混具体类型：新建 + 审核对
    Review,
    /// 硬互斥（person vs organization 等，含未知自定义类型）：完全分开
    Disjoint,
}

fn classify_type_drift(a: Option<&str>, b: Option<&str>) -> TypeDrift {
    // **同一个类型当然可能是同一个东西。**
    //
    // 这一档原本不存在，因为这个函数生来只服务"类型漂移"——同名被抽成两种
    // 类型——那里两边相同根本不会发生。后来 containment_reviews 借它当相容性
    // 判据，而那里**两边相同才是最常见的情形**，于是 person 对 person 落进了
    // 最后那行 Disjoint，被读成"永不可能是同一个东西"。
    //
    // 代价是全文最明显的同指关系一对都进不了队列：福尔摩斯前六篇里
    // Sherlock Holmes 与 Holmes 是两个实体，488 个实体只合并掉 14 个。
    // 原有的 12 个单元测试全在测跨类型，一个都没测相同类型。
    if a == b {
        return TypeDrift::Recall;
    }
    // **一侧还没判出来** → 当召回候选，走画像相似度分层。
    // 从前比的是 `== FALLBACK_TYPE_KEY`，那个 key 已经没有了：
    // 「还没判出来」现在由 `None` 表达（0009）。两边都是 None
    // 已经被上面 `a == b` 那行接住
    let (Some(a), Some(b)) = (a, b) else {
        return TypeDrift::Recall;
    };
    if CONFUSABLE_TYPE_KEYS.contains(&a) && CONFUSABLE_TYPE_KEYS.contains(&b) {
        return TypeDrift::Review;
    }
    TypeDrift::Disjoint
}

fn cosine(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return None;
    }
    Some(dot / (na.sqrt() * nb.sqrt()))
}

#[derive(Debug, sqlx::FromRow)]
struct Candidate {
    id: Uuid,
    profile_embedding: Option<Vector>,
    profile_n: i32,
    degree: i64,
}

/// 消解结果：mention 落到了哪个实体；附带需要入队的疑似重复审核对
/// （同名灰区 / 类型漂移），由调用方写入审核队列并触发裁决任务。
#[derive(Debug)]
pub struct Resolution {
    pub entity_id: Uuid,
    pub created: bool,
    pub reviews: Vec<ReviewRequest>,
}

/// 待入队的审核对：`Resolution::entity_id` vs `other_id`。
#[derive(Debug)]
pub struct ReviewRequest {
    pub other_id: Uuid,
    pub score: f32,
    pub reason: String,
}

/// 单条 mention 消解。`context` 为 mention 所在分块的向量（无 embedding 模型时为 None，
/// 退化为 v1 行为：同名归并到事实最多的候选）。
pub async fn resolve_mention(
    pool: &PgPool,
    kb_id: Uuid,
    // None = 抽取器给的类型不在本体里，或库里根本没有类（0009）
    type_id: Option<Uuid>,
    raw_name: &str,
    context: Option<&[f32]>,
) -> AppResult<Resolution> {
    let name = normalize_name(raw_name);
    // 召回键 = 本名 + 泛用后缀词干及其增广（"星尘"↔"星尘项目"互为候选）。
    // 只扩召回，归并与否仍由下方画像相似度分层定夺。
    let keys = recall_keys(&name);
    let candidates: Vec<Candidate> = sqlx::query_as(
        "SELECT e.id, e.profile_embedding, e.profile_n,
                (SELECT count(*) FROM facts f
                 WHERE (f.subject_id = e.id OR f.object_id = e.id)
                   AND f.invalidated_at IS NULL) AS degree
         FROM entities e
         WHERE e.kb_id = $1 AND e.type_id = $2 AND e.merged_into IS NULL
           AND (lower(e.canonical_name) = ANY($3)
                OR EXISTS (SELECT 1 FROM unnest(e.aliases) a WHERE lower(a) = ANY($3)))",
    )
    .bind(kb_id)
    .bind(type_id)
    .bind(&keys)
    .fetch_all(pool)
    .await?;

    if candidates.is_empty() {
        // 同类型无候选 ≠ 新名字：类型标签会漂（同一团队被抽成 organization/project/
        // concept），先查其它类型下的同名实体，按类型对的互斥强度分流。
        return resolve_type_drift(pool, kb_id, type_id, &name, &keys, context).await;
    }

    let Some(ctx) = context else {
        // 无向量可比：v1 兼容 —— 归并到事实最多的同名候选
        let best = candidates
            .iter()
            .max_by_key(|c| c.degree)
            .expect("non-empty");
        touch_entity(pool, best.id).await?;
        return Ok(Resolution {
            entity_id: best.id,
            created: false,
            reviews: Vec::new(),
        });
    };

    // 有画像的候选算相似度；无画像（历史数据/无 embedding 期创建）单独归类
    let mut best_scored: Option<(&Candidate, f32)> = None;
    let mut unprofiled: Option<&Candidate> = None;
    for c in &candidates {
        match c
            .profile_embedding
            .as_ref()
            .and_then(|p| cosine(p.as_slice(), ctx))
        {
            Some(sim) => {
                if best_scored.map(|(_, s)| sim > s).unwrap_or(true) {
                    best_scored = Some((c, sim));
                }
            }
            None => {
                if unprofiled.map(|u| c.degree > u.degree).unwrap_or(true) {
                    unprofiled = Some(c);
                }
            }
        }
    }

    if let Some((best, sim)) = best_scored {
        if sim >= SIM_ATTACH {
            update_profile(pool, best.id, best.profile_n, ctx).await?;
            return Ok(Resolution {
                entity_id: best.id,
                created: false,
                reviews: Vec::new(),
            });
        }
    }
    if let Some(c) = unprofiled {
        // 无画像候选无从判别：v1 兼容归并，并用本次上下文初始化画像
        update_profile(pool, c.id, c.profile_n, ctx).await?;
        return Ok(Resolution {
            entity_id: c.id,
            created: false,
            reviews: Vec::new(),
        });
    }

    // 走到这里：所有候选都有画像且最高分 < ATTACH → 新建实体（同名不同人）
    let id = create_entity(pool, kb_id, type_id, &name, context).await?;
    refresh_disambiguators(pool, kb_id, &name).await?;
    let mut reviews = best_scored
        .filter(|(_, sim)| *sim >= SIM_NEW)
        .map(|(c, sim)| {
            vec![ReviewRequest {
                other_id: c.id,
                score: sim,
                reason: format!("ambiguous_name|{sim:.2}"),
            }]
        })
        .unwrap_or_default();
    // 名字互相包含的既有实体：等值召回看不见它们（前缀枚举不完），
    // 于是简称会静默变成第二个实体。只入队，不合并
    reviews.extend(containment_reviews(pool, kb_id, type_id, &name, id, context).await?);
    Ok(Resolution {
        entity_id: id,
        created: true,
        reviews,
    })
}

/// 包含关系候选的最短边：两个名字里**较短的那个**至少这么长才算数。
/// 低于它的多是「研究院」「中心」这类通名，配对没有信息量，只会刷爆队列。
const MIN_CONTAIN_CHARS: i32 = 4;

/// 单次最多产出的包含关系审阅对。与 `MAX_DRIFT_REVIEWS` 同理：
/// 一个通名可能包含在几十个实体里，全放进去就把队列淹了。
const MAX_CONTAIN_REVIEWS: usize = 4;

/// SQL 侧多取几行：类型硬互斥的在 Rust 侧才筛得掉，
/// 只取 4 行的话可能 4 行全是互斥类型，真正那一对反而被 LIMIT 切掉。
const CONTAIN_SCAN_LIMIT: i64 = 16;

/// 新建实体之后，找出**名字互相包含**的既有实体，作为审阅候选。
///
/// 中文商业文本在一篇之内就会全称转简称（「星云科技上海研究院」→「上海研究院」），
/// 而 [`recall_keys`] 是等值查：它靠枚举泛用后缀命中反向，可前缀是任意组织名，
/// **枚举不完**。于是这三类全部漏网，静默变成第二个实体：
///
/// ```text
/// 上海研究院       ⊂ 星云科技上海研究院      后缀被限定
/// 启明 X7 加速卡   ~ 启明 X7 推理加速卡      中间插词（靠 LIKE 覆盖不到，见下）
/// 沧海             ⊂ 沧海分布式推理平台 2.0  前缀被扩展
/// ```
///
/// **只出候选，永不自动合并。** 同名候选那条路在相似度 ≥ [`SIM_ATTACH`] 时直接归并，
/// 包含关系绝不能走它：`华瑞集团技术中心` 与 `星云科技技术中心` 都含「技术中心」，
/// 同一篇文档里上下文相似度很容易过线，而它们是两个部门。宁分勿合。
///
/// **别名一并参与**：合并会把名字搬进 `aliases`，只查 `canonical_name` 的话，
/// 每成功合并一次就少一条召回的桥——`Holmes` 并入 `Sherlock Holmes` 之后，
/// 后来的 `Mr. Holmes` 就再也搭不上了（它跟 `Sherlock Holmes` 谁也不含谁）。
/// 合并越成功、漏得越多，是个会自我加剧的洞。
///
/// **已知不覆盖**：两个名字既不互相包含、又没有共同别名做桥的情形
///（`启明 X7 加速卡` 与 `启明 X7 推理加速卡`）。那要三元组相似度，而
/// `CREATE EXTENSION pg_trgm` 需要超级权限——本仓库是受限角色连库（见 `migrations/0010_least_privilege_role.sql`），
/// 装扩展会在部署上失败。留给需要时再说。
///
/// **性能**：反向那半（新名字包含旧名字）用不上任何索引，别名那半同样，
/// 所以这里靠 `kb_id` 收窄行集并设上限，且只在**新建实体时**跑一次，
/// 不是每条 mention。大库上如果不够，正解是建一张「后缀键」表走等值查，
/// 而不是加模糊索引。
async fn containment_reviews(
    pool: &PgPool,
    kb_id: Uuid,
    // None = 抽取器给的类型不在本体里，或库里根本没有类（0009）
    type_id: Option<Uuid>,
    name: &str,
    new_id: Uuid,
    ctx: Option<&[f32]>,
) -> AppResult<Vec<ReviewRequest>> {
    let lower = name.to_lowercase();
    if lower.chars().count() < MIN_CONTAIN_CHARS as usize {
        return Ok(Vec::new());
    }
    // 实体可能还没判出类型（0009），那时没有 key 可查
    let mention_key: Option<String> = match type_id {
        Some(t) => sqlx::query_as::<_, (String,)>("SELECT key FROM entity_types WHERE id = $1")
            .bind(t)
            .fetch_optional(pool)
            .await?
            .map(|(k,)| k),
        None => None,
    };
    // **不按类型过滤**：简称经常掉进 concept 兜底，而全称是具体类型
    //（实测 上海研究院→concept vs 星云科技上海研究院→organization，
    //  启明 X7 加速卡→concept vs 启明 X7 推理加速卡→product），
    //  按类型相等去查，这两对一个都捞不到。相容性交给下面的 classify_type_drift。
    //  多取一些行，因为硬互斥的会在 Rust 侧被筛掉
    // 第三列可空：未分类实体也要参与包含关系扫描（0009）
    let rows: Vec<(Uuid, String, Option<String>, Option<Vector>)> = sqlx::query_as(
        "SELECT e.id, e.canonical_name, t.key, e.profile_embedding
         FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND e.merged_into IS NULL
           AND e.id <> $2
           AND lower(e.canonical_name) <> $3
           AND (
             (char_length(e.canonical_name) >= $4
              AND (lower(e.canonical_name) LIKE '%' || $3 || '%'
                   OR $3 LIKE '%' || lower(e.canonical_name) || '%'))
             -- **别名也要参与召回，否则每合并一次就少一条桥。**
             --
             -- 合并把名字搬进 aliases：Holmes 并入 Sherlock Holmes 之后，
             -- Holmes 那一行 merged_into 非空、被上面第一个条件滤掉了。可十分钟后
             -- 出现的 Mr. Holmes 跟 Sherlock Holmes 谁也不含谁——本来正是靠
             -- Holmes 才桥得上。实测就是这么漏的：同类型那个 bug 修好、Holmes
             -- 正确合并之后，Mr. Holmes 反而永远进不了队列。
             --
             -- 合并越成功，桥拆得越多。这个洞会自我加剧。
             OR EXISTS (
               SELECT 1 FROM unnest(e.aliases) AS alias
               WHERE char_length(alias) >= $4
                 AND (lower(alias) LIKE '%' || $3 || '%'
                      OR $3 LIKE '%' || lower(alias) || '%'))
           )
         ORDER BY char_length(e.canonical_name)
         LIMIT $5",
    )
    .bind(kb_id)
    .bind(new_id)
    .bind(&lower)
    .bind(MIN_CONTAIN_CHARS)
    .bind(CONTAIN_SCAN_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        // 哪些类型对可能指同一个东西，既有规则已经想清楚了，别另发明一套：
        // person vs organization 永不合并，concept 兜底与谁都可能是一个
        .filter(|(_, _, type_key, _)| {
            classify_type_drift(mention_key.as_deref(), type_key.as_deref()) != TypeDrift::Disjoint
        })
        .take(MAX_CONTAIN_REVIEWS)
        .map(|(id, other_name, _, emb)| {
            // 分数只是给队列排序用的参考，**不参与是否合并的判断**——
            // 那个判断本来就不在这条路上
            let score = ctx
                .and_then(|x| emb.as_ref().and_then(|p| cosine(p.as_slice(), x)))
                .unwrap_or(0.0);
            ReviewRequest {
                other_id: id,
                score,
                reason: format!("contains|{other_name}"),
            }
        })
        .collect())
}

#[derive(Debug, sqlx::FromRow)]
struct CrossCandidate {
    id: Uuid,
    canonical_name: String,
    // None = 这个候选还没判出类型（0009）
    type_key: Option<String>,
    // 抽取升格要看它：人说过「就是没有类型」时，那也是一个决定
    type_source: String,
    profile_embedding: Option<Vector>,
    profile_n: i32,
}

/// `reason` 存 code，措辞归界面（docs/decisions/0004）——这一列不该一半 code
/// 一半英文句子，那样中文界面上就是一半能翻一半不能。
fn drift_reason(mention_key: Option<&str>, other_key: Option<&str>, sim: Option<f32>) -> String {
    // 未分类的一侧写成 `(untyped)`。这一列存 code 供界面翻译，
    // 留空会让 `a vs b` 变成 `a vs `，读起来像被截断而不像"没有"
    let a = mention_key.unwrap_or("(untyped)");
    let b = other_key.unwrap_or("(untyped)");
    match sim {
        Some(s) => format!("type_drift|{a} vs {b} {s:.2}"),
        None => format!("type_drift|{a} vs {b}"),
    }
}

fn confusable_reviews(
    // None = 这一侧还没判出类型（0009）
    mention_key: Option<&str>,
    cands: &[&CrossCandidate],
    ctx: Option<&[f32]>,
) -> Vec<ReviewRequest> {
    cands
        .iter()
        .map(|c| {
            let sim = ctx.and_then(|x| {
                c.profile_embedding
                    .as_ref()
                    .and_then(|p| cosine(p.as_slice(), x))
            });
            ReviewRequest {
                other_id: c.id,
                score: sim.unwrap_or(0.0),
                reason: drift_reason(mention_key, c.type_key.as_deref(), sim),
            }
        })
        .collect()
}

/// 同类型召回为空时的跨类型处置（类型漂移）。
/// concept 兜底型候选跑既有画像分层：高相似直接 ATTACH（漂移的召回修复，concept
/// 侧类型升格为具体类型），灰区/无从判别 → 宁分勿合新建 + 审核对；易混具体类型
/// 一律新建 + 审核对（同名 + 类型摇摆本身就是信号，不设相似度门槛）；硬互斥忽略。
async fn resolve_type_drift(
    pool: &PgPool,
    kb_id: Uuid,
    // None = 抽取器给的类型不在本体里，或库里根本没有类（0009）
    type_id: Option<Uuid>,
    name: &str,
    keys: &[String],
    context: Option<&[f32]>,
) -> AppResult<Resolution> {
    // 这一侧也可能还没判出类型（0009），那时没有 key 可查
    let mention_key: Option<String> = match type_id {
        Some(t) => sqlx::query_as::<_, (String,)>("SELECT key FROM entity_types WHERE id = $1")
            .bind(t)
            .fetch_optional(pool)
            .await?
            .map(|(k,)| k),
        None => None,
    };
    let cross: Vec<CrossCandidate> = sqlx::query_as(
        "SELECT e.id, e.canonical_name, t.key AS type_key, e.type_source,
                e.profile_embedding, e.profile_n
         FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id
         -- IS DISTINCT FROM 而不是 <>：后者遇 NULL 返回 NULL，被 WHERE 当假，
         -- 未分类实体会被整个漏掉（0009）
         WHERE e.kb_id = $1 AND e.type_id IS DISTINCT FROM $2 AND e.merged_into IS NULL
           AND (lower(e.canonical_name) = ANY($3)
                OR EXISTS (SELECT 1 FROM unnest(e.aliases) a WHERE lower(a) = ANY($3)))",
    )
    .bind(kb_id)
    .bind(type_id)
    .bind(keys)
    .fetch_all(pool)
    .await?;

    let mut recall_cands: Vec<&CrossCandidate> = Vec::new();
    let mut review_cands: Vec<&CrossCandidate> = Vec::new();
    for c in &cross {
        match classify_type_drift(mention_key.as_deref(), c.type_key.as_deref()) {
            TypeDrift::Recall => recall_cands.push(c),
            TypeDrift::Review => review_cands.push(c),
            TypeDrift::Disjoint => {}
        }
    }

    if let Some(ctx) = context {
        let best = recall_cands
            .iter()
            .filter_map(|c| {
                c.profile_embedding
                    .as_ref()
                    .and_then(|p| cosine(p.as_slice(), ctx))
                    .map(|sim| (*c, sim))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((best, sim)) = best {
            if sim >= SIM_ATTACH {
                update_profile(pool, best.id, best.profile_n, ctx).await?;
                // 候选还没判出类型而这次抽取判出来了 → 升格。
                // 不是合并（没有第二个实体），不入 entity_merges；本体页可手工改回
                //
                // **人说过的「没有类型」不算「还没判出来」。** 0009 之后两者都是
                // NULL，只看 type_key.is_none() 分不出——于是一个人看过、认为
                // 本体里没有合适类的实体，会在下一次抽取时被安上一个类型
                if best.type_key.is_none() && best.type_source != "human" && type_id.is_some() {
                    sqlx::query(
                        "UPDATE entities
                         SET type_id = $2, type_source = 'extracted', updated_at = now()
                         WHERE id = $1",
                    )
                    .bind(best.id)
                    .bind(type_id)
                    .execute(pool)
                    .await?;
                    // 类型标签兜底的消歧后缀可能已过时
                    refresh_disambiguators(pool, kb_id, &best.canonical_name).await?;
                }
                // mention 已定居到召回实体；同名易混类型实体的疑点仍在 → 照常入队
                let mut reviews =
                    confusable_reviews(mention_key.as_deref(), &review_cands, Some(ctx));
                reviews.truncate(MAX_DRIFT_REVIEWS);
                return Ok(Resolution {
                    entity_id: best.id,
                    created: false,
                    reviews,
                });
            }
        }
    }

    let id = create_entity(pool, kb_id, type_id, name, context).await?;
    if !cross.is_empty() {
        // 跨类型同名并存：消歧后缀按名字分组（不分类型），需要刷新
        refresh_disambiguators(pool, kb_id, name).await?;
    }
    let mut reviews = confusable_reviews(mention_key.as_deref(), &review_cands, context);
    for c in &recall_cands {
        let sim = context.and_then(|ctx| {
            c.profile_embedding
                .as_ref()
                .and_then(|p| cosine(p.as_slice(), ctx))
        });
        match sim {
            // 画像明确不像：完全分开，不打扰审核队列
            Some(s) if s < SIM_NEW => {}
            // 灰区或无从判别（无 embedding / 候选无画像）：宁分勿合 + 审核对
            _ => reviews.push(ReviewRequest {
                other_id: c.id,
                score: sim.unwrap_or(0.0),
                reason: drift_reason(mention_key.as_deref(), c.type_key.as_deref(), sim),
            }),
        }
    }
    reviews.truncate(MAX_DRIFT_REVIEWS);
    // 漂移这条路一样会新建实体，包含关系照查
    reviews.extend(containment_reviews(pool, kb_id, type_id, name, id, context).await?);
    Ok(Resolution {
        entity_id: id,
        created: true,
        reviews,
    })
}

async fn create_entity(
    pool: &PgPool,
    kb_id: Uuid,
    // None = 抽取器给的类型不在本体里，或库里根本没有类（0009）
    type_id: Option<Uuid>,
    name: &str,
    context: Option<&[f32]>,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name, profile_embedding, profile_n)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(type_id)
    .bind(name)
    .bind(context.map(|c| Vector::from(c.to_vec())))
    .bind(i32::from(context.is_some()))
    .execute(pool)
    .await?;
    Ok(id)
}

async fn touch_entity(pool: &PgPool, id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE entities SET updated_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 画像增量质心：profile ← (profile·n + ctx) / (n+1)。
/// 维度不匹配（换过 embedding 模型）时用新向量重置画像。
async fn update_profile(pool: &PgPool, id: Uuid, n: i32, ctx: &[f32]) -> AppResult<()> {
    let existing: Option<(Option<Vector>,)> =
        sqlx::query_as("SELECT profile_embedding FROM entities WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let old = existing.and_then(|(v,)| v);
    let (new_vec, new_n) = match old {
        Some(p) if p.as_slice().len() == ctx.len() && n > 0 => {
            let nf = n as f32;
            let merged: Vec<f32> = p
                .as_slice()
                .iter()
                .zip(ctx)
                .map(|(a, b)| (a * nf + b) / (nf + 1.0))
                .collect();
            (merged, n + 1)
        }
        _ => (ctx.to_vec(), 1),
    };
    sqlx::query(
        "UPDATE entities SET profile_embedding = $2, profile_n = $3, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(Vector::from(new_vec))
    .bind(new_n)
    .execute(pool)
    .await?;
    Ok(())
}

/// 同名组展示消歧：组内 ≥2 个存活实体时，各自取最强区分性事实
/// （works_at/part_of/located_in/leads 的宾语名），否则退回类型标签；组内唯一则清空。
pub async fn refresh_disambiguators(pool: &PgPool, kb_id: Uuid, name: &str) -> AppResult<()> {
    let group: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM entities
         WHERE kb_id = $1 AND merged_into IS NULL AND lower(canonical_name) = lower($2)",
    )
    .bind(kb_id)
    .bind(name)
    .fetch_all(pool)
    .await?;

    if group.len() < 2 {
        for (id,) in &group {
            sqlx::query("UPDATE entities SET disambiguator = NULL WHERE id = $1")
                .bind(id)
                .execute(pool)
                .await?;
        }
        return Ok(());
    }

    for (id,) in &group {
        let label: Option<(String,)> = sqlx::query_as(
            "SELECT o.canonical_name FROM facts f
             JOIN relation_types r ON r.id = f.predicate_id
             JOIN entities o ON o.id = f.object_id
             WHERE f.kb_id = $1 AND f.subject_id = $2
               AND f.invalidated_at IS NULL AND f.object_id IS NOT NULL
               AND r.key IN ('works_at', 'part_of', 'located_in', 'leads')
             ORDER BY (r.key = 'works_at') DESC, f.confidence DESC, f.recorded_at DESC
             LIMIT 1",
        )
        .bind(kb_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;
        // 关联事实找不着就退到类型标签；**类型也可能没有**（0009），
        // 那就没有可写的后缀——留 NULL，界面按同名并列显示，别编一个出来
        let disambiguator: Option<String> = match label {
            Some((l,)) => Some(l),
            None => sqlx::query_as::<_, (String,)>(
                "SELECT t.label FROM entities e JOIN entity_types t ON t.id = e.type_id
                     WHERE e.id = $1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?
            .map(|(l,)| l),
        };
        sqlx::query("UPDATE entities SET disambiguator = $2 WHERE id = $1")
            .bind(id)
            .bind(disambiguator)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 审核队列
// ---------------------------------------------------------------------------

/// 灰区疑似重复对入队（同一 pending 对幂等）。
pub async fn create_review(
    pool: &PgPool,
    kb_id: Uuid,
    left_id: Uuid,
    right_id: Uuid,
    score: f32,
    reason: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO resolution_reviews (id, kb_id, left_id, right_id, score, reason)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (kb_id, least(left_id, right_id), greatest(left_id, right_id))
             WHERE status = 'pending'
         DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(left_id)
    .bind(right_id)
    .bind(score)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct ReviewRow {
    id: Uuid,
    left_id: Uuid,
    right_id: Uuid,
    score: f32,
    reason: Option<String>,
    stage: String,
    created_at: DateTime<Utc>,
}

async fn review_side(pool: &PgPool, kb_id: Uuid, entity_id: Uuid) -> AppResult<ReviewSide> {
    #[derive(sqlx::FromRow)]
    struct SideRow {
        id: Uuid,
        name: String,
        type_label: Option<String>,
        color: String,
        disambiguator: Option<String>,
        degree: i64,
    }
    let row: SideRow = sqlx::query_as(
        "SELECT e.id, e.canonical_name AS name, t.label AS type_label,
                coalesce(t.color, '#94a3b8') AS color, e.disambiguator,
                (SELECT count(*) FROM facts f
                 WHERE (f.subject_id = e.id OR f.object_id = e.id)
                   AND f.invalidated_at IS NULL) AS degree
         -- LEFT JOIN：没判出类型的实体照样要能进审核（0009）。
         -- 内连接会让它整条审核项取不出来，而漂移审核恰恰最常发生在它们身上
         FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND e.id = $2",
    )
    .bind(kb_id)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(ReviewSide {
        id: row.id,
        name: row.name,
        type_label: row.type_label,
        color: row.color,
        disambiguator: row.disambiguator,
        degree: row.degree,
        top_facts: entity_fact_lines(pool, kb_id, entity_id, 4).await?,
    })
}

/// 实体的事实摘要行："works at → 星云科技 (2023-01 → now)"，裁决 prompt 与审核 UI 共用。
pub async fn entity_fact_lines(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    limit: i64,
) -> AppResult<Vec<String>> {
    #[derive(sqlx::FromRow)]
    struct Line {
        direction: String,
        predicate_label: String,
        other_name: Option<String>,
        valid_from: Option<DateTime<Utc>>,
        valid_to: Option<DateTime<Utc>>,
    }
    let rows: Vec<Line> = sqlx::query_as(
        "SELECT CASE WHEN f.subject_id = $2 THEN 'out' ELSE 'in' END AS direction,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                o.canonical_name AS other_name,
                f.valid_from, f.valid_to
         FROM facts f
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o
           ON o.id = CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
           AND (f.subject_id = $2 OR f.object_id = $2)
           AND COALESCE(r.label, fact_surface_predicate(f.id)) IS NOT NULL
         ORDER BY f.confidence DESC, f.recorded_at DESC
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(entity_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|l| {
            let other = l.other_name.unwrap_or_else(|| "?".into());
            let core = if l.direction == "out" {
                format!("{} → {}", l.predicate_label, other)
            } else {
                format!("{} ← {}", l.predicate_label, other)
            };
            match (l.valid_from, l.valid_to) {
                (Some(f), Some(t)) => {
                    format!("{core} ({} → {})", f.format("%Y-%m"), t.format("%Y-%m"))
                }
                (Some(f), None) => format!("{core} ({} → now)", f.format("%Y-%m")),
                _ => core,
            }
        })
        .collect())
}

async fn assemble_reviews(
    pool: &PgPool,
    kb_id: Uuid,
    rows: Vec<ReviewRow>,
) -> AppResult<Vec<ReviewItem>> {
    let mut items = Vec::with_capacity(rows.len());
    for r in rows {
        items.push(ReviewItem {
            id: r.id,
            score: r.score,
            reason: r.reason,
            stage: r.stage,
            created_at: r.created_at,
            left: review_side(pool, kb_id, r.left_id).await?,
            right: review_side(pool, kb_id, r.right_id).await?,
        });
    }
    Ok(items)
}

/// 全部待处理审核项（LLM 裁决中 + 等人工的都展示，人工可随时抢先定夺）。
pub async fn list_reviews(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ReviewItem>> {
    let rows: Vec<ReviewRow> = sqlx::query_as(
        "SELECT id, left_id, right_id, score, reason, stage, created_at
         FROM resolution_reviews
         WHERE kb_id = $1 AND status = 'pending'
         ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    assemble_reviews(pool, kb_id, rows).await
}

/// 等待 LLM 裁决的审核项（后台裁决任务消费）。
pub async fn pending_adjudications(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
) -> AppResult<Vec<ReviewItem>> {
    let rows: Vec<ReviewRow> = sqlx::query_as(
        "SELECT id, left_id, right_id, score, reason, stage, created_at
         FROM resolution_reviews
         WHERE kb_id = $1 AND status = 'pending' AND stage = 'adjudicating'
         ORDER BY created_at LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    assemble_reviews(pool, kb_id, rows).await
}

/// LLM 不确定 / 未配模型 → 转人工。
/// `reason` 存的是 **code**，可选地跟 `|detail`——不是给人看的句子。
///
/// 界面语言在客户端（见 docs/decisions/0004），服务端没有 locale 可用来措辞；
/// 写进这一列的英文散文会永久留在中文界面上。措辞归 i18n，这里只留稳定的 code。
pub async fn escalate_review(pool: &PgPool, review_id: Uuid, reason: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE resolution_reviews SET stage = 'human', reason = $2
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(review_id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// 自动定夺（LLM 高置信）：merged / kept。合并动作本身由调用方先执行。
pub async fn close_review_auto(
    pool: &PgPool,
    review_id: Uuid,
    status: &str,
    reason: &str,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE resolution_reviews SET status = $2, reason = $3, decided_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(review_id)
    .bind(status)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// 人工定夺。merge 方向：度数高（事实多）的一方作为存活目标，平局取更早创建的。
pub async fn decide_review(
    pool: &PgPool,
    kb_id: Uuid,
    review_id: Uuid,
    action: &str,
    user_id: Uuid,
) -> AppResult<()> {
    let row: Option<ReviewRow> = sqlx::query_as(
        "SELECT id, left_id, right_id, score, reason, stage, created_at
         FROM resolution_reviews WHERE id = $1 AND kb_id = $2 AND status = 'pending'",
    )
    .bind(review_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    let row = row.ok_or(AppError::NotFound)?;

    match action {
        "merge" => {
            let (target, source) = merge_direction(pool, row.left_id, row.right_id).await?;
            merge_entities(
                pool,
                kb_id,
                source,
                target,
                Some(user_id),
                "review decision",
            )
            .await?;
            sqlx::query(
                "UPDATE resolution_reviews SET status = 'merged', decided_at = now(), decided_by = $2
                 WHERE id = $1",
            )
            .bind(review_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        }
        "keep" => {
            sqlx::query(
                "UPDATE resolution_reviews SET status = 'kept', decided_at = now(), decided_by = $2
                 WHERE id = $1",
            )
            .bind(review_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        }
        _ => return Err(AppError::Validation("action must be merge or keep".into())),
    }
    Ok(())
}

/// 合并方向：返回 (target 存活, source 被并)。
pub async fn merge_direction(pool: &PgPool, a: Uuid, b: Uuid) -> AppResult<(Uuid, Uuid)> {
    let (deg_a,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM facts WHERE (subject_id = $1 OR object_id = $1) AND invalidated_at IS NULL",
    )
    .bind(a)
    .fetch_one(pool)
    .await?;
    let (deg_b,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM facts WHERE (subject_id = $1 OR object_id = $1) AND invalidated_at IS NULL",
    )
    .bind(b)
    .fetch_one(pool)
    .await?;
    // uuidv7 时间有序：度数平局时更早创建的一方存活
    Ok(if deg_a > deg_b || (deg_a == deg_b && a < b) {
        (a, b)
    } else {
        (b, a)
    })
}

// ---------------------------------------------------------------------------
// 合并 / 回滚
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct EntityFull {
    // None = 还没判出来（0009）
    type_id: Option<Uuid>,
    canonical_name: String,
    aliases: Vec<String>,
    profile_embedding: Option<Vector>,
    profile_n: i32,
    merged_into: Option<Uuid>,
}

async fn entity_full(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<EntityFull> {
    sqlx::query_as(
        "SELECT type_id, canonical_name, aliases, profile_embedding, profile_n, merged_into
         FROM entities WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)
}

/// 合并 source → target：事实改挂 target、互指事实与合并后的重复事实作废、
/// source 名并入 target 别名、画像加权合并、source 标记 merged_into。全程记日志可回滚。
pub async fn merge_entities(
    pool: &PgPool,
    kb_id: Uuid,
    source_id: Uuid,
    target_id: Uuid,
    merged_by: Option<Uuid>,
    reason: &str,
) -> AppResult<Uuid> {
    if source_id == target_id {
        return Err(AppError::invalid(
            "self_merge",
            "Cannot merge an entity into itself",
        ));
    }
    let source = entity_full(pool, kb_id, source_id).await?;
    let target = entity_full(pool, kb_id, target_id).await?;
    if source.merged_into.is_some() || target.merged_into.is_some() {
        return Err(AppError::Conflict("Entity already merged".into()));
    }

    let mut tx = pool.begin().await?;

    // 互指事实（合并后变自环）→ 作废
    let cross: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE facts SET invalidated_at = now()
         WHERE kb_id = $1 AND invalidated_at IS NULL
           AND ((subject_id = $2 AND object_id = $3) OR (subject_id = $3 AND object_id = $2))
         RETURNING id",
    )
    .bind(kb_id)
    .bind(source_id)
    .bind(target_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut invalidated: Vec<Uuid> = cross.into_iter().map(|(id,)| id).collect();

    let moved_subject: Vec<Uuid> = sqlx::query_as::<_, (Uuid,)>(
        "UPDATE facts SET subject_id = $2 WHERE kb_id = $3 AND subject_id = $1 RETURNING id",
    )
    .bind(source_id)
    .bind(target_id)
    .bind(kb_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(id,)| id)
    .collect();
    let moved_object: Vec<Uuid> = sqlx::query_as::<_, (Uuid,)>(
        "UPDATE facts SET object_id = $2 WHERE kb_id = $3 AND object_id = $1 RETURNING id",
    )
    .bind(source_id)
    .bind(target_id)
    .bind(kb_id)
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|(id,)| id)
    .collect();

    // 合并后 SPO+valid_from 重复的 live 事实：留最早 recorded_at 的一条，其余作废。
    //
    // **宾语两侧都要分组。** 字面值事实的 object_id 全是 NULL，只按它分组就等于
    // 把同主同谓下的**所有值**当成同一条断言：一次合并之后，
    // (公司, 成立年份, 2015) 与 (公司, 注册资本, …) 之外，同谓词的多个值只活得下来
    // 最早记的那一个，其余无声消失，且没有 supersedes 可查。
    let dups: Vec<(Vec<Uuid>,)> = sqlx::query_as(
        "SELECT (array_agg(id ORDER BY recorded_at))[2:] FROM facts
         WHERE kb_id = $1 AND invalidated_at IS NULL
           AND (subject_id = $2 OR object_id = $2)
         GROUP BY subject_id, predicate_id, object_id, object_value, valid_from
         HAVING count(*) > 1",
    )
    .bind(kb_id)
    .bind(target_id)
    .fetch_all(&mut *tx)
    .await?;
    let dup_ids: Vec<Uuid> = dups.into_iter().flat_map(|(ids,)| ids).collect();
    if !dup_ids.is_empty() {
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = ANY($1)")
            .bind(&dup_ids)
            .execute(&mut *tx)
            .await?;
        invalidated.extend(dup_ids);
    }

    // source 名与别名并入 target 别名（去重、排除 target 本名）
    let mut aliases = target.aliases.clone();
    let taken: std::collections::HashSet<String> = std::iter::once(&target.canonical_name)
        .chain(aliases.iter())
        .map(|s| s.to_lowercase())
        .collect();
    for a in std::iter::once(&source.canonical_name).chain(source.aliases.iter()) {
        if !taken.contains(&a.to_lowercase()) && !aliases.iter().any(|x| x.eq_ignore_ascii_case(a))
        {
            aliases.push(a.clone());
        }
    }

    // 画像加权合并
    let (profile, profile_n) = match (&target.profile_embedding, &source.profile_embedding) {
        (Some(t), Some(s)) if t.as_slice().len() == s.as_slice().len() => {
            let (nt, ns) = (
                target.profile_n.max(1) as f32,
                source.profile_n.max(1) as f32,
            );
            let merged: Vec<f32> = t
                .as_slice()
                .iter()
                .zip(s.as_slice())
                .map(|(a, b)| (a * nt + b * ns) / (nt + ns))
                .collect();
            (
                Some(Vector::from(merged)),
                target.profile_n + source.profile_n,
            )
        }
        (Some(t), _) => (Some(t.clone()), target.profile_n),
        (None, Some(s)) => (Some(s.clone()), source.profile_n),
        (None, None) => (None, 0),
    };

    // 类型调和：**没有类型的一侧让位**。存活方还没判出来而被并方判出来了，
    // 就把那个类型带过来；否则保留存活方的。
    //
    // 从前这里要先查库找出 concept 那行的 id 再跟两侧比对。「还没判出来」
    // 现在是 `None`（0009），一个 `or` 就说完了，那次查询也省了
    let new_type_id = target.type_id.or(source.type_id);

    sqlx::query(
        "UPDATE entities SET aliases = $2, profile_embedding = $3, profile_n = $4,
                type_id = $5, updated_at = now() WHERE id = $1",
    )
    .bind(target_id)
    .bind(&aliases)
    .bind(&profile)
    .bind(profile_n)
    .bind(new_type_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE entities SET merged_into = $2, updated_at = now() WHERE id = $1")
        .bind(source_id)
        .bind(target_id)
        .execute(&mut *tx)
        .await?;

    // 其余涉及 source 的 pending 审核项：**改指到合并目标，不是关掉**。
    //
    // 这里原本一律关掉，理由写的是"疑点若仍在会由后续 mention 重新提起"。
    // **那句是错的**：包含关系召回只在**新建实体时**跑一次，而这些实体早就存在了，
    // 不会再被新建，也就没有"后续 mention"来重提。关掉就是永远关掉。
    //
    // 实测：`Mr. Holmes` 跟 `Holmes` 配对入队（0.70），随后 `Holmes` 并入
    // `Sherlock Holmes`，这一对被关成 superseded by merge——而
    // `Mr. Holmes` vs `Sherlock Holmes` 这个仍然成立的问题，再没人问过。
    //
    // 先关掉两类真正过时的：重定向后会变成自环的，和目标对已经在队列里的。
    sqlx::query(
        "UPDATE resolution_reviews AS r
         SET status = 'kept', reason = 'superseded by merge', decided_at = now()
         WHERE r.kb_id = $1 AND r.status = 'pending'
           AND (r.left_id = $2 OR r.right_id = $2)
           AND NOT (least(r.left_id, r.right_id) = least($2, $3)
                    AND greatest(r.left_id, r.right_id) = greatest($2, $3))
           AND (
             (CASE WHEN r.left_id = $2 THEN r.right_id ELSE r.left_id END) = $3
             OR EXISTS (
               SELECT 1 FROM resolution_reviews d
               WHERE d.kb_id = $1 AND d.status = 'pending' AND d.id <> r.id
                 AND least(d.left_id, d.right_id)
                     = least($3, CASE WHEN r.left_id = $2 THEN r.right_id ELSE r.left_id END)
                 AND greatest(d.left_id, d.right_id)
                     = greatest($3, CASE WHEN r.left_id = $2 THEN r.right_id ELSE r.left_id END))
           )",
    )
    .bind(kb_id)
    .bind(source_id)
    .bind(target_id)
    .execute(&mut *tx)
    .await?;
    // 剩下的改指到目标，问题继续挂在队列上等裁决。
    // (source, target) 这一对本身除外——它正是本次合并的裁决对象，由调用方标记 merged，
    // 在这里动它会让它在历史里错误地显示为"保持分开"
    sqlx::query(
        "UPDATE resolution_reviews
         SET left_id = CASE WHEN left_id = $2 THEN $3 ELSE left_id END,
             right_id = CASE WHEN right_id = $2 THEN $3 ELSE right_id END,
             reason = reason || '|redirected'
         WHERE kb_id = $1 AND status = 'pending' AND (left_id = $2 OR right_id = $2)
           AND NOT (least(left_id, right_id) = least($2, $3)
                    AND greatest(left_id, right_id) = greatest($2, $3))",
    )
    .bind(kb_id)
    .bind(source_id)
    .bind(target_id)
    .execute(&mut *tx)
    .await?;

    let merge_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_merges (id, kb_id, source_id, target_id,
                moved_subject_facts, moved_object_facts, invalidated_facts,
                target_profile_before, target_profile_n_before, target_type_before,
                merged_by, reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(merge_id)
    .bind(kb_id)
    .bind(source_id)
    .bind(target_id)
    .bind(&moved_subject)
    .bind(&moved_object)
    .bind(&invalidated)
    .bind(&target.profile_embedding)
    .bind(target.profile_n)
    .bind(target.type_id)
    .bind(merged_by)
    .bind(reason)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // 搬移后的时态对账：换了主/宾的事实等价于新观察落库——两个对象折成一个后，
    // 唯一性不变量才第一次看得到旧开放区间与继任者相撞（如"星尘"并入"星尘项目"，
    // 旧负责人的 leads 应在新任起点闭合）。
    // 修正行 id 记入合并账本：这些修正的唯一成因是本次合并，回滚时必须随之撤销。
    // 注：本步在事务外，失败有自愈性——残留的旧开放行会在下一条相关新事实落库时
    // 被常规插入对账撞到并闭合。
    let moved_all: Vec<Uuid> = moved_subject
        .iter()
        .chain(moved_object.iter())
        .copied()
        .collect();
    let report = crate::temporal::reconcile_moved_facts(pool, kb_id, &moved_all).await?;
    if !report.corrected.is_empty() {
        sqlx::query("UPDATE entity_merges SET temporal_corrections = $2 WHERE id = $1")
            .bind(merge_id)
            .bind(&report.corrected)
            .execute(pool)
            .await?;
    }

    refresh_disambiguators(pool, kb_id, &source.canonical_name).await?;
    if !source
        .canonical_name
        .eq_ignore_ascii_case(&target.canonical_name)
    {
        refresh_disambiguators(pool, kb_id, &target.canonical_name).await?;
    }
    Ok(merge_id)
}

#[derive(Debug, sqlx::FromRow)]
struct MergeRow {
    source_id: Uuid,
    target_id: Uuid,
    moved_subject_facts: Vec<Uuid>,
    moved_object_facts: Vec<Uuid>,
    invalidated_facts: Vec<Uuid>,
    temporal_corrections: Vec<Uuid>,
    target_profile_before: Option<Vector>,
    target_profile_n_before: i32,
    target_type_before: Option<Uuid>,
    reverted_at: Option<DateTime<Utc>>,
}

/// 精确回滚一次合并：事实原路搬回、作废撤销、target 画像与类型恢复快照、source 复活。
pub async fn revert_merge(pool: &PgPool, kb_id: Uuid, merge_id: Uuid) -> AppResult<()> {
    let m: MergeRow = sqlx::query_as(
        "SELECT source_id, target_id, moved_subject_facts, moved_object_facts,
                invalidated_facts, temporal_corrections, target_profile_before,
                target_profile_n_before, target_type_before, reverted_at
         FROM entity_merges WHERE id = $1 AND kb_id = $2",
    )
    .bind(merge_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;
    if m.reverted_at.is_some() {
        return Err(AppError::Conflict("Merge already reverted".into()));
    }

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE facts SET subject_id = $1 WHERE id = ANY($2)")
        .bind(m.source_id)
        .bind(&m.moved_subject_facts)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE facts SET object_id = $1 WHERE id = ANY($2)")
        .bind(m.source_id)
        .bind(&m.moved_object_facts)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE facts SET invalidated_at = NULL WHERE id = ANY($1)")
        .bind(&m.invalidated_facts)
        .execute(&mut *tx)
        .await?;

    // 合并引发的时态修正随之撤销：这些修正的唯一成因是本次合并（两实体折一后
    // 不变量才看到的相撞），成因既撤、修正随撤——先恢复被取代的原行，再作废修正行。
    // 只撤仍存活的修正：之后被真实新观察再度改写过的链保持不动（那部分有独立依据）。
    sqlx::query(
        "UPDATE facts SET invalidated_at = NULL WHERE id IN (
             SELECT supersedes FROM facts
             WHERE id = ANY($1) AND invalidated_at IS NULL AND supersedes IS NOT NULL)",
    )
    .bind(&m.temporal_corrections)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE facts SET invalidated_at = now()
         WHERE id = ANY($1) AND invalidated_at IS NULL",
    )
    .bind(&m.temporal_corrections)
    .execute(&mut *tx)
    .await?;

    let source = entity_full(pool, kb_id, m.source_id).await?;
    // target 别名回退：剔除来自 source 的名字
    sqlx::query(
        "UPDATE entities SET
            aliases = (SELECT coalesce(array_agg(a), '{}') FROM unnest(aliases) a
                       WHERE lower(a) <> ALL($2)),
            profile_embedding = $3, profile_n = $4,
            type_id = coalesce($5, type_id), updated_at = now()
         WHERE id = $1",
    )
    .bind(m.target_id)
    .bind(
        std::iter::once(&source.canonical_name)
            .chain(source.aliases.iter())
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>(),
    )
    .bind(&m.target_profile_before)
    .bind(m.target_profile_n_before)
    .bind(m.target_type_before)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE entities SET merged_into = NULL, updated_at = now() WHERE id = $1")
        .bind(m.source_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE entity_merges SET reverted_at = now() WHERE id = $1")
        .bind(merge_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    refresh_disambiguators(pool, kb_id, &source.canonical_name).await?;
    Ok(())
}

/// 合并日志（审核页历史区）。
pub async fn list_merges(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<MergeLogView>> {
    let rows: Vec<MergeLogView> = sqlx::query_as(
        "SELECT m.id, s.canonical_name AS source_name, t.canonical_name AS target_name,
                u.display_name AS merged_by_name, m.reason, m.created_at, m.reverted_at
         FROM entity_merges m
         JOIN entities s ON s.id = m.source_id
         JOIN entities t ON t.id = m.target_id
         LEFT JOIN users u ON u.id = m.merged_by
         WHERE m.kb_id = $1
         ORDER BY m.created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// LLM 裁决缓存
// ---------------------------------------------------------------------------

pub async fn get_verdict(
    pool: &PgPool,
    kb_id: Uuid,
    pair_key: &str,
) -> AppResult<Option<(Option<bool>, f32)>> {
    let row: Option<(Option<bool>, f32)> = sqlx::query_as(
        "SELECT same, confidence FROM resolution_verdicts WHERE kb_id = $1 AND pair_key = $2",
    )
    .bind(kb_id)
    .bind(pair_key)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn put_verdict(
    pool: &PgPool,
    kb_id: Uuid,
    pair_key: &str,
    same: Option<bool>,
    confidence: f32,
    model: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO resolution_verdicts (kb_id, pair_key, same, confidence, model)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (kb_id, pair_key)
         DO UPDATE SET same = $3, confidence = $4, model = $5, created_at = now()",
    )
    .bind(kb_id)
    .bind(pair_key)
    .bind(same)
    .bind(confidence)
    .bind(model)
    .execute(pool)
    .await?;
    Ok(())
}

/// 记下模型提议、但本体装不下的类型。
///
/// **只写第一次**：同一实体会被多篇文档提到，第一次的提议就算它的提议；
/// 后来的覆盖会让"哪些实体在等 model 类"随最后一篇文档抖动。
/// 采纳了对应的类之后由改写流程清空——那时它已经不是"提议"而是既成事实。
pub async fn set_proposed_type(pool: &PgPool, entity_id: Uuid, proposed: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE entities SET proposed_type = left($2, 60)
         WHERE id = $1 AND proposed_type IS NULL",
    )
    .bind(entity_id)
    .bind(proposed)
    .execute(pool)
    .await?;
    Ok(())
}

/// 待认领的实体类型：模型提议过、本体没有、实体因此降级成了 concept。
///
/// 与谓词那边的 `graph::proposed_predicates` 对称——连着具体实体，所以采纳时
/// 能说清"将重新归类 43 个"并真的去改，而不是只建一个空类。
pub async fn proposed_types(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<Vec<utopia_core::models::ProposedType>> {
    Ok(sqlx::query_as(
        "SELECT e.proposed_type AS form,
                count(*) AS entity_count,
                (array_agg(e.canonical_name ORDER BY e.created_at))[1] AS example
         FROM entities e
         WHERE e.kb_id = $1 AND e.merged_into IS NULL AND e.proposed_type IS NOT NULL
           -- 用户拒绝过的类型不再出现在候选里
           AND NOT EXISTS (SELECT 1 FROM ontology_misses m
                           WHERE m.kb_id = $1 AND m.kind = 'entity_type'
                             AND m.key = e.proposed_type AND m.dismissed_at IS NOT NULL)
         GROUP BY e.proposed_type
         ORDER BY entity_count DESC, form",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 把提议过 `forms` 里那些类型的实体改到 `type_id` 上。返回 (批次 id, 改动数)。
///
/// 与谓词那边的 `graph::adopt_proposed_predicates` 对称：**只建类型不动实体，
/// 本体长大了、图没变好**——提议过 model 的实体会继续挂在 concept 下。
///
/// 实体是可变行（P0 的 PATCH 就直接改），所以这里就是 UPDATE，撤销靠账本
/// 记下改之前的类型，而不是靠 supersedes 链。
pub async fn adopt_proposed_types(
    pool: &PgPool,
    kb_id: Uuid,
    type_id: Uuid,
    forms: &[String],
    // None = 引擎自动（本体长出新类之后的收尾认领没有人在按）
    actor: Option<Uuid>,
) -> AppResult<(Uuid, u32)> {
    let batch_id = Uuid::now_v7();
    if forms.is_empty() {
        return Ok((batch_id, 0));
    }
    // 已经在目标类上的不算改动，也不进账本——撤销时不该把它们推回去。
    //
    // **`IS DISTINCT FROM` 而不是 `<>`**：0009 之后 type_id 可能是 NULL，而
    // `NULL <> uuid` 求值为 NULL 不是 true，那一行会被悄悄滤掉——偏偏带着
    // proposed_type 的几乎全是还没判出类型的实体，整个认领功能会一声不响地空转
    let targets: Vec<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT id, type_id, canonical_name FROM entities
         WHERE kb_id = $1 AND merged_into IS NULL
           -- 人拍过板的不认领。**这一行顺带让 unadopt 天然正确**：human 行
           -- 永远不进采纳批次，撤销时也就不会遇到它们，不必额外还原 type_source
           AND type_source <> 'human'
           AND proposed_type = ANY($2) AND type_id IS DISTINCT FROM $3",
    )
    .bind(kb_id)
    .bind(forms)
    .bind(type_id)
    .fetch_all(pool)
    .await?;

    let mut names: HashSet<String> = HashSet::new();
    let mut moved = 0u32;
    for (entity_id, from_type, name) in targets {
        let mut tx = pool.begin().await?;
        sqlx::query(
            // actor 有值 = 人在界面上点的批准，他背书了这个类型 → 受保护。
            // 无值 = 本体长出新类之后的收尾认领，没人在按
            "UPDATE entities
                SET type_id = $2, proposed_type = NULL, updated_at = now(),
                    type_source = CASE WHEN $3::uuid IS NULL THEN 'inferred' ELSE 'human' END
             WHERE id = $1",
        )
        .bind(entity_id)
        .bind(type_id)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO entity_retypes
                (batch_id, kb_id, entity_id, from_type_id, to_type_id, actor_id)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(batch_id)
        .bind(kb_id)
        .bind(entity_id)
        .bind(from_type)
        .bind(type_id)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        names.insert(name);
        moved += 1;
    }
    // 消歧后缀的兜底值就是类型标签，改了类就得重算（同 P0 的实体改类）
    for n in &names {
        refresh_disambiguators(pool, kb_id, n).await?;
    }
    Ok((batch_id, moved))
}

/// 撤销一次实体改类：把它们放回原来的类型。
///
/// 类型本身不删——与谓词那边同一条理由：有实体指向过它，而"它存在过"是历史。
/// `proposed_type` 也一并恢复，否则撤销之后那些实体就再也认领不回来了。
pub async fn unadopt_types(pool: &PgPool, kb_id: Uuid, batch_id: Uuid) -> AppResult<u32> {
    // from_type_id 可能是 NULL：0009 之后最常见的一次改类正是"从没有类到有类"，
    // 撤销就是把它推回没有类。解成 Uuid 会在这里当场崩
    let rows: Vec<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
        "SELECT r.entity_id, r.from_type_id, t.key
         FROM entity_retypes r JOIN entity_types t ON t.id = r.to_type_id
         WHERE r.batch_id = $1 AND r.kb_id = $2 AND r.reverted_at IS NULL",
    )
    .bind(batch_id)
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(AppError::NotFound);
    }
    let mut names: Vec<String> = Vec::new();
    let mut tx = pool.begin().await?;
    let mut reverted = 0u32;
    for (entity_id, from_type, adopted_key) in &rows {
        let row: Option<(String,)> = sqlx::query_as(
            "UPDATE entities SET type_id = $2, proposed_type = $3, updated_at = now()
             WHERE id = $1 RETURNING canonical_name",
        )
        .bind(entity_id)
        .bind(from_type)
        .bind(adopted_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((name,)) = row {
            names.push(name);
        }
        reverted += 1;
    }
    sqlx::query(
        "UPDATE entity_retypes SET reverted_at = now()
         WHERE batch_id = $1 AND kb_id = $2 AND reverted_at IS NULL",
    )
    .bind(batch_id)
    .bind(kb_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    for n in &names {
        refresh_disambiguators(pool, kb_id, n).await?;
    }
    Ok(reverted)
}

/// 把提议的类型规整成 key 的形状：小写、非字母数字换下划线、压缩重复。
/// "AI Model" → "ai_model"，与 validate_key 允许的字符集对齐。
fn normalize_type_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_us = true; // 前导下划线也算重复
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out.chars().take(40).collect()
}

/// 认领那些"类型已经在本体里、实体却还挂在 concept 下"的实体。
///
/// `adopt_proposed_types` 只在**建类的那一刻**被调用，于是类先建好、实体后被
/// 抽出来的情形就永远等不到搬运——而这恰恰是常态：本体第一轮建好，后续文档
/// 继续产出提议。这个扫描把它们收尾。
///
/// 只做**规整后精确同名**的匹配，不做近似——猜错就是把实体放进错的类，
/// 而"再等一轮"的代价接近零。
pub async fn sweep_proposed_types(
    pool: &PgPool,
    kb_id: Uuid,
    actor: Option<Uuid>,
) -> AppResult<Vec<(Uuid, u32)>> {
    let pending = proposed_types(pool, kb_id).await?;
    let existing: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, key FROM entity_types WHERE kb_id = $1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?;
    let mut out = Vec::new();
    for p in &pending {
        let norm = normalize_type_key(&p.form);
        let Some((type_id, _)) = existing.iter().find(|(_, k)| *k == norm) else {
            continue;
        };
        let (batch, n) =
            adopt_proposed_types(pool, kb_id, *type_id, std::slice::from_ref(&p.form), actor)
                .await?;
        if n > 0 {
            out.push((batch, n));
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_fullwidth_and_whitespace() {
        assert_eq!(normalize_name("　ＡＣＭＥ  Corp　"), "ACME Corp");
        assert_eq!(normalize_name("张三"), "张三");
        assert_eq!(normalize_name("张  三"), "张 三");
    }

    #[test]
    fn stem_generic_suffixes() {
        assert_eq!(name_stem("星尘项目").as_deref(), Some("星尘"));
        assert_eq!(name_stem("星辰科技公司").as_deref(), Some("星辰科技"));
        assert_eq!(name_stem("Phoenix Project").as_deref(), Some("phoenix"));
        assert_eq!(name_stem("Project Phoenix").as_deref(), Some("phoenix"));
        assert_eq!(name_stem("Acme Inc.").as_deref(), Some("acme"));
        assert_eq!(name_stem("星尘"), None);
        assert_eq!(name_stem("项目"), None); // 剥空不算词干
        assert_eq!(name_stem("项目团队"), None); // 词干本身是泛用词
        assert_eq!(name_stem("Team Project"), None);
    }

    #[test]
    fn recall_keys_bidirectional() {
        // 无后缀 mention 能召回带后缀实体（增广），反之靠词干
        let k = recall_keys("星尘");
        assert!(k.contains(&"星尘".to_string()));
        assert!(k.contains(&"星尘项目".to_string()));
        let k = recall_keys("星尘项目");
        assert!(k.contains(&"星尘项目".to_string()));
        assert!(k.contains(&"星尘".to_string()));
        let k = recall_keys("Phoenix");
        assert!(k.contains(&"phoenix project".to_string()));
        assert!(k.contains(&"project phoenix".to_string()));
        let k = recall_keys("Project Phoenix");
        assert!(k.contains(&"phoenix".to_string()));
        assert!(recall_keys("张三").len() <= 10);
    }

    #[test]
    fn type_drift_classes() {
        // 还没判出类型的那一侧 → 召回候选。0009 之前这一档比的是 `concept`
        // 这个 key，现在比的是"有没有类"本身
        assert_eq!(
            classify_type_drift(None, Some("organization")),
            TypeDrift::Recall
        );
        assert_eq!(
            classify_type_drift(Some("project"), None),
            TypeDrift::Recall
        );
        // 两边都还没判出来：也归召回，由画像相似度说话
        assert_eq!(classify_type_drift(None, None), TypeDrift::Recall);
        // 易混具体类型两两 → 审核对
        assert_eq!(
            classify_type_drift(Some("organization"), Some("project")),
            TypeDrift::Review
        );
        assert_eq!(
            classify_type_drift(Some("project"), Some("product")),
            TypeDrift::Review
        );
        assert_eq!(
            classify_type_drift(Some("product"), Some("organization")),
            TypeDrift::Review
        );
        // 硬互斥与未知自定义类型 → 完全分开
        assert_eq!(
            classify_type_drift(Some("person"), Some("organization")),
            TypeDrift::Disjoint
        );
        assert_eq!(
            classify_type_drift(Some("person"), Some("project")),
            TypeDrift::Disjoint
        );
        assert_eq!(
            classify_type_drift(Some("event"), Some("location")),
            TypeDrift::Disjoint
        );
        assert_eq!(
            classify_type_drift(Some("team"), Some("organization")),
            TypeDrift::Disjoint
        );
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap().abs() < 1e-6);
        assert!(cosine(&[1.0], &[1.0, 2.0]).is_none());
        assert!(cosine(&[0.0, 0.0], &[1.0, 1.0]).is_none());
    }
}

/// 一个等着精化类型的实体，连同用来判断它是什么的全部材料。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TypeCandidateSubject {
    pub id: Uuid,
    pub canonical_name: String,
    pub aliases: Vec<String>,
    /// 现在挂着的类，**可能没有**（0009：没判出来就是 NULL）。名字里的"粗"
    /// 是历史——抽取现在也可能直接给一个细类，而且可能给错（实测
    /// `绍兴 → address`），所以这里也可能是要被**纠正**的那个
    pub coarse_key: Option<String>,
    pub coarse_id: Option<Uuid>,
    /// 现类的描述。裁决要判"现在这个类对不对"，光看 key 不够——
    /// 导入本体的 key 常常自解释不了（`entry_point` 是什么？）
    pub coarse_description: Option<String>,
    /// 抽取时模型自己报的类型名，**词表里没有**才会留在这儿
    pub proposed_type: Option<String>,
    /// 模型对它自己的说法，每个实体都有。
    ///
    /// 这是最强的信号，因为它把任务从"读懂这是什么"换回"本体里哪个类叫这个"
    /// ——短名字对短标签。实测的失败正出在另一头：拿一段中文画像去匹配
    /// schema.org 的 "A software application."，两边形状根本不对等
    pub specific_type: Option<String>,
    /// 它参与的谓词，连着对方的名字（`produces 深蓝`、`leads by 张伟`）。
    /// **跨文档累积**，这正是抽取当场没有的东西
    pub roles: Vec<String>,
    /// 证据引文，最多几句。抽取看的是同一批句子，但那一次要同时做实体识别、
    /// 关系判断、时态解析、JSON 格式化，还要跟几十个别的实体抢注意力
    /// 证据引文，**只取它当主语的那些**。宾语位的引文讲的是主语：
    /// 「上海浦东新区」是 located_in 的宾语，引文却是"星云科技（上海）
    /// 有限公司是一家注册于上海浦东新区的股份有限公司"，整句重心在
    /// "股份有限公司"上——实测那一版检索给出的候选全是 corporation 一类
    pub quotes: Vec<String>,
    pub fact_count: i64,
}

/// 值得送去精化类型的实体：**还没有类的**，或者模型报过一个词表外类型的。
///
/// 类型化得已经很具体的实体不动——重判一次只有下降风险，没有上升空间。
pub async fn entities_for_type_resolution(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
) -> AppResult<Vec<TypeCandidateSubject>> {
    Ok(sqlx::query_as(
        "SELECT e.id, e.canonical_name, e.aliases, t.key AS coarse_key, t.id AS coarse_id,
                t.description AS coarse_description,
                e.proposed_type, e.specific_type,
                -- 对方写**名字**，不写它的类型 key。
                -- 写 key 是自毁：查询里出现 organization / product / event 这些词，
                -- 而检索的目标正是这些类自己，于是候选就是画像里那几个词。
                -- 实测「张伟」的画像是 leads→organization … works_at→organization，
                -- 回来的候选是 corporation、organization、business_entity_type——
                -- 检索到的是画像自己，不是这个人是什么
                ARRAY(
                  SELECT DISTINCT COALESCE(rt.key, fact_surface_predicate(f.id))
                                  || CASE WHEN f.subject_id = e.id THEN ' ' ELSE ' by ' END
                                  || coalesce(oe.canonical_name, 'a value')
                  FROM facts f
                  LEFT JOIN relation_types rt ON rt.id = f.predicate_id
                  LEFT JOIN entities oe ON oe.id = CASE WHEN f.subject_id = e.id
                                                       THEN f.object_id ELSE f.subject_id END
                  WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
                    AND (f.subject_id = e.id OR f.object_id = e.id)
                    AND COALESCE(rt.key, fact_surface_predicate(f.id)) IS NOT NULL
                  LIMIT 12
                ) AS roles,
                ARRAY(
                  SELECT DISTINCT ev.quote FROM fact_evidence ev
                  JOIN facts f2 ON f2.id = ev.fact_id
                  WHERE f2.kb_id = $1 AND f2.invalidated_at IS NULL
                    AND f2.subject_id = e.id
                    AND ev.quote IS NOT NULL
                  LIMIT 3
                ) AS quotes,
                (SELECT count(*) FROM facts f3
                 WHERE f3.kb_id = $1 AND f3.invalidated_at IS NULL
                   AND (f3.subject_id = e.id OR f3.object_id = e.id)) AS fact_count
         FROM entities e
         LEFT JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND e.merged_into IS NULL
           -- **人拍过板的不再重判。**
           --
           -- 少了这一行，下面第三种条件会把它们全都捞回来：一个人工定成
           -- organization 的实体，只要 organization 有子类就够格，于是引擎
           -- 每一轮都去重新裁决一遍人已经决定过的事，而账本事后才看得出是谁改的。
           --
           -- 「不落库」不等于「不许说话」：引擎若认为人定错了，该走 Review 队列，
           -- 而不是直接改掉。憋着不说和直接改掉都是失真，只是方向相反
           AND e.type_source <> 'human'
           -- 值得看的三种：**还没有类的**、模型报过词表外类型的、
           -- **以及现类本身还有子类的**。第三种是主力：抽取只认基类之后，
           -- organization 下面挂着一大批更具体的类，那才是导进来的本体
           -- 唯一的用武之地。只看前两种就把它整个漏掉了
           AND (e.type_id IS NULL OR e.proposed_type IS NOT NULL OR e.specific_type IS NOT NULL
                OR EXISTS (SELECT 1 FROM entity_type_parents p WHERE p.parent_id = t.id))
         ORDER BY fact_count DESC, e.created_at
         LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// 某个类的全部后代（含自身）。精化只能往粗类的后代走。
///
/// **递归而不是一层**：本体是 DAG，`software_application ⊂ creative_work ⊂ thing`，
/// 只查一层就把绝大多数正确答案挡在门外了。
pub async fn descendants_of(pool: &PgPool, kb_id: Uuid, root: Uuid) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "WITH RECURSIVE d(id) AS (
             SELECT $2::uuid
             UNION
             SELECT p.child_id FROM entity_type_parents p JOIN d ON d.id = p.parent_id
         )
         SELECT d.id FROM d JOIN entity_types t ON t.id = d.id WHERE t.kb_id = $1",
    )
    .bind(kb_id)
    .bind(root)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 记下模型对这个实体自己的说法。
///
/// **后写者不覆盖先写者**（`IS NULL` 才写），与 `set_proposed_type` 同一条：
/// 同一个实体会被多个分块提到，第一次的说法通常出自最完整的那句话，
/// 后面的分块往往只是顺带一提。
pub async fn set_specific_type(pool: &PgPool, entity_id: Uuid, value: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE entities SET specific_type = left($2, 80)
         WHERE id = $1 AND specific_type IS NULL",
    )
    .bind(entity_id)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// 跟这个实体最像的**已定类**实体，连同它们的类。
///
/// 类型消解的第二个候选来源。第一个是拿画像去搜类的描述，它的软肋实测很清楚：
/// 中文画像对 schema.org 的 "A software application."，两边形状根本不对等。
/// 这一条绕开了那道坎——**名字对名字、同语言**，而且库越大越准：
/// 「深蓝向量数据库」像「Milvus」，而 Milvus 已经标成 software_application。
///
/// 比的是双方的 `profile_embedding`（出现语境的滑动平均），**同一种向量对同一种
/// 向量**，不是拿文本画像去比语境向量。代价是零——这个向量实体消解本来就在维护。
///
/// 已知弱点：只出现在一篇文档里的实体，语境向量就是那一块的向量，同文档的实体
/// 会互相成为近邻。调用方要看得到 `same_document`，别把它当成类型证据。
pub async fn nearest_typed_entities(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    limit: i64,
) -> AppResult<Vec<(String, Uuid, String, f64, bool)>> {
    Ok(sqlx::query_as(
        "WITH me AS (
             SELECT profile_embedding AS v FROM entities WHERE id = $2 AND kb_id = $1
         ),
         my_docs AS (
             SELECT DISTINCT ev.document_id FROM fact_evidence ev
             JOIN facts f ON f.id = ev.fact_id
             WHERE f.kb_id = $1 AND (f.subject_id = $2 OR f.object_id = $2)
         )
         SELECT e.canonical_name, t.id, t.key,
                (e.profile_embedding <=> (SELECT v FROM me))::float8 AS distance,
                EXISTS (SELECT 1 FROM fact_evidence ev2
                        JOIN facts f2 ON f2.id = ev2.fact_id
                        WHERE f2.kb_id = $1 AND (f2.subject_id = e.id OR f2.object_id = e.id)
                          AND ev2.document_id IN (SELECT document_id FROM my_docs))
                AS same_document
         -- 内连接就是那道门：没判出类型的实体（type_id IS NULL）不是答案，
         -- 拿它当邻居的证据只会把「没判出来」传染开
         FROM entities e
         JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND e.merged_into IS NULL AND e.id <> $2
           AND e.profile_embedding IS NOT NULL
           AND (SELECT v FROM me) IS NOT NULL
         ORDER BY e.profile_embedding <=> (SELECT v FROM me)
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(entity_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// 按实体逐个改类，写进同一本账。返回 (批次 id, 改动数)。
///
/// 与 [`adopt_proposed_types`] 的区别只在挑选方式：那个按 `proposed_type` 这个
/// **说法**认领一批，这个由调用方点名——类型消解裁决出来的是"这个实体是那个类"，
/// 不是"叫这个说法的都是那个类"。
///
/// 账本格式一字不差，所以 [`unadopt_types`] 原样能撤。
pub async fn retype_entities(
    pool: &PgPool,
    kb_id: Uuid,
    picks: &[(Uuid, Uuid)],
    // None = 引擎自动裁决。跟 entity_merges.merged_by 一个约定,
    // 实体历史据此区分"某某某改的"与"高置信自动改的"
    actor: Option<Uuid>,
) -> AppResult<(Uuid, u32)> {
    let batch_id = Uuid::now_v7();
    let mut moved = 0u32;
    let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
    // 一个实体在一批里只改一次。账本主键是 (batch_id, entity_id)，同一个 id
    // 来两次会撞——而调用方拿到的是 500,整批一个都没落。这里挡住比在那边
    // 追每一条产生 picks 的路子可靠:重复的第二条本来也没有意义
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();
    for (entity_id, type_id) in picks {
        if !seen.insert(*entity_id) {
            continue;
        }
        let mut tx = pool.begin().await?;
        // 已经在目标类上的不算改动，也不进账本——撤销时不该把它们推回去。
        //
        // **旧类型要从 CTE 里读**：`UPDATE … RETURNING` 给的是新值，而账本要记的
        // 是改之前那个。直接 RETURNING type_id 拿到的就是刚写进去的那一个，
        // 撤销时等于把实体"放回"它现在的位置——账本看着满满当当，实际什么都撤不了。
        // 条件也一并放进 CTE：这一行还在、没被合并、且确实要变
        //
        // **`IS DISTINCT FROM` 而不是 `<>`**：0009 之后起点常常是 NULL，而
        // `NULL <> uuid` 是 NULL 不是 true——CTE 会空掉，UPDATE 一行不动，
        // 于是"给没有类的实体定类"这个最主要的场景整个变成空操作
        let row: Option<(Option<Uuid>, String)> = sqlx::query_as(
            "WITH before AS (
                 SELECT id, type_id, canonical_name FROM entities
                 WHERE id = $1 AND kb_id = $3 AND merged_into IS NULL
                   AND type_id IS DISTINCT FROM $2
             )
             UPDATE entities e SET type_id = $2, updated_at = now(),
                    type_source = CASE WHEN $4::uuid IS NULL THEN 'inferred' ELSE 'human' END
             FROM before
             WHERE e.id = before.id
             RETURNING before.type_id, before.canonical_name",
        )
        .bind(entity_id)
        .bind(type_id)
        .bind(kb_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((from_type, name)) = row else {
            tx.rollback().await?;
            continue;
        };
        sqlx::query(
            "INSERT INTO entity_retypes
                (batch_id, kb_id, entity_id, from_type_id, to_type_id, actor_id)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(batch_id)
        .bind(kb_id)
        .bind(entity_id)
        .bind(from_type)
        .bind(type_id)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        names.insert(name);
        moved += 1;
    }
    // 消歧后缀的兜底值就是类型标签，改了类就得重算
    for n in &names {
        refresh_disambiguators(pool, kb_id, n).await?;
    }
    Ok((batch_id, moved))
}

/// 人认可过的"粗类 → 细类"配对。
///
/// 待人工那一档由"跨没跨分类轴"触发，而那条判据测的常常是**种子类跟导入词汇表
/// 连没连上**，不是风险：schema.org 的 Place 另起 key，内置 location 零子类，
/// 于是每个城市都要问一遍。配对是类与类之间的事，实体只是碰巧撞上它——
/// 认可一次就该一直算数。
pub async fn approved_refinements(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<std::collections::HashSet<(Uuid, Uuid)>> {
    let rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT from_type_id, to_type_id FROM type_refinement_pairs WHERE kb_id = $1",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// 记下一个被认可的配对。重复认可是幂等的。
pub async fn approve_refinement(
    pool: &PgPool,
    kb_id: Uuid,
    from_type_id: Uuid,
    to_type_id: Uuid,
    by: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO type_refinement_pairs (kb_id, from_type_id, to_type_id, approved_by)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (kb_id, from_type_id, to_type_id) DO NOTHING",
    )
    .bind(kb_id)
    .bind(from_type_id)
    .bind(to_type_id)
    .bind(by)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod same_type_tests {
    use super::{classify_type_drift, TypeDrift};

    /// **相同类型是最常见的情形，而它曾经落在 Disjoint 上。**
    ///
    /// 这个函数生来服务"类型漂移"（同名被抽成两种类型），那里两边相同不会发生；
    /// 后来被 `containment_reviews` 借去当相容性判据，那里两边相同是常态。
    /// 结果是 `Sherlock Holmes` 与 `Holmes` 一对都进不了审阅队列。
    #[test]
    fn the_same_type_is_a_recall_candidate() {
        for k in ["person", "location", "organization", "product", "event"] {
            assert_eq!(
                classify_type_drift(Some(k), Some(k)),
                TypeDrift::Recall,
                "{k} 对 {k} 必须可召回"
            );
        }
        // 自定义类型同样——这里判的是"两边是不是同一种东西"，不是"它在不在白名单里"
        assert_eq!(
            classify_type_drift(Some("drug"), Some("drug")),
            TypeDrift::Recall
        );
        // 跨类型的老结论一条都不变
        assert_eq!(
            classify_type_drift(Some("person"), Some("organization")),
            TypeDrift::Disjoint
        );
    }
}
