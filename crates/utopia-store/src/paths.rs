//! 两点之间的路径（#558）：`paths_between(a, b)`——A 和 B 是怎么连上的。
//!
//! 「OpenAI 和 Anthropic 的关系」是一道图谱题，而图谱工具从前只有按名找实体和
//! 倒出一个实体的全部事实。模型要答它，得把 OpenAI 的 555 条事实和 Anthropic 的
//! 全倒进上下文自己连线；实测十六次里三次连上，三次查了六七轮后宣布「库里没有」。
//! 库里有，两跳：OpenAI —employee→ Dario Amodei —founder→ Anthropic。
//!
//! **回的是链，不是节点集。** 一跳邻域展开已经有了（`graph::neighborhood`，给画布用），
//! 拿来答关系题等于把倒事实换成倒邻居。这里每条结果都是从 a 到 b 的一条边序列，
//! 短的在前，途经节点越冷门越靠前。
//!
//! **两根时间轴都要过。** 带 `at` 时，路径上**每一条边**都得在那一刻成立——2019 年
//! 的 OpenAI 和 Anthropic 之间还没有 Anthropic；带 `as_of` 时按当时的记录走，包括
//! 当时还没合并的实体归谁（`record_axis::owner_at`）。这是普通图谱路径查询没有的
//! 一半，也是这个库该露出来的那一半。
//!
//! **搜索是两头对中间。** 三跳以内的路径 a–x–y–b，只要知道 a 的两跳邻接和 b 的一跳
//! 邻居就能拼出来，不必把 a 的第三层也铺开——第三层是 OpenAI 这种点的几万条边。
//! 枢纽（度数超过 `hub_degree` 的中间节点）不往下展：经过 Sam Altman 的路径两点之间
//! 到处都是，说明不了什么；两端本身不受此限，起点是谁就从谁走。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use utopia_core::AppResult;
use uuid::Uuid;

use crate::{record_axis, world_axis};

/// 搜索的边界。测试把 `hub_degree` 调小来验证枢纽不被穿越；产品用默认值。
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 最多几跳（1..=3；再长的路径两点之间到处都是，说明不了关系）
    pub max_hops: usize,
    /// 最多回几条
    pub max_paths: usize,
    /// 途经节点的度数上限：超过它的不往下展（两端不受此限）
    pub hub_degree: i64,
    /// 第二层展开最多读多少条边——按度数从小到大取途经节点，读满即止
    pub max_edges: i64,
    /// 排序之前最多攒多少条候选：同一对节点之间多条边会让组合数爆开
    pub max_candidates: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_hops: 3,
            max_paths: 10,
            hub_degree: 150,
            max_edges: 20_000,
            max_candidates: 400,
        }
    }
}

/// 路径上的一条边，带两端的名字和它自己的区间；方向由调用方按走向判断
/// （`subject_id` 等于上一个节点就是顺着走的）
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PathEdge {
    pub fact_id: Uuid,
    pub subject_id: Uuid,
    pub subject_name: String,
    pub object_id: Uuid,
    pub object_name: String,
    pub predicate: Option<String>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_from_precision: Option<String>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_to_precision: Option<String>,
    pub holds_from: Option<DateTime<Utc>>,
    pub holds_to: Option<DateTime<Utc>>,
    pub confidence: f32,
}

#[derive(Debug, Clone)]
pub struct Path {
    /// 从 a 到 b 的节点序列，含两端
    pub nodes: Vec<Uuid>,
    /// `nodes[i]` 与 `nodes[i+1]` 之间的边
    pub edges: Vec<PathEdge>,
    /// 途经节点度数的 ln(1+d) 之和，越小越具体；两端不计
    pub specificity: f64,
}

impl Path {
    pub fn hops(&self) -> usize {
        self.edges.len()
    }
}

/// 一条边碰到的两个实体（已经按 `as_of` 折算成当时的归属）
#[derive(Debug, Clone, Copy, sqlx::FromRow)]
struct Touch {
    id: Uuid,
    subject_id: Uuid,
    object_id: Uuid,
}

/// 候选：事实 id 序列与节点序列，排序前先攒起来
struct Candidate {
    nodes: Vec<Uuid>,
    facts: Vec<Uuid>,
}

