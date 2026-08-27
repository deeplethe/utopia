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
    let strip_punct = |w: &str| w.trim_end_matches(|c| c == '.' || c == ',').to_string();
    let generic =
        |s: &str| GENERIC_SUFFIXES_CJK.contains(&s) || GENERIC_WORDS_EN.contains(&strip_punct(s).as_str());
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
// 类型漂移：同名实体被抽成了不同类型（"Orion platform team" ↔ organization/project/concept）
// ---------------------------------------------------------------------------

/// 抽取兜底类型：白名单外类型与漏报的主宾都降级到 concept（见 extraction），
/// 因此 concept ↔ 具体类型 的同名大概率是同一实体的类型漂移，按召回候选参与画像分层。
pub const FALLBACK_TYPE_KEY: &str = "concept";

/// 易混具体类型：抽取常在这几类间摇摆（一个团队算组织还是项目？平台算项目还是产品？）。
/// 同名跨这组类型 → 照建实体（宁分勿合），但入队审核对交 LLM/人工裁决。
/// DESIGN.md §3.2 的 disjoint 公理目前只是设计面（0005 仅落了 subClassOf 数据面），
/// 公理落库后这张表应改从本体读取。
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

fn classify_type_drift(a: &str, b: &str) -> TypeDrift {
    if a == FALLBACK_TYPE_KEY || b == FALLBACK_TYPE_KEY {
        return TypeDrift::Recall;
    }
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
    type_id: Uuid,
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
        let best = candidates.iter().max_by_key(|c| c.degree).expect("non-empty");
        touch_entity(pool, best.id).await?;
        return Ok(Resolution { entity_id: best.id, created: false, reviews: Vec::new() });
    };

    // 有画像的候选算相似度；无画像（历史数据/无 embedding 期创建）单独归类
    let mut best_scored: Option<(&Candidate, f32)> = None;
    let mut unprofiled: Option<&Candidate> = None;
    for c in &candidates {
        match c.profile_embedding.as_ref().and_then(|p| cosine(p.as_slice(), ctx)) {
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
            return Ok(Resolution { entity_id: best.id, created: false, reviews: Vec::new() });
        }
    }
    if let Some(c) = unprofiled {
        // 无画像候选无从判别：v1 兼容归并，并用本次上下文初始化画像
        update_profile(pool, c.id, c.profile_n, ctx).await?;
        return Ok(Resolution { entity_id: c.id, created: false, reviews: Vec::new() });
    }

    // 走到这里：所有候选都有画像且最高分 < ATTACH → 新建实体（同名不同人）
    let id = create_entity(pool, kb_id, type_id, &name, context).await?;
    refresh_disambiguators(pool, kb_id, &name).await?;
    let reviews = best_scored
        .filter(|(_, sim)| *sim >= SIM_NEW)
        .map(|(c, sim)| {
            vec![ReviewRequest {
                other_id: c.id,
                score: sim,
                reason: format!("ambiguous name match (context similarity {sim:.2})"),
            }]
        })
        .unwrap_or_default();
    Ok(Resolution { entity_id: id, created: true, reviews })
}

#[derive(Debug, sqlx::FromRow)]
struct CrossCandidate {
    id: Uuid,
    canonical_name: String,
    type_key: String,
    profile_embedding: Option<Vector>,
    profile_n: i32,
}

fn drift_reason(mention_key: &str, other_key: &str, sim: Option<f32>) -> String {
    match sim {
        Some(s) => format!("type drift: {mention_key} vs {other_key} (context similarity {s:.2})"),
        None => format!("type drift: {mention_key} vs {other_key} (same name, no context to compare)"),
    }
}

