//! 治理（0025）：agent 先读台账再裁。
//!
//! 三件事：**先例**——这个库里的人对这一对、这个名字、这种类型对做过什么；
//! **队列**——等人的重复对按先进先出排，队头带着它的簇（同名或同实体的其他对）
//! 一起走；**闸门**——一个纯函数，说这条判决是自己动手还是写成建议。模型调用在
//! server 的 `governance` 任务里，这里不碰模型。
//!
//! 先例只认人的决定（actor 不为空）。agent 自己裁过的不算——否则它引用自己，
//! 越判越自信。人接受了 agent 的建议算：那一笔是人签的，走的是人的裁决路径。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use utopia_core::models::{AgentDecisionView, ReviewItem};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

use crate::resolution::{assemble_reviews, ReviewRow};

/// 自己动手的置信度线。比攒批裁决器的 0.8 高一点：那一档只看这一对，这里是替人做主
pub const AUTO_CONF: f32 = 0.85;
/// 历史同意时的线：人对这一对、这个名字或这种类型对做过同样的决定，agent 的把握可以低一点
pub const SUPPORTED_CONF: f32 = 0.75;
/// 类型对的习惯要有多少笔人的决定才算数
pub const TYPE_PAIR_MIN: i64 = 5;
/// 每族先例最多带几条进提示词
const PER_FAMILY: i64 = 8;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Precedent {
    pub event_id: Uuid,
    /// review.merge | review.keep | merge.manual | merge.revert
    pub action: String,
    pub left: String,
    pub right: String,
    pub at: DateTime<Utc>,
    /// 人拍板时写的那一句（0026）。**先例带着理由才是先例**：只有结果的话，
    /// 一次错误的合并会被读成"这类该合"，错误洗成政策。老行没有这一列，为空
    pub why: Option<String>,
}

impl Precedent {
    /// 这一笔是把两个记录合了，还是分开了
    pub fn merged(&self) -> bool {
        matches!(self.action.as_str(), "review.merge" | "merge.manual")
    }
}

#[derive(Debug, Clone, Default, Serialize, sqlx::FromRow)]
pub struct TypePairStats {
    pub merged: i64,
    pub kept: i64,
    pub reverted: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Precedents {
    /// 人对这两个名字（不分左右）做过的决定
    pub same_pair: Vec<Precedent>,
    /// 其中一个名字对别的名字：这个名字是重名磁铁，还是一直被合
    pub same_name: Vec<Precedent>,
    /// 这个库对这种类型对的习惯；一侧没类型就没有
    pub type_pair: Option<TypePairStats>,
    /// 涉及任一名字的撤回：有一笔就只建议不动手
    pub reverts: Vec<Precedent>,
}

/// 台账里两族 detail 的写法：review.* 记 left/right，merge.* 记 source/target
const L: &str = "lower(COALESCE(detail->>'left', detail->>'source', ''))";
const R: &str = "lower(COALESCE(detail->>'right', detail->>'target', ''))";
const COLS: &str = "id AS event_id, action,
         COALESCE(detail->>'left', detail->>'source', '') AS \"left\",
         COALESCE(detail->>'right', detail->>'target', '') AS \"right\",
         created_at AS at,
         NULLIF(detail->>'why', '') AS why";

pub async fn precedents_for(
    pool: &PgPool,
    kb_id: Uuid,
    item: &ReviewItem,
) -> AppResult<Precedents> {
    let (a, b) = (
        item.left.name.to_lowercase(),
        item.right.name.to_lowercase(),
    );
    let is_pair = format!("(({L} = $2 AND {R} = $3) OR ({L} = $3 AND {R} = $2))");
    let touches = format!("({L} IN ($2, $3) OR {R} IN ($2, $3))");

    let same_pair: Vec<Precedent> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM audit_events
         WHERE kb_id = $1 AND actor_id IS NOT NULL
           AND action IN ('review.merge', 'review.keep', 'merge.manual') AND {is_pair}
         ORDER BY created_at DESC LIMIT $4"
    ))
    .bind(kb_id)
    .bind(&a)
    .bind(&b)
    .bind(PER_FAMILY)
    .fetch_all(pool)
    .await?;

    let same_name: Vec<Precedent> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM audit_events
         WHERE kb_id = $1 AND actor_id IS NOT NULL
           AND action IN ('review.merge', 'review.keep', 'merge.manual')
           AND {touches} AND NOT {is_pair}
         ORDER BY created_at DESC LIMIT $4"
    ))
    .bind(kb_id)
    .bind(&a)
    .bind(&b)
    .bind(PER_FAMILY)
    .fetch_all(pool)
    .await?;

    let reverts: Vec<Precedent> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM audit_events
         WHERE kb_id = $1 AND action = 'merge.revert' AND {touches}
         ORDER BY created_at DESC LIMIT $4"
    ))
    .bind(kb_id)
    .bind(&a)
    .bind(&b)
    .bind(PER_FAMILY)
    .fetch_all(pool)
    .await?;

    let types: Vec<(Uuid, Option<Uuid>)> =
        sqlx::query_as("SELECT id, type_id FROM entities WHERE id = ANY($1)")
            .bind(vec![item.left.id, item.right.id])
            .fetch_all(pool)
            .await?;
    let type_of = |id: Uuid| types.iter().find(|t| t.0 == id).and_then(|t| t.1);
    let type_pair = match (type_of(item.left.id), type_of(item.right.id)) {
        (Some(ta), Some(tb)) => Some(type_pair_stats(pool, kb_id, ta, tb).await?),
        _ => None,
    };

    Ok(Precedents {
        same_pair,
        same_name,
        type_pair,
        reverts,
    })
}