pub async fn paths_between(
    pool: &PgPool,
    kb_id: Uuid,
    a: Uuid,
    b: Uuid,
    at: Option<DateTime<Utc>>,
    as_of: Option<DateTime<Utc>>,
    limits: Limits,
) -> AppResult<Vec<Path>> {
    let max_hops = limits.max_hops.clamp(1, 3);
    if a == b {
        return Ok(Vec::new());
    }

    // a 的一跳：邻居 → 连到它的边
    let mut a_edges: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for t in touching(pool, kb_id, &[a], at, as_of).await? {
        let other = if t.subject_id == a {
            t.object_id
        } else {
            t.subject_id
        };
        a_edges.entry(other).or_default().push(t.id);
    }

    // b 的一跳（两跳以上才用得上）
    let mut b_edges: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    if max_hops >= 2 {
        for t in touching(pool, kb_id, &[b], at, as_of).await? {
            let other = if t.subject_id == b {
                t.object_id
            } else {
                t.subject_id
            };
            b_edges.entry(other).or_default().push(t.id);
        }
    }

    // a 的第二层：只展非枢纽的一跳邻居，按度数从小到大读到 `max_edges` 为止
    let mut adj2: HashMap<Uuid, Vec<(Uuid, Uuid)>> = HashMap::new();
    if max_hops >= 3 {
        let mids: Vec<Uuid> = a_edges
            .keys()
            .copied()
            .filter(|x| *x != a && *x != b)
            .collect();
        let degree = degrees(pool, kb_id, &mids, at, as_of).await?;
        let mut by_degree: Vec<(Uuid, i64)> = mids
            .iter()
            .map(|x| (*x, degree.get(x).copied().unwrap_or(0)))
            .filter(|(_, d)| *d <= limits.hub_degree)
            .collect();
        by_degree.sort_by_key(|(_, d)| *d);
        let mut budget = limits.max_edges;
        let mut expand = Vec::new();
        for (x, d) in by_degree {
            if budget < d {
                break;
            }
            budget -= d;
            expand.push(x);
        }
        if !expand.is_empty() {
            let expand_set: HashSet<Uuid> = expand.iter().copied().collect();
            for t in touching(pool, kb_id, &expand, at, as_of).await? {
                for (x, y) in [(t.subject_id, t.object_id), (t.object_id, t.subject_id)] {
                    if expand_set.contains(&x) {
                        adj2.entry(x).or_default().push((y, t.id));
                    }
                }
            }
        }
    }

    // 拼候选：一跳直连，两跳交集，三跳经第二层碰到 b 的邻居
    let mut candidates: Vec<Candidate> = Vec::new();
    let cap = limits.max_candidates.max(1);
    if let Some(direct) = a_edges.get(&b) {
        for f in direct {
            candidates.push(Candidate {
                nodes: vec![a, b],
                facts: vec![*f],
            });
        }
    }
    if max_hops >= 2 {
        let mut mids: Vec<&Uuid> = a_edges
            .keys()
            .filter(|x| **x != a && **x != b && b_edges.contains_key(*x))
            .collect();
        mids.sort();
        'two: for x in mids {
            for fa in &a_edges[x] {
                for fb in &b_edges[x] {
                    if candidates.len() >= cap {
                        break 'two;
                    }
                    candidates.push(Candidate {
                        nodes: vec![a, *x, b],
                        facts: vec![*fa, *fb],
                    });
                }
            }
        }
    }
    if max_hops >= 3 {
        let mut xs: Vec<&Uuid> = adj2.keys().collect();
        xs.sort();
        'three: for x in xs {
            for (y, fxy) in &adj2[x] {
                if *y == a || *y == b || y == x {
                    continue;
                }
                let Some(fbs) = b_edges.get(y) else {
                    continue;
                };
                for fa in &a_edges[x] {
                    for fb in fbs {
                        if candidates.len() >= cap {
                            break 'three;
                        }
                        candidates.push(Candidate {
                            nodes: vec![a, *x, *y, b],
                            facts: vec![*fa, *fxy, *fb],
                        });
                    }
                }
            }
        }
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    // 排序：短的在前；同长的按途经节点的冷门程度；再按置信度
    let mut mids: Vec<Uuid> = candidates
        .iter()
        .flat_map(|c| c.nodes[1..c.nodes.len() - 1].iter().copied())
        .collect();
    mids.sort();
    mids.dedup();
    let degree = degrees(pool, kb_id, &mids, at, as_of).await?;
    let specificity = |c: &Candidate| -> f64 {
        c.nodes[1..c.nodes.len() - 1]
            .iter()
            .map(|n| (1.0 + degree.get(n).copied().unwrap_or(0) as f64).ln())
            .sum()
    };
    let mut all_facts: Vec<Uuid> = candidates.iter().flat_map(|c| c.facts.clone()).collect();
    all_facts.sort();
    all_facts.dedup();
    let detail = details(pool, kb_id, &all_facts, as_of).await?;
    let confidence = |c: &Candidate| -> f64 {
        let n = c.facts.len().max(1) as f64;
        c.facts
            .iter()
            .map(|f| detail.get(f).map(|e| e.confidence as f64).unwrap_or(0.0))
            .sum::<f64>()
            / n
    };
    let mut scored: Vec<(usize, f64, f64, Candidate)> = candidates
        .into_iter()
        .map(|c| (c.facts.len(), specificity(&c), confidence(&c), c))
        .collect();
    scored.sort_by(|x, y| {
        x.0.cmp(&y.0)
            .then(x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))
            .then(y.2.partial_cmp(&x.2).unwrap_or(std::cmp::Ordering::Equal))
    });

    // 同一串节点、同一串有向谓词只回一条：同向的重复观察折叠，但 A → B 和
    // A ← B 不是同一条关系，不能因遍历时两端相同而丢掉一个方向。
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (_, spec, _, c) in scored {
        if out.len() >= limits.max_paths {
            break;
        }
        let edges: Option<Vec<PathEdge>> = c.facts.iter().map(|f| detail.get(f).cloned()).collect();
        // 细节查不到的边（as_of 时刻两端之一不可见）整条路径不要：链断了一环就不是链
        let Some(edges) = edges else {
            continue;
        };
        let directed_predicates: Vec<_> = edges
            .iter()
            .zip(&c.nodes)
            .map(|(e, node)| {
                (
                    e.predicate.clone().unwrap_or_default(),
                    e.subject_id == *node,
                )
            })
            .collect();
        if !seen.insert((c.nodes.clone(), directed_predicates)) {
            continue;
        }
        out.push(Path {
            nodes: c.nodes,
            edges,
            specificity: spec,
        });
    }
    Ok(out)
}