fn confusable_reviews(
    mention_key: &str,
    cands: &[&CrossCandidate],
    ctx: Option<&[f32]>,
) -> Vec<ReviewRequest> {
    cands
        .iter()
        .map(|c| {
            let sim = ctx
                .and_then(|x| c.profile_embedding.as_ref().and_then(|p| cosine(p.as_slice(), x)));
            ReviewRequest {
                other_id: c.id,
                score: sim.unwrap_or(0.0),
                reason: drift_reason(mention_key, &c.type_key, sim),
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
    type_id: Uuid,
    name: &str,
    keys: &[String],
    context: Option<&[f32]>,
) -> AppResult<Resolution> {
    let (mention_key,): (String,) = sqlx::query_as("SELECT key FROM entity_types WHERE id = $1")
        .bind(type_id)
        .fetch_one(pool)
        .await?;
    let cross: Vec<CrossCandidate> = sqlx::query_as(
        "SELECT e.id, e.canonical_name, t.key AS type_key, e.profile_embedding, e.profile_n
         FROM entities e JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND e.type_id <> $2 AND e.merged_into IS NULL
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
        match classify_type_drift(&mention_key, &c.type_key) {
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
                if best.type_key == FALLBACK_TYPE_KEY && mention_key != FALLBACK_TYPE_KEY {
                    // concept 只是"没认出来"，具体类型是更强的判断 → 升格。
                    // 不是合并（没有第二个实体），不入 entity_merges；本体页可手工改回。
                    sqlx::query(
                        "UPDATE entities SET type_id = $2, updated_at = now() WHERE id = $1",
                    )
                    .bind(best.id)
                    .bind(type_id)
                    .execute(pool)
                    .await?;
                    // 类型标签兜底的消歧后缀可能已过时
                    refresh_disambiguators(pool, kb_id, &best.canonical_name).await?;
                }
                // mention 已定居到召回实体；同名易混类型实体的疑点仍在 → 照常入队
                let mut reviews = confusable_reviews(&mention_key, &review_cands, Some(ctx));
                reviews.truncate(MAX_DRIFT_REVIEWS);
                return Ok(Resolution { entity_id: best.id, created: false, reviews });
            }
        }
    }

    let id = create_entity(pool, kb_id, type_id, name, context).await?;
    if !cross.is_empty() {
        // 跨类型同名并存：消歧后缀按名字分组（不分类型），需要刷新
        refresh_disambiguators(pool, kb_id, name).await?;
    }
    let mut reviews = confusable_reviews(&mention_key, &review_cands, context);
    for c in &recall_cands {
        let sim = context
            .and_then(|ctx| c.profile_embedding.as_ref().and_then(|p| cosine(p.as_slice(), ctx)));
        match sim {
            // 画像明确不像：完全分开，不打扰审核队列
            Some(s) if s < SIM_NEW => {}
            // 灰区或无从判别（无 embedding / 候选无画像）：宁分勿合 + 审核对
            _ => reviews.push(ReviewRequest {
                other_id: c.id,
                score: sim.unwrap_or(0.0),
                reason: drift_reason(&mention_key, &c.type_key, sim),
            }),
        }
    }
    reviews.truncate(MAX_DRIFT_REVIEWS);
    Ok(Resolution { entity_id: id, created: true, reviews })
}

async fn create_entity(
    pool: &PgPool,
    kb_id: Uuid,
    type_id: Uuid,
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
        let disambiguator = match label {
            Some((l,)) => l,
            None => {
                let (type_label,): (String,) = sqlx::query_as(
                    "SELECT t.label FROM entities e JOIN entity_types t ON t.id = e.type_id
                     WHERE e.id = $1",
                )
                .bind(id)
                .fetch_one(pool)
                .await?;
                type_label
            }
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
        type_label: String,
        color: String,
        disambiguator: Option<String>,
        degree: i64,
    }
    let row: SideRow = sqlx::query_as(
        "SELECT e.id, e.canonical_name AS name, t.label AS type_label, t.color, e.disambiguator,
                (SELECT count(*) FROM facts f
                 WHERE (f.subject_id = e.id OR f.object_id = e.id)
                   AND f.invalidated_at IS NULL) AS degree
         FROM entities e JOIN entity_types t ON t.id = e.type_id
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
                r.label AS predicate_label, o.canonical_name AS other_name,
                f.valid_from, f.valid_to
         FROM facts f
         JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o
           ON o.id = CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
           AND (f.subject_id = $2 OR f.object_id = $2)
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

async fn assemble_reviews(pool: &PgPool, kb_id: Uuid, rows: Vec<ReviewRow>) -> AppResult<Vec<ReviewItem>> {
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
pub async fn list_reviews(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ReviewItem>> {
    let rows: Vec<ReviewRow> = sqlx::query_as(
        "SELECT id, left_id, right_id, score, reason, stage, created_at
         FROM resolution_reviews
         WHERE kb_id = $1 AND status = 'pending'
         ORDER BY created_at DESC LIMIT 100",
    )
    .bind(kb_id)
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
            merge_entities(pool, kb_id, source, target, Some(user_id), "review decision").await?;
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
    Ok(if deg_a > deg_b || (deg_a == deg_b && a < b) { (a, b) } else { (b, a) })
}

// ---------------------------------------------------------------------------
// 合并 / 回滚
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct EntityFull {
    type_id: Uuid,
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
        return Err(AppError::Validation("Cannot merge an entity into itself".into()));
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

    // 合并后 SPO+valid_from 重复的 live 事实：留最早 recorded_at 的一条，其余作废
    let dups: Vec<(Vec<Uuid>,)> = sqlx::query_as(
        "SELECT (array_agg(id ORDER BY recorded_at))[2:] FROM facts
         WHERE kb_id = $1 AND invalidated_at IS NULL
           AND (subject_id = $2 OR object_id = $2)
         GROUP BY subject_id, predicate_id, object_id, valid_from
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
        if !taken.contains(&a.to_lowercase()) && !aliases.iter().any(|x| x.eq_ignore_ascii_case(a)) {
            aliases.push(a.clone());
        }
    }

    // 画像加权合并
    let (profile, profile_n) = match (&target.profile_embedding, &source.profile_embedding) {
        (Some(t), Some(s)) if t.as_slice().len() == s.as_slice().len() => {
            let (nt, ns) = (target.profile_n.max(1) as f32, source.profile_n.max(1) as f32);
            let merged: Vec<f32> = t
                .as_slice()
                .iter()
                .zip(s.as_slice())
                .map(|(a, b)| (a * nt + b * ns) / (nt + ns))
                .collect();
            (Some(Vector::from(merged)), target.profile_n + source.profile_n)
        }
        (Some(t), _) => (Some(t.clone()), target.profile_n),
        (None, Some(s)) => (Some(s.clone()), source.profile_n),
        (None, None) => (None, 0),
    };

    // 类型调和：concept 是抽取兜底而非类型判断，被并侧带具体类型时目标升格；
    // 两个具体类型相并（易混对被裁"same"）保留 target（存活方）的类型。
    let concept_type: Option<Uuid> =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM entity_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(FALLBACK_TYPE_KEY)
            .fetch_optional(&mut *tx)
            .await?
            .map(|(id,)| id);
    let new_type_id = if concept_type == Some(target.type_id) && concept_type != Some(source.type_id)
    {
        source.type_id
    } else {
        target.type_id
    };

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

    // 其余涉及 source 的 pending 审核项关闭（合并后过时；疑点若仍在会由后续 mention 重新提起）。
    // (source, target) 这一对本身除外——它正是本次合并的裁决对象，由调用方标记 merged，
    // 在这里扫掉会让它在历史里错误地显示为"保持分开"
    sqlx::query(
        "UPDATE resolution_reviews
         SET status = 'kept', reason = 'superseded by merge', decided_at = now()
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
    let moved_all: Vec<Uuid> =
        moved_subject.iter().chain(moved_object.iter()).copied().collect();
    let report = crate::temporal::reconcile_moved_facts(pool, kb_id, &moved_all).await?;
    if !report.corrected.is_empty() {
        sqlx::query("UPDATE entity_merges SET temporal_corrections = $2 WHERE id = $1")
            .bind(merge_id)
            .bind(&report.corrected)
            .execute(pool)
            .await?;
    }

    refresh_disambiguators(pool, kb_id, &source.canonical_name).await?;
    if !source.canonical_name.eq_ignore_ascii_case(&target.canonical_name) {
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
pub async fn list_merges(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<MergeLogView>> {
    let rows: Vec<MergeLogView> = sqlx::query_as(
        "SELECT m.id, s.canonical_name AS source_name, t.canonical_name AS target_name,
                u.display_name AS merged_by_name, m.reason, m.created_at, m.reverted_at
         FROM entity_merges m
         JOIN entities s ON s.id = m.source_id
         JOIN entities t ON t.id = m.target_id
         LEFT JOIN users u ON u.id = m.merged_by
         WHERE m.kb_id = $1
         ORDER BY m.created_at DESC LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
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
        // 兜底型任意一侧 → 召回候选
        assert_eq!(classify_type_drift("concept", "organization"), TypeDrift::Recall);
        assert_eq!(classify_type_drift("project", "concept"), TypeDrift::Recall);
        assert_eq!(classify_type_drift("concept", "person"), TypeDrift::Recall);
        // 易混具体类型两两 → 审核对
        assert_eq!(classify_type_drift("organization", "project"), TypeDrift::Review);
        assert_eq!(classify_type_drift("project", "product"), TypeDrift::Review);
        assert_eq!(classify_type_drift("product", "organization"), TypeDrift::Review);
        // 硬互斥与未知自定义类型 → 完全分开
        assert_eq!(classify_type_drift("person", "organization"), TypeDrift::Disjoint);
        assert_eq!(classify_type_drift("person", "project"), TypeDrift::Disjoint);
        assert_eq!(classify_type_drift("event", "location"), TypeDrift::Disjoint);
        assert_eq!(classify_type_drift("team", "organization"), TypeDrift::Disjoint);
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]).unwrap() - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).unwrap().abs() < 1e-6);
        assert!(cosine(&[1.0], &[1.0, 2.0]).is_none());
        assert!(cosine(&[0.0, 0.0], &[1.0, 1.0]).is_none());
    }
}
