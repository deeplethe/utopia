//! 图谱仓储：本体、实体消解（P2 第一刀：同 KB 同类型同名合一）、事实账本、图查询。

use sqlx::PgPool;
use std::collections::HashSet;
use utopia_core::models::{
    ChunkFactView, EntityFact, EntityHistoryEvent, EntityType, EvidenceView, FactReviewItem,
    GraphEdge, GraphNode, RelationType,
};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 同断言已有事实的行投影：(id, valid_from, valid_to)。
type FactSpanRow = (
    Uuid,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// 内置本体模板：(key, label, color, shape)
// 低饱和粉彩色系（深色画布上柔和发光，不刺眼）；组织/产品用方形区分"机构/制品"
const DEFAULT_ENTITY_TYPES: &[(&str, &str, &str, &str)] = &[
    ("person", "Person", "#7fd0ff", "circle"),
    ("organization", "Organization", "#4cc38a", "square"),
    ("project", "Project", "#f2b66d", "circle"),
    // 问数语义层：可问的量与可切的维度（BI 语义层标准抽象），映射经 mapped_to 指向数据资产
    ("metric", "Metric", "#ffd580", "square"),
    ("dimension", "Dimension", "#9adcc6", "square"),
    ("product", "Product", "#c4a5ff", "square"),
    ("event", "Event", "#ff9daf", "circle"),
    ("concept", "Concept", "#8ea5bd", "circle"),
    ("location", "Location", "#5fd4d0", "circle"),
];

/// (key, label, temporal, functional, inverse_functional)
/// functional = 同一时刻一个主语至多一个宾语（张三同时只 reports_to 一人）；
/// inverse_functional = 同一时刻一个宾语至多一个主语（一个项目同时只有一个 leads 它的人）。
/// 两者都是时态冲突检测（自动闭合 valid_to）的触发依据。
const DEFAULT_RELATION_TYPES: &[(&str, &str, &str, bool, bool)] = &[
    ("works_at", "works at", "state", false, false),
    ("leads", "leads", "state", false, true),
    ("reports_to", "reports to", "state", true, false),
    // 多对多：一个项目既属于 Microsoft Learn 也属于 Microsoft，一个组件同时属于
    // 多个系统；即便按严格层级理解，原文也会并列陈述父级与祖先。曾误标 functional，
    // 真实语料上把这些并存关系全判成矛盾——28 篇企业新闻就积压了 59 条假冲突。
    ("part_of", "part of", "state", false, false),
    ("participates_in", "participates in", "state", false, false),
    ("located_in", "located in", "state", false, false),
    ("produces", "produces", "state", false, false),
    ("alias_of", "alias of", "eternal", false, false),
    ("related_to", "related to", "state", false, false),
    // 问数语义层：概念 → 数据资产定义（object_value 宾语：{source, table?, expr?, sql?,
    // derived?, unit?, summary}）。多源=多条并存；同源口径演变由确认流程显式闭合，
    // 不靠引擎盲判（唯一性粒度是 (概念,源)，在 object_value 内部，引擎不感知）
    ("mapped_to", "mapped to", "state", false, false),
];

pub async fn ensure_default_ontology(pool: &PgPool, kb_id: Uuid) -> AppResult<()> {
    for (key, label, color, shape) in DEFAULT_ENTITY_TYPES {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label, color, shape, builtin)
             VALUES ($1, $2, $3, $4, $5, $6, TRUE) ON CONFLICT (kb_id, key) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(key)
        .bind(label)
        .bind(color)
        .bind(shape)
        .execute(pool)
        .await?;
    }
    for (key, label, temporal, functional, inverse_functional) in DEFAULT_RELATION_TYPES {
        sqlx::query(
            "INSERT INTO relation_types
                (id, kb_id, key, label, temporal, functional, inverse_functional, builtin)
             VALUES ($1, $2, $3, $4, $5, $6, $7, TRUE) ON CONFLICT (kb_id, key) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(key)
        .bind(label)
        .bind(temporal)
        .bind(functional)
        .bind(inverse_functional)
        .execute(pool)
        .await?;
    }
    Ok(())
}

pub async fn entity_types(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<EntityType>> {
    Ok(
        sqlx::query_as("SELECT * FROM entity_types WHERE kb_id = $1 ORDER BY created_at")
            .bind(kb_id)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn relation_types(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<RelationType>> {
    Ok(
        sqlx::query_as("SELECT * FROM relation_types WHERE kb_id = $1 ORDER BY created_at")
            .bind(kb_id)
            .fetch_all(pool)
            .await?,
    )
}

/// 写入事实。返回 (事实 id, 是否新建)。
///
/// 同断言（同主谓宾）的多次观察不各立门户：
/// - 同 valid_from 的 live 行已存在 → 复用（证据累积到同一条）
/// - 新观察**没带时间**、同断言已有开放行 → 弱化陈述并入已有行（"隶属星云科技"
///   并进"2021-02 起隶属星云科技"，不再产生一条无时间的重复）
/// - 新观察**带了时间**、同断言已有的是无时无终的裸行 → 时间精化：新行落库后
///   把裸行作废并以 supersedes 链上（作废+改写，认知史完整），证据随行复制
/// - 双方都带时间但不同 → 保守并存（可能真是两段区间，如离职又回归）
///
/// 事实的宾语：实体（关系）或字面值（属性/问数映射）。同一套折并与时间精化逻辑。
#[derive(Debug, Clone, Copy)]
pub enum FactObject<'a> {
    Entity(Uuid),
    Value(&'a serde_json::Value),
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_fact(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    predicate_id: Uuid,
    object_id: Uuid,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_precision: &str,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    insert_fact_inner(
        pool,
        kb_id,
        subject_id,
        predicate_id,
        FactObject::Entity(object_id),
        valid_from,
        valid_to,
        valid_precision,
        confidence,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_fact_inner(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    predicate_id: Uuid,
    object: FactObject<'_>,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_precision: &str,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    let same_sql = match object {
        FactObject::Entity(_) => {
            "SELECT id, valid_from, valid_to FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND object_id = $4
               AND invalidated_at IS NULL"
        }
        FactObject::Value(_) => {
            "SELECT id, valid_from, valid_to FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND object_value = $4
               AND object_id IS NULL AND invalidated_at IS NULL"
        }
    };
    let mut q = sqlx::query_as(same_sql)
        .bind(kb_id)
        .bind(subject_id)
        .bind(predicate_id);
    q = match object {
        FactObject::Entity(id) => q.bind(id),
        FactObject::Value(v) => q.bind(v),
    };
    let same: Vec<FactSpanRow> = q.fetch_all(pool).await?;
    // 精确重复：同 valid_from → 复用
    if let Some((existing, _, _)) = same.iter().find(|(_, vf, _)| *vf == valid_from) {
        return Ok((*existing, false));
    }
    // 弱化陈述：新观察无时间，同断言已有开放行 → 并入（取起点最新的开放行）
    if valid_from.is_none() && valid_to.is_none() {
        if let Some((existing, _, _)) = same
            .iter()
            .filter(|(_, _, vt)| vt.is_none())
            .max_by_key(|(_, vf, _)| *vf)
        {
            return Ok((*existing, false));
        }
    }
    // 时间精化候选：已有无时无终的裸行，本次观察带了起点 → 落库后作废裸行并链上
    let refine_target = if valid_from.is_some() {
        same.iter()
            .find(|(_, vf, vt)| vf.is_none() && vt.is_none())
            .map(|(id, _, _)| *id)
    } else {
        None
    };

    let id = Uuid::now_v7();
    let insert_sql = match object {
        FactObject::Entity(_) => {
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                valid_from, valid_to, valid_precision, confidence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        }
        FactObject::Value(_) => {
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                                valid_from, valid_to, valid_precision, confidence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
        }
    };
    let mut ins = sqlx::query(insert_sql)
        .bind(id)
        .bind(kb_id)
        .bind(subject_id)
        .bind(predicate_id);
    ins = match object {
        FactObject::Entity(oid) => ins.bind(oid),
        FactObject::Value(v) => ins.bind(v),
    };
    ins.bind(valid_from)
        .bind(valid_to)
        .bind(valid_precision)
        .bind(confidence)
        .execute(pool)
        .await?;

    // 时间精化：裸行（无时无终的同断言）被本次带时间的观察取代——作废+链上，证据随行
    if let Some(old_id) = refine_target {
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(old_id)
            .execute(pool)
            .await?;
        sqlx::query("UPDATE facts SET supersedes = $2 WHERE id = $1")
            .bind(id)
            .bind(old_id)
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
             SELECT $1, chunk_id, quote, document_id, doc_version
             FROM fact_evidence WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(old_id)
        .execute(pool)
        .await?;
    }
    Ok((id, true))
}

/// 字面值宾语的事实（object_value 通道，问数映射首个消费者）。
/// 去重：同 (S,P) 且 object_value 完全相等的 live 事实只存一条。
#[allow(clippy::too_many_arguments)]
pub async fn insert_value_fact(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    predicate_id: Uuid,
    object_value: &serde_json::Value,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    valid_precision: &str,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    insert_fact_inner(
        pool,
        kb_id,
        subject_id,
        predicate_id,
        FactObject::Value(object_value),
        valid_from,
        valid_to,
        valid_precision,
        confidence,
    )
    .await
}

/// 已确认（置信 ≥ threshold）的问数映射：概念名 + 定义。问数 prompt 注入用。
pub async fn confirmed_mappings(
    pool: &PgPool,
    kb_id: Uuid,
    threshold: f32,
    limit: i64,
) -> AppResult<Vec<(String, serde_json::Value)>> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT s.canonical_name, f.object_value
         FROM facts f
         JOIN relation_types r ON r.id = f.predicate_id AND r.key = 'mapped_to'
         JOIN entities s ON s.id = f.subject_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.valid_to IS NULL
           AND f.object_value IS NOT NULL AND f.confidence >= $2
         ORDER BY s.canonical_name
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(threshold)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_evidence(
    pool: &PgPool,
    fact_id: Uuid,
    chunk_id: Uuid,
    quote: Option<&str>,
) -> AppResult<()> {
    // 证据落笔即记版本：出自哪份文档的第几版（S3 版本对账与"证据过期"判定的依据）
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         SELECT $1, $2, $3, c.document_id, c.doc_version FROM chunks c WHERE c.id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(fact_id)
    .bind(chunk_id)
    .bind(quote)
    .execute(pool)
    .await?;
    Ok(())
}

const NODE_SQL: &str = "SELECT e.id, e.canonical_name AS name, t.key AS type_key,
        t.label AS type_label, t.color, t.shape, e.disambiguator,
        (SELECT count(*) FROM facts f
         WHERE (f.subject_id = e.id OR f.object_id = e.id) AND f.invalidated_at IS NULL) AS degree
     FROM entities e JOIN entity_types t ON t.id = e.type_id";

/// 全图概览：按度数取 top N 实体及其间的边。
/// `at`：服务端 as-of——只返回 T 时刻有效的边（起点不晚于 T 或未知，终点晚于 T 或开放）。
/// 前端时间滑杆走本地过滤不传此参数；这是给 API/MCP 消费者的时间旅行入口。
pub async fn overview(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    at: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{NODE_SQL} WHERE e.kb_id = $1 AND e.merged_into IS NULL ORDER BY degree DESC, e.created_at LIMIT $2"
    ))
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
    let edges = edges_among(pool, kb_id, &ids, at).await?;
    Ok((nodes, edges))
}

async fn edges_among(
    pool: &PgPool,
    kb_id: Uuid,
    ids: &[Uuid],
    at: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<GraphEdge>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let edges: Vec<GraphEdge> = sqlx::query_as(
        "SELECT f.id, f.subject_id AS source, f.object_id AS target, r.key AS predicate,
                r.label, f.valid_from, f.valid_to, f.confidence
         FROM facts f JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.object_id IS NOT NULL
           AND f.subject_id = ANY($2) AND f.object_id = ANY($2)
           AND ($3::timestamptz IS NULL
                OR ((f.valid_from IS NULL OR f.valid_from <= $3)
                    AND (f.valid_to IS NULL OR f.valid_to > $3)))",
    )
    .bind(kb_id)
    .bind(ids)
    .bind(at)
    .fetch_all(pool)
    .await?;
    Ok(edges)
}

/// 邻域扩展（BFS，最多 2 跳，节点数封顶）。
pub async fn neighborhood(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    hops: u8,
    at: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
    const MAX_NODES: usize = 300;
    let mut seen: HashSet<Uuid> = HashSet::from([entity_id]);
    let mut frontier: Vec<Uuid> = vec![entity_id];

    for _ in 0..hops.clamp(1, 2) {
        if frontier.is_empty() || seen.len() >= MAX_NODES {
            break;
        }
        let touching: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(
            "SELECT subject_id, object_id FROM facts
             WHERE kb_id = $1 AND invalidated_at IS NULL AND object_id IS NOT NULL
               AND (subject_id = ANY($2) OR object_id = ANY($2))",
        )
        .bind(kb_id)
        .bind(&frontier)
        .fetch_all(pool)
        .await?;

        let mut next = Vec::new();
        for (s, o) in touching {
            for id in [Some(s), o].into_iter().flatten() {
                if seen.len() >= MAX_NODES {
                    break;
                }
                if seen.insert(id) {
                    next.push(id);
                }
            }
        }
        frontier = next;
    }

    let ids: Vec<Uuid> = seen.into_iter().collect();
    let nodes: Vec<GraphNode> =
        sqlx::query_as(&format!("{NODE_SQL} WHERE e.kb_id = $1 AND e.id = ANY($2)"))
            .bind(kb_id)
            .bind(&ids)
            .fetch_all(pool)
            .await?;
    let edges = edges_among(pool, kb_id, &ids, at).await?;
    Ok((nodes, edges))
}

pub async fn search_entities(
    pool: &PgPool,
    kb_id: Uuid,
    q: &str,
    limit: i64,
) -> AppResult<Vec<GraphNode>> {
    let pattern = format!("%{}%", q.trim());
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{NODE_SQL} WHERE e.kb_id = $1 AND e.merged_into IS NULL
         AND e.canonical_name ILIKE $2 ORDER BY degree DESC LIMIT $3"
    ))
    .bind(kb_id)
    .bind(pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(nodes)
}

/// 实体详情：节点信息 + 事实时间线。
pub async fn entity_detail(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
) -> AppResult<(GraphNode, Vec<EntityFact>)> {
    let node: GraphNode = sqlx::query_as(&format!("{NODE_SQL} WHERE e.kb_id = $1 AND e.id = $2"))
        .bind(kb_id)
        .bind(entity_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let facts: Vec<EntityFact> = sqlx::query_as(
        "SELECT f.id,
                CASE WHEN f.subject_id = $2 THEN 'out' ELSE 'in' END AS direction,
                r.key AS predicate_key, r.label AS predicate_label, r.temporal,
                CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END AS other_id,
                o.canonical_name AS other_name, f.object_value,
                f.valid_from, f.valid_to, f.valid_precision, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
                 AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                                 JOIN chunks c ON c.id = fe.chunk_id
                                 WHERE fe.fact_id = f.id AND c.superseded_at IS NULL)
                ) AS stale,
                (f.supersedes IS NOT NULL) AS corrected,
                (SELECT MAX(COALESCE(d.doc_time, d.created_at))
                 FROM fact_evidence fe JOIN documents d ON d.id = fe.document_id
                 WHERE fe.fact_id = f.id) AS last_evidence_time
         FROM facts f
         JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o
           ON o.id = CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
           AND (f.subject_id = $2 OR f.object_id = $2)
         ORDER BY f.valid_from NULLS LAST, f.recorded_at",
    )
    .bind(kb_id)
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    Ok((node, facts))
}

/// 人工修正实体的类型或名字。返回 (改前快照, 改后状态)——调用方据此记审计台账。
///
/// 类型判错、名字抽歪，此前只能整库重抽这把大锤。抽取给的是初判，不是定论。
///
/// 同名不拦：同类同名的两个实体是"宁分勿合"的正当产物（两个张伟），
/// 拦下来就录不进第二个。碰撞由调用方查出后提示合并，见 `same_name_peers`。
pub async fn update_entity(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    type_id: Option<Uuid>,
    canonical_name: Option<&str>,
) -> AppResult<(GraphNode, GraphNode)> {
    let before: GraphNode = sqlx::query_as(&format!(
        "{NODE_SQL} WHERE e.kb_id = $1 AND e.id = $2 AND e.merged_into IS NULL"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let new_name = match canonical_name {
        Some(raw) => {
            let n = raw.trim();
            if n.is_empty() {
                return Err(AppError::Validation("Name cannot be empty".into()));
            }
            // 与抽取侧同一上限：越过这条线的多半是整句被当成了名字
            if n.chars().count() > 100 {
                return Err(AppError::Validation("Name is too long (max 100)".into()));
            }
            Some(n)
        }
        None => None,
    };

    if let Some(t) = type_id {
        let exists: Option<(Uuid,)> =
            sqlx::query_as("SELECT id FROM entity_types WHERE id = $1 AND kb_id = $2")
                .bind(t)
                .bind(kb_id)
                .fetch_optional(pool)
                .await?;
        if exists.is_none() {
            return Err(AppError::Validation("No such entity type in this KB".into()));
        }
    }

    sqlx::query(
        "UPDATE entities
         SET type_id = COALESCE($3, type_id),
             canonical_name = COALESCE($4, canonical_name),
             updated_at = now()
         WHERE id = $1 AND kb_id = $2 AND merged_into IS NULL",
    )
    .bind(entity_id)
    .bind(kb_id)
    .bind(type_id)
    .bind(new_name)
    .execute(pool)
    .await?;

    // 消歧后缀依赖名字分组与类型标签（类型标签是它的兜底值），两者都刚被改过。
    // 改名要刷两组：旧名那组可能掉到 1 个（后缀该清掉），新名那组可能涨到 2 个。
    if let Some(n) = new_name.filter(|n| !n.eq_ignore_ascii_case(&before.name)) {
        crate::resolution::refresh_disambiguators(pool, kb_id, &before.name).await?;
        crate::resolution::refresh_disambiguators(pool, kb_id, n).await?;
    } else if type_id.is_some() {
        crate::resolution::refresh_disambiguators(pool, kb_id, &before.name).await?;
    }

    let after: GraphNode = sqlx::query_as(&format!("{NODE_SQL} WHERE e.kb_id = $1 AND e.id = $2"))
        .bind(kb_id)
        .bind(entity_id)
        .fetch_one(pool)
        .await?;
    Ok((before, after))
}

/// 与给定实体同名（不区分大小写）的其他存活实体——用于改名后提示"是否合并"。
/// 只报告，不阻断：判定它们是否真是同一个，是人的事。
pub async fn same_name_peers(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
) -> AppResult<Vec<GraphNode>> {
    sqlx::query_as(&format!(
        "{NODE_SQL} WHERE e.kb_id = $1 AND e.merged_into IS NULL AND e.id <> $2
           AND lower(e.canonical_name) = (SELECT lower(canonical_name) FROM entities WHERE id = $2)
         ORDER BY degree DESC LIMIT 10"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// 低置信 live 事实（审核页）。
pub async fn low_confidence_facts(
    pool: &PgPool,
    kb_id: Uuid,
    below: f32,
    limit: i64,
) -> AppResult<Vec<FactReviewItem>> {
    let rows: Vec<FactReviewItem> = sqlx::query_as(
        "SELECT f.id, s.canonical_name AS subject_name, r.label AS predicate_label,
                COALESCE(o.canonical_name, f.object_value->>'summary') AS object_name,
                f.valid_from, f.valid_to, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (SELECT fe.quote FROM fact_evidence fe
                 WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.confidence < $2
         ORDER BY f.confidence, f.recorded_at DESC
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(below)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// "证据全部停留在旧版"的现行事实（S3 第三刀：文档新版没再确认的知识）。
/// 判定纯派生自 chunk 存活性——认领机制保证未变段落的证据不被误伤；
/// 绝不自动删除（没再提 ≠ 不成立），删除/闭合权在 Review 的人手里。
pub async fn stale_facts(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<FactReviewItem>> {
    let rows: Vec<FactReviewItem> = sqlx::query_as(
        "SELECT f.id, s.canonical_name AS subject_name, r.label AS predicate_label,
                COALESCE(o.canonical_name, f.object_value->>'summary') AS object_name,
                f.valid_from, f.valid_to, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (SELECT fe.quote FROM fact_evidence fe
                 WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
           AND EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
           AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                           JOIN chunks c ON c.id = fe.chunk_id
                           WHERE fe.fact_id = f.id AND c.superseded_at IS NULL)
         ORDER BY f.recorded_at DESC
         LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 人工确认低置信事实：置信度提到 1.0。
pub async fn confirm_fact(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE facts SET confidence = 1.0 WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    // 问数映射的口径唯一性：确认 (概念, 源) 的新定义 = 背书替换，同源旧映射作废。
    // 认知轴修正（旧口径是"我们当时的理解"，不是世界里结束的状态）；
    // 唯一性只活在这条确认流里，时态引擎保持领域无关。
    sqlx::query(
        "UPDATE facts f SET invalidated_at = now()
         FROM facts nf
         JOIN relation_types r ON r.id = nf.predicate_id
         WHERE nf.id = $1 AND r.key = 'mapped_to'
           AND f.kb_id = $2 AND f.id <> nf.id
           AND f.subject_id = nf.subject_id AND f.predicate_id = nf.predicate_id
           AND f.invalidated_at IS NULL
           AND f.object_value->>'source' = nf.object_value->>'source'",
    )
    .bind(fact_id)
    .bind(kb_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 人工否决事实：作废（账本 append-only，不 DELETE）。
pub async fn reject_fact(pool: &PgPool, kb_id: Uuid, fact_id: Uuid) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE facts SET invalidated_at = now()
         WHERE id = $1 AND kb_id = $2 AND invalidated_at IS NULL",
    )
    .bind(fact_id)
    .bind(kb_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 反向证据链：该文档每个分块抽出了哪些 live 事实（文档查看器右栏）。
pub async fn document_extractions(
    pool: &PgPool,
    document_id: Uuid,
) -> AppResult<Vec<ChunkFactView>> {
    let rows: Vec<ChunkFactView> = sqlx::query_as(
        "SELECT fe.chunk_id, f.id AS fact_id,
                f.subject_id, s.canonical_name AS subject,
                r.label AS predicate,
                f.object_id, o.canonical_name AS object,
                f.valid_from, f.valid_to, f.confidence
         FROM fact_evidence fe
         JOIN chunks c ON c.id = fe.chunk_id AND c.document_id = $1
              AND c.superseded_at IS NULL
         JOIN facts f ON f.id = fe.fact_id AND f.invalidated_at IS NULL
         JOIN entities s ON s.id = f.subject_id
         JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         ORDER BY c.seq, f.recorded_at",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 证据回放路径：不过滤 superseded——它的职责就是能看旧版。
/// stale = 证据版本落后于文档当前版本（UI 标 "from v{n}"）。
pub async fn fact_evidence(pool: &PgPool, fact_id: Uuid) -> AppResult<Vec<EvidenceView>> {
    let rows: Vec<EvidenceView> = sqlx::query_as(
        "SELECT fe.quote, fe.chunk_id, c.document_id, d.filename, c.seq,
                c.doc_version,
                c.doc_version < COALESCE(
                    (SELECT MAX(version) FROM document_versions dv
                     WHERE dv.document_id = c.document_id), 1) AS stale
         FROM fact_evidence fe
         JOIN chunks c ON c.id = fe.chunk_id
         JOIN documents d ON d.id = c.document_id
         WHERE fe.fact_id = $1",
    )
    .bind(fact_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 清空 KB 的整个图层（Rebuild graph 的清算语义）：实体/事实/证据/待审/冲突/合并
/// 记录全删，本体（类与关系定义）与文档/分块/嵌入保留。
///
/// 刻意保留两样：决策台账（audit_events，快照自包含，图没了记录仍可读）与裁决
/// 缓存（resolution_verdicts，重建后同名对重现直接命中，省一批 LLM 调用）。
/// 返回 (删除实体数, 删除事实数)。
pub async fn purge_graph(pool: &PgPool, kb_id: Uuid) -> AppResult<(i64, i64)> {
    let mut tx = pool.begin().await?;
    let (entity_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM entities WHERE kb_id = $1")
        .bind(kb_id)
        .fetch_one(&mut *tx)
        .await?;
    let (fact_count,): (i64,) = sqlx::query_as("SELECT count(*) FROM facts WHERE kb_id = $1")
        .bind(kb_id)
        .fetch_one(&mut *tx)
        .await?;

    // FK 多为 CASCADE，但两处自引用是 NO ACTION：先解引用再删，顺序显式写出
    // （这段本身就是"图层由什么构成"的定义）
    for sql in [
        "DELETE FROM fact_conflicts WHERE kb_id = $1",
        "DELETE FROM resolution_reviews WHERE kb_id = $1",
        "DELETE FROM entity_merges WHERE kb_id = $1",
        "UPDATE facts SET supersedes = NULL WHERE kb_id = $1",
        "DELETE FROM fact_evidence WHERE fact_id IN (SELECT id FROM facts WHERE kb_id = $1)",
        "DELETE FROM facts WHERE kb_id = $1",
        "UPDATE entities SET merged_into = NULL WHERE kb_id = $1",
        "DELETE FROM entities WHERE kb_id = $1",
        // 未匹配统计由抽取重新累积
        "DELETE FROM ontology_misses WHERE kb_id = $1",
    ] {
        sqlx::query(sql).bind(kb_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok((entity_count, fact_count))
}

/// 实体的认知变更历史（记录时间轴）。
///
/// 与 entity_detail 的根本差别：那里 `invalidated_at IS NULL`，只答"现在认为是什么"；
/// 这里不过滤，答"我们何时这么认为、又何时改了主意"。数据一直都在——账本
/// append-only，修正是插新行 + 标旧行作废，从不覆盖。
///
/// 一行事实最多产出两个事件：写入（asserted / corrected）与作废（rejected）。
/// 有后继修正行的作废不单独记——那次死亡已由后继那条 corrected 解释。
///
/// 归因：审计台账里 fact.close 的 target 是**被闭合的旧行**，而修正行是新插的另一行，
/// 所以按 COALESCE(supersedes, id) 回查；冲突裁决的 target 是 conflict 行，再绕一跳。
/// 查不到审计记录 = 引擎自动（抽取写入或时态对账），actor 为 NULL。
pub async fn entity_history(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<EntityHistoryEvent>, i64)> {
    const EVENTS: &str = "
        WITH ef AS (
            SELECT f.*,
                   CASE WHEN f.subject_id = $2 THEN 'out' ELSE 'in' END AS direction,
                   CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END AS other_id
            FROM facts f
            WHERE f.kb_id = $1 AND (f.subject_id = $2 OR f.object_id = $2)
        ),
        ev AS (
            SELECT ef.*, ef.recorded_at AS at,
                   CASE WHEN ef.supersedes IS NULL THEN 'asserted' ELSE 'corrected' END AS kind
            FROM ef
            UNION ALL
            SELECT ef.*, ef.invalidated_at AS at, 'rejected' AS kind
            FROM ef
            WHERE ef.invalidated_at IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM facts s WHERE s.supersedes = ef.id)
        )";
    let rows: Vec<EntityHistoryEvent> = sqlx::query_as(&format!(
        "{EVENTS}
         SELECT ev.id AS fact_id, ev.at, ev.kind, ev.direction,
                r.label AS predicate_label, o.canonical_name AS other_name,
                ev.object_value, ev.valid_from, ev.valid_to, ev.valid_precision,
                ev.confidence, act.actor_name, act.action,
                src.document_id, src.filename, src.quote
         FROM ev
         JOIN relation_types r ON r.id = ev.predicate_id
         LEFT JOIN entities o ON o.id = ev.other_id
         LEFT JOIN LATERAL (
             SELECT u.display_name AS actor_name, a.action
             FROM audit_events a
             LEFT JOIN users u ON u.id = a.actor_id
             WHERE a.kb_id = $1
               -- 断言由抽取写入，从来不是人的决定：归因只问修正与推翻这两类事件，
               -- 否则后发生的人工裁决会被错安到当初那条断言头上
               AND ev.kind <> 'asserted'
               AND a.action = ANY(CASE ev.kind
                     WHEN 'corrected' THEN ARRAY['fact.close', 'conflict.close_old']
                     ELSE ARRAY['fact.reject', 'conflict.reject_new'] END)
               AND (a.target_id = COALESCE(ev.supersedes, ev.id)
                    OR a.target_id IN (SELECT c.id FROM fact_conflicts c
                                       WHERE c.old_fact_id = COALESCE(ev.supersedes, ev.id)
                                          OR c.new_fact_id = ev.id))
             ORDER BY a.created_at DESC LIMIT 1
         ) act ON true
         LEFT JOIN LATERAL (
             SELECT d.id AS document_id, d.filename, fe.quote
             FROM fact_evidence fe
             JOIN chunks c ON c.id = fe.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE fe.fact_id = ev.id
             ORDER BY fe.doc_version DESC NULLS LAST LIMIT 1
         ) src ON true
         ORDER BY ev.at DESC, ev.id
         LIMIT $3 OFFSET $4"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) = sqlx::query_as(&format!("{EVENTS} SELECT count(*) FROM ev"))
        .bind(kb_id)
        .bind(entity_id)
        .fetch_one(pool)
        .await?;
    Ok((rows, total))
}