/// 碰到这批实体的边（两个方向），两根时间轴都过，自环不要
async fn touching(
    pool: &PgPool,
    kb_id: Uuid,
    ids: &[Uuid],
    at: Option<DateTime<Utc>>,
    as_of: Option<DateTime<Utc>>,
) -> AppResult<Vec<Touch>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let subject = record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false);
    let object = record_axis::owner_at("f", "object_id", as_of.map(|_| 3), true);
    let rows: Vec<Touch> = sqlx::query_as(&format!(
        "SELECT f.id, {subject} AS subject_id, {object} AS object_id
           FROM facts f
          WHERE f.kb_id = $1 AND f.object_id IS NOT NULL
            AND ({subject} = ANY($2) OR {object} = ANY($2))
            AND {subject} <> {object}
            AND {held} AND {hold}",
        held = record_axis::facts_held_at("f", as_of.map(|_| 3)),
        hold = world_axis::facts_hold_at("f", 4),
    ))
    .bind(kb_id)
    .bind(ids)
    .bind(as_of)
    .bind(at)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 这批实体各自的度数，同一套时间轴过滤下
async fn degrees(
    pool: &PgPool,
    kb_id: Uuid,
    ids: &[Uuid],
    at: Option<DateTime<Utc>>,
    as_of: Option<DateTime<Utc>>,
) -> AppResult<HashMap<Uuid, i64>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let subject = record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false);
    let object = record_axis::owner_at("f", "object_id", as_of.map(|_| 3), true);
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(&format!(
        "SELECT n.id, count(f.id)
           FROM unnest($2::uuid[]) AS n(id)
           JOIN facts f ON f.kb_id = $1 AND f.object_id IS NOT NULL
                       AND ({subject} = n.id OR {object} = n.id)
                       AND {held} AND {hold}
          GROUP BY n.id",
        held = record_axis::facts_held_at("f", as_of.map(|_| 3)),
        hold = world_axis::facts_hold_at("f", 4),
    ))
    .bind(kb_id)
    .bind(ids)
    .bind(as_of)
    .bind(at)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// 这批事实的两端名字、谓词与区间
async fn details(
    pool: &PgPool,
    kb_id: Uuid,
    fact_ids: &[Uuid],
    as_of: Option<DateTime<Utc>>,
) -> AppResult<HashMap<Uuid, PathEdge>> {
    if fact_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let subject = record_axis::owner_at("f", "subject_id", as_of.map(|_| 3), false);
    let object = record_axis::owner_at("f", "object_id", as_of.map(|_| 3), true);
    let rows: Vec<PathEdge> = sqlx::query_as(&format!(
        "SELECT f.id AS fact_id,
                {subject} AS subject_id, s.canonical_name AS subject_name,
                {object} AS object_id, o.canonical_name AS object_name,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate,
                f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                {holds_from} AS holds_from, {holds_to} AS holds_to, f.confidence
           FROM facts f
           LEFT JOIN relation_types r ON r.id = f.predicate_id
           JOIN entities s ON s.id = {subject}
           JOIN entities o ON o.id = {object}
          WHERE f.kb_id = $1 AND f.id = ANY($2)",
        holds_from = world_axis::facts_holds_from("f"),
        holds_to = world_axis::facts_holds_to("f"),
    ))
    .bind(kb_id)
    .bind(fact_ids)
    .bind(as_of)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|e| (e.fact_id, e)).collect())
}