/// 这个库对这种类型对的习惯：人合了多少、分了多少、合了又撤回多少
async fn type_pair_stats(
    pool: &PgPool,
    kb_id: Uuid,
    ta: Uuid,
    tb: Uuid,
) -> AppResult<TypePairStats> {
    let (merged, kept): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE rr.status = 'merged'),
                count(*) FILTER (WHERE rr.status = 'kept')
         FROM resolution_reviews rr
         JOIN entities a ON a.id = rr.left_id
         JOIN entities b ON b.id = rr.right_id
         WHERE rr.kb_id = $1 AND rr.decided_by IS NOT NULL
           AND ((a.type_id = $2 AND b.type_id = $3) OR (a.type_id = $3 AND b.type_id = $2))",
    )
    .bind(kb_id)
    .bind(ta)
    .bind(tb)
    .fetch_one(pool)
    .await?;
    let reverted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entity_merges m
         JOIN entities s ON s.id = m.source_id
         JOIN entities t ON t.id = m.target_id
         WHERE m.kb_id = $1 AND m.reverted_at IS NOT NULL
           AND ((s.type_id = $2 AND t.type_id = $3) OR (s.type_id = $3 AND t.type_id = $2))",
    )
    .bind(kb_id)
    .bind(ta)
    .bind(tb)
    .fetch_one(pool)
    .await?;
    Ok(TypePairStats {
        merged,
        kept,
        reverted,
    })
}

/// 先例写成提示词里的行。英文：读者是模型
pub fn render_lines(p: &Precedents) -> Vec<String> {
    let verb = |x: &Precedent| if x.merged() { "merged" } else { "kept apart" };
    let day = |t: &DateTime<Utc>| t.format("%Y-%m-%d").to_string();
    // 人写了理由就带上：模型该学的是「凭什么」，不是「多半怎么判」
    let because = |x: &Precedent| {
        x.why
            .as_deref()
            .map(|w| format!("; they wrote: \"{w}\""))
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for x in &p.same_pair {
        out.push(format!(
            "this same pair was {} by a person on {}{}",
            verb(x),
            day(&x.at),
            because(x)
        ));
    }
    for x in &p.same_name {
        out.push(format!(
            "\"{}\" against \"{}\" was {} by a person on {}{}",
            x.left,
            x.right,
            verb(x),
            day(&x.at),
            because(x)
        ));
    }
    if let Some(t) = &p.type_pair {
        if t.merged + t.kept > 0 {
            out.push(format!(
                "for pairs of these two types in this base, people merged {}, kept {} apart, and later reverted {} merge(s)",
                t.merged, t.kept, t.reverted
            ));
        }
    }
    for x in &p.reverts {
        out.push(format!(
            "a merge of \"{}\" into \"{}\" was reverted by a person on {}{}",
            x.left,
            x.right,
            day(&x.at),
            because(x)
        ));
    }
    out
}

/// 落进 agent_decisions.precedents 的样子：每条带一个 family
pub fn precedents_json(p: &Precedents) -> serde_json::Value {
    let tag = |family: &str, xs: &[Precedent]| {
        xs.iter()
            .map(|x| {
                serde_json::json!({
                    "family": family, "event_id": x.event_id, "action": x.action,
                    "left": x.left, "right": x.right, "at": x.at, "why": x.why,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut all = tag("same_pair", &p.same_pair);
    all.extend(tag("same_name", &p.same_name));
    all.extend(tag("revert", &p.reverts));
    if let Some(t) = &p.type_pair {
        all.push(serde_json::json!({
            "family": "type_pair", "merged": t.merged, "kept": t.kept, "reverted": t.reverted,
        }));
    }
    serde_json::Value::Array(all)
}

/// 名字的形状：不靠模型也能看出来的那几种
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameShape {
    /// 一字不差（忽略大小写与开头的 the）
    Identical,
    /// 一个是另一个去掉了前面的限定词：Google DeepMind / DeepMind
    Abbreviation,
    /// 一个是另一个加了公司后缀：Apple Inc. / Apple、Thrive Capital / Thrive
    Suffix,
    /// 一个是另一个加了版本、届次：Claude 4 / Claude、AlphaFold2 / AlphaFold、2024 IMO / IMO
    Version,
    /// 一个是另一个加了一两个词：DeepMind Health / DeepMind、Gemini Robotics-ER / Gemini Robotics
    Extension,
    /// 一个是含着另一个的一句话：Sam Altman's efforts / Sam Altman；或列表 / 成员
    Phrase,
    /// 互不包含
    Unrelated,
}

fn norm_name(s: &str) -> String {
    let s = s.trim().to_lowercase();
    let s = s.strip_prefix("the ").unwrap_or(&s).to_string();
    // 末尾括号里的缩写不是版本：reinforcement learning (RL)、Department of Defense (DoD)
    let s = match (s.rfind(" ("), s.ends_with(')')) {
        (Some(i), true)
            if s[i + 2..s.len() - 1].chars().all(|c| c.is_alphanumeric()) && s.len() - i <= 12 =>
        {
            s[..i].to_string()
        }
        _ => s,
    };
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

const CORPORATE_SUFFIX: &[&str] = &[
    "inc",
    "ltd",
    "llc",
    "plc",
    "corp",
    "corporation",
    "co",
    "company",
    "capital",
    "group",
    "platforms",
    "industries",
    "technologies",
    "holdings",
    "limited",
    "gmbh",
    "sa",
    "ag",
];

pub fn name_shape(a: &str, b: &str) -> NameShape {
    let (a, b) = (norm_name(a), norm_name(b));
    if a == b {
        return NameShape::Identical;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    // 要整词出现：Time 在 Financial Times 里不算（后面跟着字母），AlphaFold 在 AlphaFold2
    // 里算（后面跟着数字，那是版本）
    let word_at = |long: &str, short: &str| -> Option<usize> {
        let mut from = 0;
        while let Some(i) = long[from..].find(short) {
            let i = from + i;
            let before_ok = i == 0
                || !long[..i]
                    .chars()
                    .last()
                    .is_some_and(|c| c.is_alphanumeric());
            // 后面跟着字母不算整词——除非只多一个复数的 s：sorting algorithms 含着
            // sorting algorithm，Times 含着 Time
            let rest = &long[i + short.len()..];
            let mut rest_chars = rest.chars();
            let after_ok = match (rest_chars.next(), rest_chars.next()) {
                (None, _) => true,
                (Some(c), _) if !c.is_alphabetic() => true,
                (Some('s'), None) => true,
                (Some('s'), Some(d)) => !d.is_alphanumeric(),
                _ => false,
            };
            if before_ok && after_ok {
                return Some(i);
            }
            // **前进一个字符，不是一个字节。** `i + 1` 落在多字节字符中间时，
            // 下一轮的 `long[from..]` 直接 panic——中文名字一撞上就炸，而这条
            // 路在裁决里跑，panic 掉的是整个任务：任务行永远停在 running，
            // 没有报错、没有重试，队列静默少一格（2026-09-08 实测）
            from = i + long[i..].chars().next().map_or(1, char::len_utf8);
        }
        None
    };
    let Some(pos) = word_at(long, short) else {
        return NameShape::Unrelated;
    };
    if short.is_empty() {
        return NameShape::Unrelated;
    }
    let words = |s: &str| {
        s.split(|c: char| !c.is_alphanumeric() && c != '\'')
            .filter(|w| !w.is_empty())
            .count()
    };
    let phrase_marker = |s: &str| {
        s.contains("'s ")
            || s.ends_with("'s")
            || [
                " of ",
                " by ",
                " from ",
                " for ",
                " against ",
                " with ",
                " to ",
                " in ",
                " on ",
                " at ",
                " and ",
                " or ",
                " led ",
                " into ",
            ]
            .iter()
            .any(|m| format!(" {s} ").contains(m))
    };
    if long.matches(", ").count() >= 2 {
        return NameShape::Phrase;
    }
    // 短的那个在长的里到哪里结束：复数的 s 算在里面
    let end = {
        let rest = &long[pos + short.len()..];
        if rest.starts_with('s')
            && rest[1..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
        {
            pos + short.len() + 1
        } else {
            pos + short.len()
        }
    };
    if end == long.len() {
        let front = long[..pos].trim().trim_end_matches([',', '-', ':']);
        if front.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return NameShape::Version;
        }
        if phrase_marker(front) || words(front) > 2 {
            return NameShape::Phrase;
        }
        return NameShape::Abbreviation;
    }
    if pos == 0 {
        let tail = long[end..]
            .trim()
            .trim_start_matches([',', '-', ':', '.', 'v']);
        let tail = tail.trim();
        if tail.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return NameShape::Version;
        }
        let tail_words: Vec<&str> = tail
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        if !tail_words.is_empty() && tail_words.iter().all(|w| CORPORATE_SUFFIX.contains(w)) {
            return NameShape::Suffix;
        }
        // 尾巴两个词起就是一句话了：DeepMind Health data sharing、OpenAI Ireland Ltd、
        // Meta Superintelligence Labs——都不是前面那个东西
        if phrase_marker(tail) || tail_words.len() > 1 {
            return NameShape::Phrase;
        }
        return NameShape::Extension;
    }
    NameShape::Phrase
}

/// 类型标签的大类：抽取器给的标签本身很吵（同一家公司在两篇里是 Organization 与
/// Corporation），只有大类不同才算冲突。None = 说不上是哪一类，与谁都不冲突
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFamily {
    Person,
    Organization,
    Place,
    Event,
}

pub fn type_family(label: &str) -> Option<TypeFamily> {
    let l = label.to_lowercase();
    if l == "person" || l == "researcher" || l.ends_with("person") {
        return Some(TypeFamily::Person);
    }
    if l.contains("event") {
        return Some(TypeFamily::Event);
    }
    if [
        "organization",
        "corporation",
        "business",
        "store",
        "hospital",
        "university",
        "college",
        "school",
        "consortium",
        "ngo",
        "party",
        "agency",
        "company",
        "bank",
        "fund",
        "newspaper",
        "governmentoffice",
        "team",
    ]
    .iter()
    .any(|k| l.contains(k))
    {
        return Some(TypeFamily::Organization);
    }
    if [
        "place",
        "city",
        "country",
        "state",
        "continent",
        "administrativearea",
        "region",
        "landmark",
        "civicstructure",
        "locality",
    ]
    .iter()
    .any(|k| l.contains(k))
    {
        return Some(TypeFamily::Place);
    }
    None
}

/// 两边的类型标签是不是不同的大类
pub fn types_conflict(a: Option<&str>, b: Option<&str>) -> bool {
    matches!(
        (a.and_then(type_family), b.and_then(type_family)),
        (Some(x), Some(y)) if x != y
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum Gate {
    /// 自己动手：合并可撤，分开可再合
    Apply,
    /// 写成建议留给人
    Propose,
}

impl Precedents {
    /// 历史同意这个判决：这一对被人这么裁过、这个名字每次都这么裁、或这种类型对有
    /// 这个习惯（够 TYPE_PAIR_MIN 笔；合并的习惯还要求没撤回过）
    pub fn supports(&self, same: bool) -> bool {
        let pair = self.same_pair.iter().any(|x| x.merged() == same);
        let name = !self.same_name.is_empty() && self.same_name.iter().all(|x| x.merged() == same);
        let habit = self.type_pair.as_ref().is_some_and(|t| {
            t.merged + t.kept >= TYPE_PAIR_MIN
                && if same {
                    t.merged >= t.kept && t.reverted == 0
                } else {
                    t.kept > t.merged
                }
        });
        pair || name || habit
    }
}

/// agent 按自己的把握判，历史是参考（0025 决定 4，2026-09-06 修订）。历史反对就拦住：
/// 人分开过这一对却说合、合过却说分、涉及任一名字的撤回。历史同意就把线放低。其余
/// 按 agent 自己的把握。类型冲突永远不合——那是规则，不是把握
pub fn gate(
    same: Option<bool>,
    conf: f32,
    types_conflict: bool,
    shape: NameShape,
    p: &Precedents,
) -> Gate {
    let Some(same) = same else {
        return Gate::Propose;
    };
    if !p.reverts.is_empty() || p.same_pair.iter().any(|x| x.merged() != same) {
        return Gate::Propose;
    }
    // 两条不靠模型的规则：类型的大类不同不合；版本尾巴或含着名字的一句话不自动合——
    // 模型在这两种上最爱说 same，而它们几乎从不是
    if same && (types_conflict || matches!(shape, NameShape::Version | NameShape::Phrase)) {
        return Gate::Propose;
    }
    let bar = if p.supports(same) {
        SUPPORTED_CONF
    } else {
        AUTO_CONF
    };
    if conf >= bar {
        Gate::Apply
    } else {
        Gate::Propose
    }
}

/// 同一对实体，人裁过没有：Some(true) 合过、Some(false) 分过、None 没裁过。
/// 按实体 id 认，不按名字——名字一样的两个「张伟」不是同一个问题
pub async fn decided_before(
    pool: &PgPool,
    kb_id: Uuid,
    a: Uuid,
    b: Uuid,
) -> AppResult<Option<bool>> {
    let merged: Option<bool> = sqlx::query_scalar(
        "SELECT status = 'merged' FROM resolution_reviews
         WHERE kb_id = $1 AND decided_by IS NOT NULL AND status IN ('merged', 'kept')
           AND ((left_id = $2 AND right_id = $3) OR (left_id = $3 AND right_id = $2))
         ORDER BY decided_at DESC LIMIT 1",
    )
    .bind(kb_id)
    .bind(a)
    .bind(b)
    .fetch_optional(pool)
    .await?;
    Ok(merged)
}

/// 人已经定过的一对，照人的定：同一对实体裁过；或者名字不同的一对名字，人对这一对
/// 名字的决定一致（「OpenAI OpCo ≟ OpenAI」合过就是合）。同名的一对不算——那只说明
/// 库里有过重名，不说明眼前这两个是谁。有撤回的不走这条路
pub fn settled_by_people(
    left_name: &str,
    right_name: &str,
    p: &Precedents,
    prior: Option<bool>,
) -> Option<bool> {
    if !p.reverts.is_empty() {
        return None;
    }
    if let Some(m) = prior {
        return Some(m);
    }
    if left_name.to_lowercase() == right_name.to_lowercase() {
        return None;
    }
    let mut it = p.same_pair.iter();
    let first = it.next()?.merged();
    it.all(|x| x.merged() == first).then_some(first)
}

/// 有一条开着的建议的对不进队列：agent 已经问过了，等人答
const OPEN_PROPOSAL: &str = "NOT EXISTS (SELECT 1 FROM agent_decisions d
    WHERE d.target_kind = 'review' AND d.target_id = rr.id AND d.status = 'proposed')";

/// 等人的重复对，先进先出
pub async fn queue(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<ReviewItem>> {
    let rows: Vec<ReviewRow> = sqlx::query_as(&format!(
        "SELECT rr.id, rr.left_id, rr.right_id, rr.score, rr.reason, rr.stage, rr.created_at
         FROM resolution_reviews rr
         WHERE rr.kb_id = $1 AND rr.status = 'pending' AND {OPEN_PROPOSAL}
         ORDER BY rr.created_at, rr.id LIMIT $2"
    ))
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    assemble_reviews(pool, kb_id, rows).await
}

/// 队头的簇：与它同名或同实体的其他等人的对，同样先进先出
pub async fn cluster_of(
    pool: &PgPool,
    kb_id: Uuid,
    head: &ReviewItem,
    limit: i64,
) -> AppResult<Vec<ReviewItem>> {
    let rows: Vec<ReviewRow> = sqlx::query_as(&format!(
        "SELECT rr.id, rr.left_id, rr.right_id, rr.score, rr.reason, rr.stage, rr.created_at
         FROM resolution_reviews rr
         JOIN entities a ON a.id = rr.left_id
         JOIN entities b ON b.id = rr.right_id
         WHERE rr.kb_id = $1 AND rr.status = 'pending' AND rr.id <> $2 AND {OPEN_PROPOSAL}
           AND (rr.left_id = ANY($3) OR rr.right_id = ANY($3)
                OR lower(a.canonical_name) = ANY($4) OR lower(b.canonical_name) = ANY($4))
         ORDER BY rr.created_at, rr.id LIMIT $5"
    ))
    .bind(kb_id)
    .bind(head.id)
    .bind(vec![head.left.id, head.right.id])
    .bind(vec![
        head.left.name.to_lowercase(),
        head.right.name.to_lowercase(),
    ])
    .bind(limit)
    .fetch_all(pool)
    .await?;
    assemble_reviews(pool, kb_id, rows).await
}

pub struct NewDecision<'a> {
    pub run_id: Uuid,
    pub target_id: Uuid,
    /// merge | keep | unsure
    pub action: &'a str,
    pub confidence: f32,
    pub reason: Option<&'a str>,
    pub precedents: serde_json::Value,
    /// proposed | applied
    pub status: &'a str,
    pub merge_id: Option<Uuid>,
    /// defer 留给人的问题（第二刀）
    pub question: Option<&'a str>,
    /// 循环看了什么：[{tool, args, note}]
    pub trace: serde_json::Value,
    /// 循环里花的模型调用
    pub calls: i32,
}

pub async fn record(pool: &PgPool, kb_id: Uuid, d: NewDecision<'_>) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_decisions
            (id, kb_id, run_id, target_kind, target_id, action, confidence, reason,
             precedents, status, merge_id, question, trace, calls)
         VALUES ($1, $2, $3, 'review', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(d.run_id)
    .bind(d.target_id)
    .bind(d.action)
    .bind(d.confidence)
    .bind(d.reason)
    .bind(d.precedents)
    .bind(d.status)
    .bind(d.merge_id)
    .bind(d.question)
    .bind(d.trace)
    .bind(d.calls)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 今天这个库在循环里花了几次模型调用：每库每天的预算按它算
pub async fn loop_calls_today(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COALESCE(sum(calls), 0)::bigint FROM agent_decisions
         WHERE kb_id = $1 AND created_at >= date_trunc('day', now())",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 工具：提到这个实体的原文片段——它的事实是从哪句话里抽出来的，出自哪份文档。
/// 证据上有 quote 就用 quote，没有就截分块开头。
///
/// **名字事实排在后面，引文就是名字本身的不要**（0041）。名字事实置信度 1.0，
/// 按置信度排它每块都赢，而「这一块写了它的名字」的引文往往只是那个名字——
/// 给裁决的 agent 一串光秃秃的名字，等于没给原文。带整句引文的别名（「简称海探1」）
/// 照样留着，那正是判断是不是一个东西要看的
pub async fn quotes_of(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    limit: i64,
) -> AppResult<Vec<(String, String)>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT filename, quote FROM (
           SELECT DISTINCT ON (fe.chunk_id) d.filename,
                  COALESCE(NULLIF(fe.quote, ''), left(c.text, 240)) AS quote
           FROM facts f
           -- 谓词可以为空（0010），LEFT JOIN 才不丢那些事实
           LEFT JOIN relation_types r ON r.id = f.predicate_id
           JOIN fact_evidence fe ON fe.fact_id = f.id
           JOIN chunks c ON c.id = fe.chunk_id
           JOIN documents d ON d.id = c.document_id
           WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
             AND (f.subject_id = $2 OR f.object_id = $2)
             AND NOT (coalesce(r.builtin AND r.key = 'known_as', false)
                      AND lower(trim(coalesce(fe.quote, ''))) = lower(f.object_value->>'value'))
           ORDER BY fe.chunk_id, coalesce(r.builtin AND r.key = 'known_as', false), f.confidence DESC
         ) q
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(entity_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 工具：台账里人对某个名字做过的决定——名字包含 query 的合并、分开与撤回
pub async fn ledger_search(
    pool: &PgPool,
    kb_id: Uuid,
    query: &str,
    limit: i64,
) -> AppResult<Vec<Precedent>> {
    let like = format!("%{}%", query.trim().to_lowercase());
    let rows: Vec<Precedent> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM audit_events
         WHERE kb_id = $1 AND actor_id IS NOT NULL
           AND action IN ('review.merge', 'review.keep', 'merge.manual', 'merge.revert')
           AND ({L} LIKE $2 OR {R} LIKE $2)
         ORDER BY created_at DESC LIMIT $3"
    ))
    .bind(kb_id)
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 工具：库里名字包含 query 的其他实体——叫这个名字的有几个、各是什么、各有多少事实
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Namesake {
    pub name: String,
    pub type_label: Option<String>,
    pub facts: i64,
    pub merged: bool,
}

pub async fn namesakes(
    pool: &PgPool,
    kb_id: Uuid,
    query: &str,
    limit: i64,
) -> AppResult<Vec<Namesake>> {
    let like = format!("%{}%", query.trim().to_lowercase());
    let rows: Vec<Namesake> = sqlx::query_as(
        "SELECT e.canonical_name AS name, t.label AS type_label,
                (SELECT count(*) FROM facts f
                  WHERE f.invalidated_at IS NULL
                    AND (f.subject_id = e.id OR f.object_id = e.id)) AS facts,
                (e.merged_into IS NOT NULL) AS merged
         FROM entities e
         LEFT JOIN entity_types t ON t.id = e.type_id
         WHERE e.kb_id = $1 AND lower(e.canonical_name) LIKE $2
         ORDER BY e.merged_into IS NOT NULL, facts DESC, e.canonical_name
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(&like)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

const VIEW: &str = "SELECT d.id, d.run_id, d.target_kind, d.target_id, d.action, d.confidence,
        d.reason, d.precedents, d.status, d.merge_id, d.question, d.trace, d.calls,
        d.created_at, d.decided_at,
        u.display_name AS decided_by_name,
        a.canonical_name AS \"left\", b.canonical_name AS \"right\"
    FROM agent_decisions d
    LEFT JOIN users u ON u.id = d.decided_by
    LEFT JOIN resolution_reviews rr ON rr.id = d.target_id
    LEFT JOIN entities a ON a.id = rr.left_id
    LEFT JOIN entities b ON b.id = rr.right_id";

/// Agent 队列：最新的在前
pub async fn list(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<AgentDecisionView>> {
    let rows = sqlx::query_as(&format!(
        "{VIEW} WHERE d.kb_id = $1 ORDER BY d.created_at DESC, d.id DESC LIMIT $2 OFFSET $3"
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<AgentDecisionView> {
    sqlx::query_as(&format!("{VIEW} WHERE d.kb_id = $1 AND d.id = $2"))
        .bind(kb_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// 这个库的 govern 任务此刻在跑
pub async fn agent_running(pool: &PgPool, kb_id: Uuid) -> AppResult<bool> {
    let running = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = 'govern' AND status = 'running'
                          AND payload->>'kb_id' = $1)",
    )
    .bind(kb_id.to_string())
    .fetch_one(pool)
    .await?;
    Ok(running)
}

/// 还没轮到 agent 看的对：等人的、还没有开着的建议的
pub async fn queue_len(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    let n = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM resolution_reviews rr
         WHERE rr.kb_id = $1 AND rr.status = 'pending' AND {OPEN_PROPOSAL}"
    ))
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// agent 正在裁的一簇：标成 adjudicating，界面上按钮灰掉、接口上拒绝人改——模型
/// 调用在飞的时候人把一对裁了，落地那一步会撞上
pub async fn lock(pool: &PgPool, kb_id: Uuid, ids: &[Uuid]) -> AppResult<()> {
    sqlx::query(
        "UPDATE resolution_reviews SET stage = 'adjudicating'
         WHERE kb_id = $1 AND id = ANY($2) AND status = 'pending'",
    )
    .bind(kb_id)
    .bind(ids)
    .execute(pool)
    .await?;
    Ok(())
}

/// 任务开始与结束时放开所有锁：一轮中途出错、开关关掉，都不能把对锁死
pub async fn release_locks(pool: &PgPool, kb_id: Uuid) -> AppResult<u64> {
    let n = sqlx::query(
        "UPDATE resolution_reviews SET stage = 'human'
         WHERE kb_id = $1 AND status = 'pending' AND stage = 'adjudicating'",
    )
    .bind(kb_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// 这一对此刻是不是 agent 正在裁的：治理开着、任务在跑、这一对标着 adjudicating。
/// 三个条件缺一个都不算锁——任务没在跑的 adjudicating 只是旧裁决器留下的标记，人照样能裁
pub async fn locked_by_agent(pool: &PgPool, kb_id: Uuid, review_id: Uuid) -> AppResult<bool> {
    let locked = sqlx::query_scalar(
        "SELECT EXISTS (
            SELECT 1 FROM resolution_reviews rr
            JOIN knowledge_bases k ON k.id = rr.kb_id
            WHERE rr.id = $2 AND rr.kb_id = $1 AND rr.status = 'pending'
              AND rr.stage = 'adjudicating' AND k.governance
              AND EXISTS (SELECT 1 FROM jobs j WHERE j.kind = 'govern' AND j.status = 'running'
                            AND j.payload->>'kb_id' = $1::text))",
    )
    .bind(kb_id)
    .bind(review_id)
    .fetch_one(pool)
    .await?;
    Ok(locked)
}

pub async fn count_proposed(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    let n = sqlx::query_scalar(
        "SELECT count(*) FROM agent_decisions WHERE kb_id = $1 AND status = 'proposed'",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 人的回答：accepted / overridden 答一条建议，reverted / overridden 答一条自动裁决。
/// 已经答过的不能再答——那是另一个人刚做的决定
pub async fn settle(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    status: &str,
    user_id: Uuid,
) -> AppResult<()> {
    let n = sqlx::query(
        "UPDATE agent_decisions SET status = $3, decided_at = now(), decided_by = $4
         WHERE kb_id = $1 AND id = $2 AND status IN ('proposed', 'applied')",
    )
    .bind(kb_id)
    .bind(id)
    .bind(status)
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::Conflict(
            "this decision has already been answered".into(),
        ));
    }
    Ok(())
}

/// 人在卡片上裁了一对：这一对上开着的建议就此有了答案——动作与建议相同是
/// accepted，不同是 overridden；没有开着的建议就什么都不做
pub async fn answer_open(
    pool: &PgPool,
    kb_id: Uuid,
    review_id: Uuid,
    action: &str,
    user_id: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE agent_decisions
            SET status = CASE WHEN action = $3 THEN 'accepted' ELSE 'overridden' END,
                decided_at = now(), decided_by = $4
          WHERE kb_id = $1 AND target_kind = 'review' AND target_id = $2 AND status = 'proposed'",
    )
    .bind(kb_id)
    .bind(review_id)
    .bind(action)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 人裁了一对：同名或同实体的其他对上开着的建议过时了——那些建议是在没有这笔
/// 先例时写的。标成 superseded，那些对回到队列，下一轮带着新先例再看
pub async fn supersede_siblings(pool: &PgPool, kb_id: Uuid, review_id: Uuid) -> AppResult<u64> {
    let n = sqlx::query(
        "UPDATE agent_decisions d SET status = 'superseded', decided_at = now()
         FROM resolution_reviews me
              JOIN entities ma ON ma.id = me.left_id
              JOIN entities mb ON mb.id = me.right_id,
              resolution_reviews rr
              JOIN entities a ON a.id = rr.left_id
              JOIN entities b ON b.id = rr.right_id
         WHERE me.id = $2 AND me.kb_id = $1
           AND d.kb_id = $1 AND d.status = 'proposed'
           AND d.target_kind = 'review' AND d.target_id = rr.id AND rr.id <> me.id
           AND (rr.left_id IN (me.left_id, me.right_id) OR rr.right_id IN (me.left_id, me.right_id)
                OR lower(a.canonical_name) IN (lower(ma.canonical_name), lower(mb.canonical_name))
                OR lower(b.canonical_name) IN (lower(ma.canonical_name), lower(mb.canonical_name)))",
    )
    .bind(kb_id)
    .bind(review_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n)
}

/// 保险丝（0025 决定 9）：多少天里撤回几次就跳
pub const FUSE_WINDOW_DAYS: i64 = 7;
pub const FUSE_REVERTS: i64 = 2;

/// 从某一刻起，人撤回了 agent 的自动合并几次
pub async fn reverts_since(pool: &PgPool, kb_id: Uuid, since: DateTime<Utc>) -> AppResult<i64> {
    let n = sqlx::query_scalar(
        "SELECT count(*) FROM agent_decisions
         WHERE kb_id = $1 AND status = 'reverted' AND decided_at >= $2",
    )
    .bind(kb_id)
    .bind(since)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 保险丝跳闸：开关开着就关掉。回 true 表示这一次真的关了——告警只发一次
pub async fn trip(pool: &PgPool, kb_id: Uuid) -> AppResult<bool> {
    let n = sqlx::query(
        "UPDATE knowledge_bases SET governance = FALSE, updated_at = now()
         WHERE id = $1 AND governance",
    )
    .bind(kb_id)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// 人从合并历史撤回了一条合并：如果那是 agent 自己裁的，那一行记成 reverted。
/// 回那一行的 id；不是 agent 的合并就是 None
pub async fn settle_by_merge(
    pool: &PgPool,
    kb_id: Uuid,
    merge_id: Uuid,
    user_id: Uuid,
) -> AppResult<Option<Uuid>> {
    let id = sqlx::query_scalar(
        "UPDATE agent_decisions SET status = 'reverted', decided_at = now(), decided_by = $3
         WHERE kb_id = $1 AND merge_id = $2 AND status = 'applied'
         RETURNING id",
    )
    .bind(kb_id)
    .bind(merge_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(id)
}

/// 开关开着、队列里还有 agent 没看过的对的库：定时扫描用
pub async fn due(pool: &PgPool) -> AppResult<Vec<Uuid>> {
    let ids = sqlx::query_scalar(&format!(
        "SELECT kb.id FROM knowledge_bases kb
         WHERE kb.governance AND EXISTS (
             SELECT 1 FROM resolution_reviews rr
             WHERE rr.kb_id = kb.id AND rr.status = 'pending' AND {OPEN_PROPOSAL})"
    ))
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(action: &str) -> Precedent {
        Precedent {
            event_id: Uuid::now_v7(),
            action: action.into(),
            left: "a".into(),
            right: "b".into(),
            at: Utc::now(),
            why: None,
        }
    }

    fn stats(merged: i64, kept: i64, reverted: i64) -> Option<TypePairStats> {
        Some(TypePairStats {
            merged,
            kept,
            reverted,
        })
    }

    fn with_stats(merged: i64, kept: i64, reverted: i64) -> Precedents {
        Precedents {
            type_pair: stats(merged, kept, reverted),
            ..Default::default()
        }
    }

    #[test]
    fn unsure_and_low_confidence_are_proposed() {
        let none = Precedents::default();
        assert_eq!(
            gate(None, 0.99, false, NameShape::Unrelated, &none),
            Gate::Propose
        );
        assert_eq!(
            gate(Some(false), 0.6, false, NameShape::Unrelated, &none),
            Gate::Propose
        );
        assert_eq!(
            gate(Some(true), 0.84, false, NameShape::Unrelated, &none),
            Gate::Propose
        );
    }

    #[test]
    fn keeping_apart_needs_confidence_only() {
        let none = Precedents::default();
        assert_eq!(
            gate(Some(false), 0.9, false, NameShape::Unrelated, &none),
            Gate::Apply
        );
        // 类型冲突的对也能自己分开：分开不改图
        assert_eq!(
            gate(Some(false), 0.9, true, NameShape::Unrelated, &none),
            Gate::Apply
        );
    }

    #[test]
    fn the_agent_judges_and_history_moves_the_bar() {
        // 没有任何历史：agent 自己的把握够就合，不够就问
        let none = Precedents::default();
        assert_eq!(
            gate(Some(true), 0.9, false, NameShape::Unrelated, &none),
            Gate::Apply
        );
        assert_eq!(
            gate(Some(true), 0.8, false, NameShape::Unrelated, &none),
            Gate::Propose
        );
        // 历史同意：线降到 SUPPORTED_CONF
        let pair = Precedents {
            same_pair: vec![p("review.merge")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(true), 0.8, false, NameShape::Unrelated, &pair),
            Gate::Apply
        );
        assert_eq!(
            gate(Some(true), 0.7, false, NameShape::Unrelated, &pair),
            Gate::Propose
        );
        let name = Precedents {
            same_name: vec![p("merge.manual"), p("review.merge")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(true), 0.8, false, NameShape::Unrelated, &name),
            Gate::Apply
        );
        // 同名的决定有合有分：不算同意，回到 agent 自己的线
        let mixed = Precedents {
            same_name: vec![p("review.merge"), p("review.keep")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(true), 0.8, false, NameShape::Unrelated, &mixed),
            Gate::Propose
        );
        assert_eq!(
            gate(Some(true), 0.9, false, NameShape::Unrelated, &mixed),
            Gate::Apply
        );
        // 类型对的习惯
        assert_eq!(
            gate(
                Some(true),
                0.8,
                false,
                NameShape::Unrelated,
                &with_stats(6, 2, 0)
            ),
            Gate::Apply
        );
        assert_eq!(
            gate(
                Some(true),
                0.8,
                false,
                NameShape::Unrelated,
                &with_stats(3, 1, 0)
            ),
            Gate::Propose,
            "习惯要够 {TYPE_PAIR_MIN} 笔才算"
        );
        assert_eq!(
            gate(
                Some(true),
                0.8,
                false,
                NameShape::Unrelated,
                &with_stats(6, 2, 1)
            ),
            Gate::Propose,
            "撤回过的类型对不算合并的习惯"
        );
        // 分开那一边同样：人一直分开的名字，分开的线也低
        assert_eq!(
            gate(Some(false), 0.8, false, NameShape::Unrelated, &none),
            Gate::Propose
        );
        let kept = Precedents {
            same_pair: vec![p("review.keep")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(false), 0.8, false, NameShape::Unrelated, &kept),
            Gate::Apply
        );
        assert_eq!(
            gate(
                Some(false),
                0.8,
                false,
                NameShape::Unrelated,
                &with_stats(1, 8, 0)
            ),
            Gate::Apply
        );
    }

    #[test]
    fn a_pair_people_decided_skips_the_agent() {
        let none = Precedents::default();
        assert_eq!(
            settled_by_people("Apple", "Apple Inc.", &none, Some(true)),
            Some(true)
        );
        assert_eq!(
            settled_by_people("Zhang Wei", "Zhang Wei", &none, Some(false)),
            Some(false)
        );
        assert_eq!(settled_by_people("Apple", "Apple Inc.", &none, None), None);
        let merged = Precedents {
            same_pair: vec![p("review.merge"), p("merge.manual")],
            ..Default::default()
        };
        assert_eq!(
            settled_by_people("Apple", "Apple Inc.", &merged, None),
            Some(true)
        );
        assert_eq!(
            settled_by_people("Zhang Wei", "Zhang Wei", &merged, None),
            None,
            "同名的一对：名字上的先例说不了眼前这两个是谁"
        );
        let mixed = Precedents {
            same_pair: vec![p("review.merge"), p("review.keep")],
            ..Default::default()
        };
        assert_eq!(settled_by_people("Apple", "Apple Inc.", &mixed, None), None);
        let reverted = Precedents {
            same_pair: vec![p("review.merge")],
            reverts: vec![p("merge.revert")],
            ..Default::default()
        };
        assert_eq!(
            settled_by_people("Apple", "Apple Inc.", &reverted, Some(true)),
            None
        );
    }

    #[test]
    fn a_name_has_a_shape() {
        use NameShape::*;
        assert_eq!(name_shape("Google", "google"), Identical);
        assert_eq!(
            name_shape("The New York Times", "New York Times"),
            Identical
        );
        assert_eq!(name_shape("Google DeepMind", "DeepMind"), Abbreviation);
        assert_eq!(name_shape("Adam D'Angelo", "D'Angelo"), Abbreviation);
        assert_eq!(name_shape("Apple Inc.", "Apple"), Suffix);
        assert_eq!(name_shape("Thrive Capital", "Thrive"), Suffix);
        assert_eq!(name_shape("Meta Platforms", "Meta"), Suffix);
        assert_eq!(name_shape("Claude 4 Opus", "Claude"), Version);
        assert_eq!(name_shape("AlphaFold2", "AlphaFold"), Version);
        assert_eq!(name_shape("GPT-4.5", "GPT-4"), Version);
        assert_eq!(name_shape("Lyria 3", "Lyria"), Version);
        assert_eq!(
            name_shape(
                "2024 International Mathematical Olympiad",
                "International Mathematical Olympiad"
            ),
            Version
        );
        assert_eq!(name_shape("DeepMind Health", "DeepMind"), Extension);
        // **中文名字不能把它炸掉。** 下面每一对都要走进「整词判定失败、
        // 换个位置再找」那条路，而那条路从前按字节前进，落在多字节字符
        // 中间就 panic——测试断言的是「有个答案」，不是答案是什么
        for (a, b) in [
            ("繪圖處理器", "繪圖"),
            ("英伟达繪圖繪圖", "繪圖"),
            ("北京中关村科技园", "中关村"),
            ("東京都渋谷区", "渋谷"),
            ("Nvidia 繪圖處理器", "繪圖"),
        ] {
            let _ = name_shape(a, b);
            let _ = name_shape(b, a);
        }
        assert_eq!(
            name_shape("Gemini Robotics-ER", "Gemini Robotics"),
            Extension
        );
        assert_eq!(name_shape("OpenAI Ireland Ltd", "OpenAI"), Phrase);
        assert_eq!(
            name_shape("DeepMind Health data sharing", "DeepMind Health"),
            Phrase
        );
        assert_eq!(
            name_shape("reinforcement learning (RL)", "reinforcement learning"),
            Identical
        );
        assert_eq!(
            name_shape(
                "US Securities and Exchange Commission (SEC)",
                "Securities and Exchange Commission"
            ),
            Abbreviation
        );
        assert_eq!(
            name_shape(
                "United States Department of Defense (DoD)",
                "Department of Defense"
            ),
            Abbreviation
        );
        assert_eq!(
            name_shape("Amazon Web Services (AWS)", "Amazon Web Services"),
            Identical
        );
        assert_eq!(name_shape("Sam Altman's efforts", "Sam Altman"), Phrase);
        assert_eq!(
            name_shape("share sale led by Thrive Capital", "Thrive Capital"),
            Phrase
        );
        assert_eq!(
            name_shape("psychological abuse from Sam Altman", "Sam Altman"),
            Phrase
        );
        assert_eq!(
            name_shape("MuZero, AlphaStar, AlphaGeometry", "AlphaStar"),
            Phrase
        );
        assert_eq!(
            name_shape("MuZero, AlphaStar, AlphaGeometry", "MuZero"),
            Phrase
        );
        assert_eq!(name_shape("Time", "Financial Times"), Abbreviation);
        assert_eq!(
            name_shape(
                "C++ Standard Library sorting algorithms",
                "sorting algorithm"
            ),
            Phrase
        );
        assert_eq!(name_shape("Timeline", "Time"), Unrelated);
        assert_eq!(name_shape("Altimeter", "Altimeter Capital"), Suffix);
    }

    #[test]
    fn types_conflict_only_across_families() {
        assert!(!types_conflict(Some("Organization"), Some("Corporation")));
        assert!(!types_conflict(
            Some("SoftwareApplication"),
            Some("CreativeWork")
        ));
        assert!(!types_conflict(Some("Organization"), None));
        assert!(types_conflict(Some("Person"), Some("Organization")));
        assert!(!types_conflict(Some("City"), Some("Product")));
        assert!(types_conflict(Some("City"), Some("Corporation")));
        assert!(types_conflict(Some("ConferenceEvent"), Some("Person")));
    }

    #[test]
    fn a_version_or_a_phrase_never_merges_on_its_own() {
        let none = Precedents::default();
        assert_eq!(
            gate(Some(true), 0.99, false, NameShape::Version, &none),
            Gate::Propose
        );
        assert_eq!(
            gate(Some(true), 0.99, false, NameShape::Phrase, &none),
            Gate::Propose
        );
        assert_eq!(
            gate(Some(true), 0.99, false, NameShape::Extension, &none),
            Gate::Apply
        );
        assert_eq!(
            gate(Some(false), 0.9, false, NameShape::Version, &none),
            Gate::Apply
        );
    }

    #[test]
    fn hard_rules_come_first() {
        let pair = Precedents {
            same_pair: vec![p("review.merge")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(true), 0.99, true, NameShape::Unrelated, &pair),
            Gate::Propose,
            "类型冲突不合"
        );
        let reverted = Precedents {
            same_pair: vec![p("review.merge")],
            reverts: vec![p("merge.revert")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(true), 0.99, false, NameShape::Unrelated, &reverted),
            Gate::Propose
        );
        assert_eq!(
            gate(Some(false), 0.99, false, NameShape::Unrelated, &reverted),
            Gate::Propose
        );
        let kept = Precedents {
            same_pair: vec![p("review.keep")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(true), 0.99, false, NameShape::Unrelated, &kept),
            Gate::Propose,
            "人分开过这一对，模型说合，只建议"
        );
        let merged = Precedents {
            same_pair: vec![p("review.merge")],
            ..Default::default()
        };
        assert_eq!(
            gate(Some(false), 0.99, false, NameShape::Unrelated, &merged),
            Gate::Propose,
            "人合过这一对，模型说分，只建议"
        );
    }
}
