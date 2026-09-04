//! 图谱仓储：本体、实体消解（P2 第一刀：同 KB 同类型同名合一）、事实账本、图查询。

use sqlx::PgPool;
use std::collections::HashSet;
use utopia_core::models::{
    ChunkFactView, EntityFact, EntityHistoryEvent, EntityType, EvidenceView, FactReviewItem,
    GraphChange, GraphEdge, GraphNode, ProposedPredicate, RelationType,
};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 同断言已有事实的行投影：(id, valid_from, valid_to)。
type FactSpanRow = (
    Uuid,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// 采纳时旧事实的去向（`fact_adoptions.mode`）：新写一行取代它。
const ADOPT_SUPERSEDED: &str = "superseded";
/// 目标断言已存在 → 并进去。旧行被作废且没有后继，实体历史必须据此把它
/// 读成"并入"而不是"撤回"，否则界面会宣称一件没发生的事。
const ADOPT_MERGED: &str = "merged";

// 建库不再播种任何关系，也不再有 `ensure_default_ontology`。
//
// 这里曾经有十条种子关系、一张中文措辞表、一个按语言取措辞的 `localized`，
// 以及一个在建库 / 首次读本体 / 每次抽取前都会跑一遍的播种函数。它们分三次退场：
//
// - `related_to`（0010）：代码层面的兜底，摆进提示词就成了逃生舱
// - 另外八条（`#125`）：零个带签名、装了本体包也不会被同名顶替
//   （`worksFor` 与 `works_at` 的 key 对不上，于是并存成两条边）、
//   而且公理位恒为 false，一致性检查在它们上面永远查不出矛盾
// - `mapped_to`（0011）：它是「这个数怎么算」不是「世界上有什么」，
//   已搬去 `concept_mappings`
//
// 剩下的那个函数于是只是在遍历一张空表。**本体从建库第一天起就只有
// 用户自己导入的词表**——与 0009 删掉内置实体类是同一件事的下半段。

pub async fn entity_types(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<EntityType>> {
    Ok(
        // 又一次 SELECT *：parents 在关联表里，`*` 取不到。
        // 这是同一个陷阱的第三次——SQL 在字符串里，cargo check 全绿，
        // 第一个请求才报 no column found
        sqlx::query_as(
            "SELECT t.*,
                    ARRAY(SELECT p.parent_id FROM entity_type_parents p
                          WHERE p.child_id = t.id) AS parents,
                    (SELECT p.parent_id FROM entity_type_parents p
                      WHERE p.child_id = t.id AND p.is_primary) AS primary_parent
             FROM entity_types t WHERE t.kb_id = $1 ORDER BY t.created_at",
        )
        .bind(kb_id)
        .fetch_all(pool)
        .await?,
    )
}

pub async fn relation_types(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<RelationType>> {
    // 不用 SELECT *：domain/range 在关联表里，`*` 取不到，
    // 而且 sqlx 要到运行时才会说 "no column found" —— 编译器看不见 SQL 字符串
    Ok(sqlx::query_as(
        "SELECT r.*,
                ARRAY(SELECT d.entity_type_id FROM relation_type_domains d
                      WHERE d.relation_type_id = r.id) AS domains,
                ARRAY(SELECT g.entity_type_id FROM relation_type_ranges g
                      WHERE g.relation_type_id = r.id) AS ranges
         FROM relation_types r WHERE r.kb_id = $1 ORDER BY r.created_at",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
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
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object_id: Uuid,
    validity: Validity<'_>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    insert_fact_inner(
        pool,
        kb_id,
        subject_id,
        predicate_id,
        FactObject::Entity(object_id),
        validity,
        confidence,
    )
    .await
}

/// 一条事实在**世界轴**上的位置：两端各自的时刻与粒度。
///
/// 打包成结构而不是四个平行参数：`Option<DateTime>` 和 `Option<&str>` 各有两个，
/// 相邻同型的参数写反了编译器一声不吭，而这里写反的后果是一条事实的起止颠倒。
///
/// 结束端的三种状态（数据库的 `facts_to_precision_matches_date` 约束在挡）：
///
/// | 语义 | `to` | `to_precision` |
/// |---|---|---|
/// | 仍在持续 | `None` | `None` |
/// | **结束了，不知哪天** | `None` | `Some("unknown")` |
/// | 某时结束 | `Some(t)` | `Some("year"/"month"/"day")` |
///
/// 第二行是后加的。在它之前 `to = None` 同时承载「还在持续」和「不知何时
/// 结束」，于是 "former CEO of Weta Digital" 这种**结束明确、日期缺失**的句子
/// 只能写成前者，图会断言一件原文说已经结束的事。
#[derive(Debug, Clone, Copy, Default)]
pub struct Validity<'a> {
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub from_precision: Option<&'a str>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    pub to_precision: Option<&'a str>,
}

/// `valid_to_precision` 表示「结束了，但不知道是哪天」。
pub const ENDED_UNKNOWN: &str = "unknown";

impl<'a> Validity<'a> {
    /// 起始端已知、结束端未知或不适用。
    pub fn starting(
        from: Option<chrono::DateTime<chrono::Utc>>,
        from_precision: Option<&'a str>,
    ) -> Self {
        Self {
            from,
            from_precision,
            to: None,
            to_precision: None,
        }
    }

    /// 原文说它结束了，但没说哪天。
    pub fn ended_when_unknown(mut self) -> Self {
        self.to = None;
        self.to_precision = Some(ENDED_UNKNOWN);
        self
    }

    /// 这条断言是否已经不再成立——**两种结束都算**。
    ///
    /// 判据写在这里而不是散在各处的 `valid_to.is_some()`：那种写法会把
    /// 「结束了但不知哪天」漏成「仍在持续」，而那正是两端各记精度要修的东西。
    pub fn has_ended(&self) -> bool {
        self.to.is_some() || self.to_precision == Some(ENDED_UNKNOWN)
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_fact_inner(
    pool: &PgPool,
    kb_id: Uuid,
    subject_id: Uuid,
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object: FactObject<'_>,
    validity: Validity<'_>,
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
    if let Some((existing, _, _)) = same.iter().find(|(_, vf, _)| *vf == validity.from) {
        return Ok((*existing, false));
    }
    // 弱化陈述：新观察无时间，同断言已有开放行 → 并入（取起点最新的开放行）
    if validity.from.is_none() && !validity.has_ended() {
        if let Some((existing, _, _)) = same
            .iter()
            .filter(|(_, _, vt)| vt.is_none())
            .max_by_key(|(_, vf, _)| *vf)
        {
            return Ok((*existing, false));
        }
    }
    // 时间精化候选：已有无时无终的裸行，本次观察带了起点 → 落库后作废裸行并链上
    let refine_target = if validity.from.is_some() {
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
                                valid_from, valid_from_precision,
                                valid_to, valid_to_precision, confidence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
        }
        FactObject::Value(_) => {
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                                valid_from, valid_from_precision,
                                valid_to, valid_to_precision, confidence)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
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
    ins.bind(validity.from)
        .bind(validity.from_precision)
        .bind(validity.to)
        .bind(validity.to_precision)
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
            // 表层谓词随证据一起搬：精化的是时间，不是原文说了什么
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version)
             SELECT $1, chunk_id, quote, proposed_predicate, document_id, doc_version
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
    // None = 本体里没有对应的关系。原意不丢——它在证据的 proposed_predicate 里，
    // 显示时由 fact_surface_predicate() 取回（见 `facts.predicate_id`）
    predicate_id: Option<Uuid>,
    object_value: &serde_json::Value,
    validity: Validity<'_>,
    confidence: f32,
) -> AppResult<(Uuid, bool)> {
    insert_fact_inner(
        pool,
        kb_id,
        subject_id,
        predicate_id,
        FactObject::Value(object_value),
        validity,
        confidence,
    )
    .await
}

/// `proposed`：模型在这一块里实际提议的谓词。命中本体时它等于 key，
/// 本体外的谓词不落到关系上时，它是唯一还留着原意的东西——事实行上只剩
/// "有关联"，原文说的"runs on"就靠这里活下来。
pub async fn add_evidence(
    pool: &PgPool,
    fact_id: Uuid,
    chunk_id: Uuid,
    quote: Option<&str>,
    proposed: Option<&str>,
) -> AppResult<()> {
    // 证据落笔即记版本：出自哪份文档的第几版（S3 版本对账与"证据过期"判定的依据）
    // 冲突时补写表层谓词而非整行跳过：重抽命中的多是已有的 (事实, 分块) 对，
    // DO NOTHING 会让存量证据永远填不上这一列。只在原值为空时补，不覆盖——
    // 同一分块的同一条事实，第一次记下的说法就是它的说法
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version)
         SELECT $1, $2, $3, left($4, 120), c.document_id, c.doc_version FROM chunks c WHERE c.id = $2
         ON CONFLICT (fact_id, chunk_id) DO UPDATE
           SET proposed_predicate = COALESCE(fact_evidence.proposed_predicate, EXCLUDED.proposed_predicate)",
    )
    .bind(fact_id)
    .bind(chunk_id)
    .bind(quote)
    .bind(proposed)
    .execute(pool)
    .await?;
    Ok(())
}

/// 所有图查询共用的取节点语句。
///
/// **LEFT JOIN，不是 JOIN**（0009）。没判出类型的实体照样是图上的节点：它有名字、
/// 有事实、有证据，缺的只是一个标签。内连接会让它整个消失——事实还在库里，
/// 图上却查无此人，那是最难发现的一种数据丢失。
///
/// key 与 label 留 NULL，**颜色和形状给缺省值**：前者是身份，没有就该说没有；
/// 后者是画布必须拿到的东西，编不出来就没法渲染。灰色圆点正是「还没定」的样子
/// `as_of`：绑记录轴参数的位置（`None` = 只答现在，写路径和"当下"视图用这个）。
/// 度数跟着画布走——回放时数的是**当时**连在这个节点上的边，否则右上角的数
/// 和眼前的图对不上。
fn node_sql(as_of: Option<usize>) -> String {
    let held = match as_of {
        Some(param) => crate::record_axis::facts_held_at("f", param),
        None => "f.invalidated_at IS NULL".to_string(),
    };
    format!(
        "SELECT e.id, e.canonical_name AS name, t.key AS type_key,
        t.label AS type_label,
        coalesce(t.color, '#94a3b8') AS color,
        coalesce(t.shape, 'circle') AS shape,
        e.disambiguator,
        (SELECT count(*) FROM facts f
         WHERE (f.subject_id = e.id OR f.object_id = e.id) AND {held}) AS degree
     FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id"
    )
}

/// 全图概览：按度数取 top N 实体及其间的边。
/// `at`：世界轴——只返回 T 时刻**有效**的边（起点不晚于 T 或未知，终点晚于 T 或开放）。
/// `as_of`：记录轴（0019）——只返回 T 时刻**我们持有**的边，三月被改掉的断言在
/// 三月之前的位置上应当还在。两个参数一路分开到 API：折成一个控件，就会拿
/// 「三月的世界，以今天的认知」去答「三月的世界，以三月的认知」，而两者在
/// 屏幕上都说得通。
/// 图谱总览：度数最高的 `limit` 个节点，以及它们之间的边。
///
/// **一并回总数。** 画多少个是渲染的事，库里有多少是知识库的事，两者从前
/// 在界面上被同一个数字表示——一个上万实体的库，右上角永远写着 150，而那
/// 是上限不是规模。渲染上限本身是合理的（画一万个点没人看得懂），骗人的是
/// 把它说成总数。
pub async fn overview(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, Vec<GraphEdge>, i64, i64)> {
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.merged_into IS NULL ORDER BY degree DESC, e.created_at LIMIT $2",
        node_sql(Some(3))
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(as_of)
    .fetch_all(pool)
    .await?;

    let ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
    let edges = edges_among(pool, kb_id, &ids, at, as_of).await?;

    // 总数按与画布同一套口径数：合并掉的实体不算，作废的事实不算，
    // 属性事实（宾语是字面值）画不出边所以也不算。口径不同的话，
    // 「150 / 325」里那个 325 会跟用户在别处看到的数对不上——**回放时也一样**，
    // 边数跟着记录轴走，否则倒回三月的图上写着今天的边数。
    //
    // 节点数没跟着倒：实体身上没有记录轴（`merged_into` 只说合并发生过，
    // 时刻在 `entity_merges` 里），要倒得顺着那张表拆合并，是另一刀（0019 遗留问题）
    let total_nodes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entities WHERE kb_id = $1 AND merged_into IS NULL",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    let total_edges: i64 = sqlx::query_scalar(&format!(
        "SELECT (SELECT count(*) FROM facts f
                  WHERE f.kb_id = $1 AND {facts_held} AND f.object_id IS NOT NULL)
              + (SELECT count(*) FROM derived_facts d
                  WHERE d.kb_id = $1 AND {derived_held})",
        facts_held = crate::record_axis::facts_held_at("f", 2),
        derived_held = crate::record_axis::derived_held_at("d", 2),
    ))
    .bind(kb_id)
    .bind(as_of)
    .fetch_one(pool)
    .await?;
    Ok((nodes, edges, total_nodes, total_edges))
}

async fn edges_among(
    pool: &PgPool,
    kb_id: Uuid,
    ids: &[Uuid],
    at: Option<chrono::DateTime<chrono::Utc>>,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<GraphEdge>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // **派生边显式 UNION 进来。** 它们住在 `derived_facts`，不在 `facts` 里——
    // 所以每一个想看到推理结果的读路径都得像这里一样写出来。忘了写的后果是
    // 看不见派生，而不是把它们当成谁的断言（那正是分表买到的东西）。
    //
    // 图要它们，因为「这条边是推出来的」正是用户该看见的信息之一；`derived`
    // 那一位让界面画得出区别，也让人整体过滤掉。
    //
    // 第三段是**幽灵边**（0017 §3）：推出来却没落地的派生，住在 `axiom_violations`
    // 的 `detail` 里。它的 id 是违规的 id；`derived` 与 `blocked` 同时为 true，
    // 界面据此让它跟着派生开关走、画成争议色往背景混的那一档。
    //
    // 断言那一段多算一位 `contested`：有 open 的违规或时态冲突指着它。派生撞断言
    // 时被撞的是 left；right 只是最后一条前提，它本身没有争议
    let edges: Vec<GraphEdge> = sqlx::query_as(&format!(
        "SELECT f.id, f.subject_id AS source, f.object_id AS target,
                COALESCE(r.key, fact_surface_predicate(f.id)) AS predicate,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS label,
                r.id IS NULL AS inferred, FALSE AS derived, NULL::text AS rule,
                ARRAY[]::uuid[] AS premises,
                f.valid_from, f.valid_to, f.confidence,
                (EXISTS (SELECT 1 FROM axiom_violations v
                          WHERE {violation_open}
                            AND (v.left_fact = f.id
                                 OR (v.right_fact = f.id AND v.kind <> 'derived_contradiction')))
                 OR EXISTS (SELECT 1 FROM fact_conflicts c
                             WHERE {conflict_open}
                               AND (c.old_fact_id = f.id OR c.new_fact_id = f.id))
                ) AS contested,
                FALSE AS blocked
         FROM facts f LEFT JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND {facts_held} AND f.object_id IS NOT NULL
           AND f.subject_id = ANY($2) AND f.object_id = ANY($2)
           AND ($3::timestamptz IS NULL
                OR ((f.valid_from IS NULL OR f.valid_from <= $3)
                    AND (f.valid_to IS NULL OR f.valid_to > $3)))
         UNION ALL
         SELECT d.id, d.subject_id AS source, d.object_id AS target,
                r.key AS predicate, r.label AS label,
                FALSE AS inferred, TRUE AS derived, ru.kind AS rule,
                ARRAY(SELECT fd.premise_fact_id FROM fact_derivations fd
                       WHERE fd.derived_fact_id = d.id ORDER BY fd.seq) AS premises,
                d.valid_from, d.valid_to, d.confidence,
                FALSE AS contested, FALSE AS blocked
         FROM derived_facts d JOIN relation_types r ON r.id = d.predicate_id
                              JOIN rules ru ON ru.id = d.rule_id
         WHERE d.kb_id = $1 AND {derived_held}
           AND d.subject_id = ANY($2) AND d.object_id = ANY($2)
           AND ($3::timestamptz IS NULL
                OR ((d.valid_from IS NULL OR d.valid_from <= $3)
                    AND (d.valid_to IS NULL OR d.valid_to > $3)))
         UNION ALL
         SELECT v.id,
                (v.detail->>'subject_id')::uuid AS source,
                (v.detail->>'object_id')::uuid AS target,
                v.detail->>'predicate' AS predicate, v.detail->>'predicate' AS label,
                FALSE AS inferred, TRUE AS derived, v.detail->>'rule' AS rule,
                v.path AS premises,
                (v.detail->>'valid_from')::timestamptz AS valid_from,
                (v.detail->>'valid_to')::timestamptz AS valid_to,
                0::real AS confidence,
                TRUE AS contested, TRUE AS blocked
         FROM axiom_violations v
         WHERE v.kb_id = $1 AND v.kind = 'derived_contradiction' AND {violation_open}
           AND (v.detail->>'subject_id')::uuid = ANY($2)
           AND (v.detail->>'object_id')::uuid = ANY($2)
           AND ($3::timestamptz IS NULL
                OR (((v.detail->>'valid_from')::timestamptz IS NULL
                     OR (v.detail->>'valid_from')::timestamptz <= $3)
                    AND ((v.detail->>'valid_to')::timestamptz IS NULL
                         OR (v.detail->>'valid_to')::timestamptz > $3)))",
        facts_held = crate::record_axis::facts_held_at("f", 4),
        derived_held = crate::record_axis::derived_held_at("d", 4),
        violation_open = crate::record_axis::violation_open_at("v", 4),
        conflict_open = crate::record_axis::conflict_open_at("c", 4),
    ))
    .bind(kb_id)
    .bind(ids)
    .bind(at)
    .bind(as_of)
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
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
    const MAX_NODES: usize = 300;
    let mut seen: HashSet<Uuid> = HashSet::from([entity_id]);
    let mut frontier: Vec<Uuid> = vec![entity_id];

    for _ in 0..hops.clamp(1, 2) {
        if frontier.is_empty() || seen.len() >= MAX_NODES {
            break;
        }
        // 铺开也走记录轴：邻居按**当时**的边找，否则回放的图上会长出
        // 只有今天才连得上的节点
        let touching: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(&format!(
            "SELECT f.subject_id, f.object_id FROM facts f
             WHERE f.kb_id = $1 AND {facts_held} AND f.object_id IS NOT NULL
               AND (f.subject_id = ANY($2) OR f.object_id = ANY($2))",
            facts_held = crate::record_axis::facts_held_at("f", 3),
        ))
        .bind(kb_id)
        .bind(&frontier)
        .bind(as_of)
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
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = ANY($2)",
        node_sql(Some(3))
    ))
    .bind(kb_id)
    .bind(&ids)
    .bind(as_of)
    .fetch_all(pool)
    .await?;
    let edges = edges_among(pool, kb_id, &ids, at, as_of).await?;
    Ok((nodes, edges))
}

/// 按名字找实体。**一并回总数**——「宁分勿合」本来就会造出一堆同名，
/// 固定十条的时候，想找的那个可能根本不在这十条里而界面上看不出来。
pub async fn search_entities(
    pool: &PgPool,
    kb_id: Uuid,
    q: &str,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<GraphNode>, i64)> {
    let pattern = format!("%{}%", q.trim());
    let nodes: Vec<GraphNode> = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.merged_into IS NULL
         AND e.canonical_name ILIKE $2
         ORDER BY degree DESC, e.canonical_name LIMIT $3 OFFSET $4",
        node_sql(None)
    ))
    .bind(kb_id)
    .bind(&pattern)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM entities e
          WHERE e.kb_id = $1 AND e.merged_into IS NULL AND e.canonical_name ILIKE $2",
    )
    .bind(kb_id)
    .bind(&pattern)
    .fetch_one(pool)
    .await?;
    Ok((nodes, total))
}

/// 实体详情：节点信息 + 事实时间线。
pub async fn entity_detail(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<(GraphNode, Vec<EntityFact>)> {
    let node: GraphNode = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = $2",
        node_sql(Some(3))
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(as_of)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let facts: Vec<EntityFact> = sqlx::query_as(&format!(
        "SELECT f.id,
                CASE WHEN f.subject_id = $2 THEN 'out' ELSE 'in' END AS direction,
                COALESCE(r.key, fact_surface_predicate(f.id)) AS predicate_key,
                COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                r.id IS NULL AS inferred, r.temporal,
                CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END AS other_id,
                o.canonical_name AS other_name, f.object_value,
                f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
                 AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                                 JOIN chunks c ON c.id = fe.chunk_id
                                 WHERE fe.fact_id = f.id AND {chunk_live})
                ) AS stale,
                (f.supersedes IS NOT NULL) AS corrected,
                (SELECT MAX(COALESCE(d.doc_time, d.created_at))
                 FROM fact_evidence fe JOIN documents d ON d.id = fe.document_id
                 WHERE fe.fact_id = f.id) AS last_evidence_time,
                COALESCE(
                    (SELECT jsonb_build_object(
                                'kind', v.kind, 'ref_id', v.id,
                                'derived', CASE WHEN v.kind = 'derived_contradiction'
                                    THEN (v.detail->>'subject') || ' · '
                                         || (v.detail->>'predicate') || ' · '
                                         || (v.detail->>'object') END)
                       FROM axiom_violations v
                      WHERE {violation_open}
                        AND (v.left_fact = f.id
                             OR (v.right_fact = f.id AND v.kind <> 'derived_contradiction'))
                      ORDER BY v.detected_at DESC LIMIT 1),
                    (SELECT jsonb_build_object('kind', 'temporal_conflict', 'ref_id', c.id)
                       FROM fact_conflicts c
                      WHERE {conflict_open}
                        AND (c.old_fact_id = f.id OR c.new_fact_id = f.id)
                      ORDER BY c.created_at DESC LIMIT 1)
                ) AS contested
         FROM facts f
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o
           ON o.id = CASE WHEN f.subject_id = $2 THEN f.object_id ELSE f.subject_id END
         WHERE f.kb_id = $1 AND {facts_held}
           AND (f.subject_id = $2 OR f.object_id = $2)
         ORDER BY f.valid_from NULLS LAST, f.recorded_at",
        facts_held = crate::record_axis::facts_held_at("f", 3),
        chunk_live = crate::record_axis::chunk_live_at("c", 3),
        violation_open = crate::record_axis::violation_open_at("v", 3),
        conflict_open = crate::record_axis::conflict_open_at("c", 3),
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(as_of)
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
        "{} WHERE e.kb_id = $1 AND e.id = $2 AND e.merged_into IS NULL",
        node_sql(None)
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
                return Err(AppError::invalid(
                    "entity_name_required",
                    "Name cannot be empty",
                ));
            }
            // 与抽取侧同一上限：越过这条线的多半是整句被当成了名字
            if n.chars().count() > 100 {
                return Err(AppError::invalid(
                    "entity_name_too_long",
                    "Name is too long (max 100)",
                ));
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
            return Err(AppError::invalid(
                "unknown_entity_type",
                "No such entity type in this KB",
            ));
        }
    }

    sqlx::query(
        // 改了类型才标 human——这个端点也用来改名字，只改名不该顺手把类型
        // 的来源盖成人工。`$3 IS NULL` 在这个接口里表示「本次没提供类型」，
        // 不是「把类型清空」：路由层要求两个字段至少给一个，给不出三态。
        //
        // 于是有一件今天做不到的事：0009 之后「没有类型」可能是人的决定
        //（看过了，本体里没有合适的类），而这个接口表达不了它。要补得让
        // 请求体区分「未提供」与「显式置空」，那是另一件事
        "UPDATE entities
         SET type_id = COALESCE($3, type_id),
             canonical_name = COALESCE($4, canonical_name),
             type_source = CASE WHEN $3::uuid IS NULL THEN type_source ELSE 'human' END,
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

    let after: GraphNode = sqlx::query_as(&format!(
        "{} WHERE e.kb_id = $1 AND e.id = $2",
        node_sql(None)
    ))
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
        "{} WHERE e.kb_id = $1 AND e.merged_into IS NULL AND e.id <> $2
           AND lower(e.canonical_name) = (SELECT lower(canonical_name) FROM entities WHERE id = $2)
         ORDER BY degree DESC LIMIT 10",
        node_sql(None)
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
    offset: i64,
) -> AppResult<Vec<FactReviewItem>> {
    let rows: Vec<FactReviewItem> = sqlx::query_as(
        "SELECT f.id, s.canonical_name AS subject_name, COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                COALESCE(o.canonical_name, f.object_value->>'summary') AS object_name,
                f.valid_from, f.valid_to, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (SELECT fe.quote FROM fact_evidence fe
                 WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL AND f.confidence < $2
         ORDER BY f.confidence, f.recorded_at DESC
         LIMIT $3 OFFSET $4",
    )
    .bind(kb_id)
    .bind(below)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// "证据全部停留在旧版"的现行事实（S3 第三刀：文档新版没再确认的知识）。
/// 判定纯派生自 chunk 存活性——认领机制保证未变段落的证据不被误伤；
/// 绝不自动删除（没再提 ≠ 不成立），删除/闭合权在 Review 的人手里。
pub async fn stale_facts(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<FactReviewItem>> {
    let rows: Vec<FactReviewItem> = sqlx::query_as(
        "SELECT f.id, s.canonical_name AS subject_name, COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate_label,
                COALESCE(o.canonical_name, f.object_value->>'summary') AS object_name,
                f.valid_from, f.valid_to, f.confidence,
                (SELECT count(*) FROM fact_evidence fe WHERE fe.fact_id = f.id) AS evidence_count,
                (SELECT fe.quote FROM fact_evidence fe
                 WHERE fe.fact_id = f.id AND fe.quote IS NOT NULL LIMIT 1) AS quote
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
         LEFT JOIN entities o ON o.id = f.object_id
         WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
           AND EXISTS (SELECT 1 FROM fact_evidence fe WHERE fe.fact_id = f.id)
           AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                           JOIN chunks c ON c.id = fe.chunk_id
                           WHERE fe.fact_id = f.id AND c.superseded_at IS NULL)
         ORDER BY f.recorded_at DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
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
    // 从前这里还有一段：确认 `mapped_to` 事实时把同 (概念, 源) 的旧映射作废。
    // 映射已搬出账本（0011，`concept_mappings` 自己管唯一性），那段 SQL 恒匹配零行，删了。
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
                COALESCE(r.label, fact_surface_predicate(f.id)) AS predicate,
                r.id IS NULL AS inferred,
                f.object_id, o.canonical_name AS object,
                f.valid_from, f.valid_to, f.confidence
         FROM fact_evidence fe
         JOIN chunks c ON c.id = fe.chunk_id AND c.document_id = $1
              AND c.superseded_at IS NULL
         JOIN facts f ON f.id = fe.fact_id AND f.invalidated_at IS NULL
         JOIN entities s ON s.id = f.subject_id
         LEFT JOIN relation_types r ON r.id = f.predicate_id
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
        "SELECT fe.quote, fe.proposed_predicate, fe.chunk_id, c.document_id, d.filename, c.seq,
                c.doc_version,
                c.doc_version < COALESCE(
                    (SELECT MAX(version) FROM document_versions dv
                     WHERE dv.document_id = c.document_id), 1) AS stale,
                d.deleted_at IS NOT NULL AS document_deleted
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
            -- 作废且无后继 = 被推翻……除非它是被并进了另一条断言。那种情形下
            -- 内容一字未少，说成「撤回」就是界面在陈述一件没发生的事
            SELECT ef.*, ef.invalidated_at AS at,
                   CASE WHEN EXISTS (SELECT 1 FROM fact_adoptions fa
                                     WHERE fa.old_fact_id = ef.id AND fa.mode = 'merged'
                                       AND fa.reverted_at IS NULL)
                        THEN 'merged' ELSE 'rejected' END AS kind
            FROM ef
            WHERE ef.invalidated_at IS NOT NULL
              AND NOT EXISTS (SELECT 1 FROM facts s WHERE s.supersedes = ef.id)
        ),
        -- 改类不是事实：没有谓词、没有对方、没有方向。它来自 entity_retypes，
        -- 一行最多产出两个事件——改动本身，以及撤销。
        --
        -- **撤销过的照样显示。** 读成「改过、又撤了」，不是没发生过。同一类错
        -- 这仓库栽过两次（#37；并入被读成撤回），所以这不是防御性编程
        rt AS (
            SELECT r.created_at AS at, 'retyped' AS kind, r.actor_id,
                   tf.label AS from_type_label, tt.label AS to_type_label
            FROM entity_retypes r
            LEFT JOIN entity_types tf ON tf.id = r.from_type_id
            JOIN entity_types tt ON tt.id = r.to_type_id
            WHERE r.kb_id = $1 AND r.entity_id = $2
            UNION ALL
            SELECT r.reverted_at, 'retype_reverted', r.actor_id, tf.label, tt.label
            FROM entity_retypes r
            LEFT JOIN entity_types tf ON tf.id = r.from_type_id
            JOIN entity_types tt ON tt.id = r.to_type_id
            WHERE r.kb_id = $1 AND r.entity_id = $2 AND r.reverted_at IS NOT NULL
        )";
    let rows: Vec<EntityHistoryEvent> = sqlx::query_as(&format!(
        "{EVENTS}
         SELECT * FROM (
         SELECT ev.id AS fact_id, ev.at, ev.kind, ev.direction,
                COALESCE(r.label, fact_surface_predicate(ev.id)) AS predicate_label, o.canonical_name AS other_name,
                ev.object_value, ev.valid_from, ev.valid_from_precision,
                ev.valid_to, ev.valid_to_precision,
                ev.confidence, act.actor_name, act.action,
                src.document_id, src.filename, src.quote,
                NULL::text AS from_type_label, NULL::text AS to_type_label
         FROM ev
         LEFT JOIN relation_types r ON r.id = ev.predicate_id
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
                     WHEN 'corrected' THEN
                       ARRAY['fact.close', 'conflict.close_old', 'ontology.predicate_adopted']
                     -- 并入只可能由采纳造成，不会是 Review 里的拒绝
                     WHEN 'merged' THEN ARRAY['ontology.predicate_adopted']
                     ELSE ARRAY['fact.reject', 'conflict.reject_new',
                                'ontology.adoption_reverted'] END)
               AND (a.target_id = COALESCE(ev.supersedes, ev.id)
                    OR a.target_id IN (SELECT c.id FROM fact_conflicts c
                                       WHERE c.old_fact_id = COALESCE(ev.supersedes, ev.id)
                                          OR c.new_fact_id = ev.id)
                    -- 采纳与撤销都记在关系类型上、一次动作改一批事实，
                    -- 靠 fact_adoptions 精确关联到具体哪几条（corrected 事件
                    -- 是新行、merged 是旧行，两头都认）
                    OR (a.action IN ('ontology.predicate_adopted',
                                     'ontology.adoption_reverted')
                        AND EXISTS (SELECT 1 FROM fact_adoptions fa
                                    WHERE fa.predicate_id = a.target_id
                                      AND (fa.new_fact_id = ev.id
                                           OR fa.old_fact_id = ev.id))))
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
         UNION ALL
         SELECT NULL::uuid, rt.at, rt.kind, NULL::text,
                NULL::text, NULL::text,
                NULL::jsonb, NULL::timestamptz, NULL::text,
                NULL::timestamptz, NULL::text,
                NULL::real, u.display_name, NULL::text,
                NULL::uuid, NULL::text, NULL::text,
                rt.from_type_label, rt.to_type_label
         FROM rt LEFT JOIN users u ON u.id = rt.actor_id
         ) x
         ORDER BY x.at DESC, x.fact_id
         LIMIT $3 OFFSET $4"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    // 总数把改类那一支也算上，否则分页会少一截
    let (total,): (i64,) = sqlx::query_as(&format!(
        "{EVENTS} SELECT (SELECT count(*) FROM ev) + (SELECT count(*) FROM rt)"
    ))
    .bind(kb_id)
    .bind(entity_id)
    .fetch_one(pool)
    .await?;
    Ok((rows, total))
}

/// 一段记录时间窗口里，全库的认知变更。
///
/// **窗口开在认知轴上**：`since`/`until` 比的是 recorded_at 与 invalidated_at，
/// 不是 valid_from/valid_to。这是与 `entity_facts(at)` 唯一也是全部的区别——
/// 那个问"某时刻世界什么样"，这个问"某段时间里我们改了什么主意"。两者查同一张表、
/// 用不同的列，混起来会安静地给出一个看着合理的错答案。
///
/// 事件推导与 `entity_history` 同源（见那里的注释）：一条事实行最多产出两个事件，
/// 且已被后继修正的死亡不重复记。
pub async fn graph_changes(
    pool: &PgPool,
    kb_id: Uuid,
    since: chrono::DateTime<chrono::Utc>,
    until: chrono::DateTime<chrono::Utc>,
    entity_id: Option<Uuid>,
    kinds: Option<&[String]>,
    limit: i64,
) -> AppResult<Vec<GraphChange>> {
    // 两个分支各自按**自己那根时间列**开窗，而不是先union再过滤：
    // 一条 2 月写入、8 月被推翻的事实，在"3–4 月"窗口里两个事件都不该出现
    const EVENTS: &str = "
        WITH ev AS (
            SELECT f.id, f.subject_id, f.predicate_id, f.object_id, f.object_value,
                   f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision, f.confidence,
                   f.recorded_at AS at,
                   CASE WHEN f.supersedes IS NULL THEN 'asserted' ELSE 'corrected' END AS kind
            FROM facts f
            WHERE f.kb_id = $1 AND f.recorded_at >= $2 AND f.recorded_at < $3
              AND ($4::uuid IS NULL OR f.subject_id = $4 OR f.object_id = $4)
            UNION ALL
            SELECT f.id, f.subject_id, f.predicate_id, f.object_id, f.object_value,
                   f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision, f.confidence,
                   f.invalidated_at AS at,
                   CASE WHEN EXISTS (SELECT 1 FROM fact_adoptions fa
                                     WHERE fa.old_fact_id = f.id AND fa.mode = 'merged'
                                       AND fa.reverted_at IS NULL)
                        THEN 'merged' ELSE 'rejected' END AS kind
            FROM facts f
            WHERE f.kb_id = $1 AND f.invalidated_at >= $2 AND f.invalidated_at < $3
              AND NOT EXISTS (SELECT 1 FROM facts s WHERE s.supersedes = f.id)
              AND ($4::uuid IS NULL OR f.subject_id = $4 OR f.object_id = $4)
        )";
    Ok(sqlx::query_as(&format!(
        "{EVENTS}
         SELECT ev.id AS fact_id, ev.at, ev.kind,
                ev.subject_id, s.canonical_name AS subject_name,
                COALESCE(r.label, fact_surface_predicate(ev.id)) AS predicate_label, o.canonical_name AS object_name,
                ev.object_value, ev.valid_from, ev.valid_from_precision,
                ev.valid_to, ev.valid_to_precision,
                ev.confidence, src.document_id, src.filename, src.quote
         FROM ev
         LEFT JOIN relation_types r ON r.id = ev.predicate_id
         JOIN entities s ON s.id = ev.subject_id
         LEFT JOIN entities o ON o.id = ev.object_id
         LEFT JOIN LATERAL (
             SELECT d.id AS document_id, d.filename, fe.quote
             FROM fact_evidence fe
             JOIN chunks c ON c.id = fe.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE fe.fact_id = ev.id
             ORDER BY fe.doc_version DESC NULLS LAST LIMIT 1
         ) src ON true
         WHERE $5::text[] IS NULL OR ev.kind = ANY($5)
         ORDER BY ev.at DESC, ev.id
         LIMIT $6"
    ))
    .bind(kb_id)
    .bind(since)
    .bind(until)
    .bind(entity_id)
    .bind(kinds)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

// ---------------------------------------------------------------------------
// 谓词消解：把没有谓词的事实认领回本体
// ---------------------------------------------------------------------------

// 视图类型 ProposedPredicate 定义在 utopia-core::models（store 不直接依赖 serde）

/// 没有谓词的事实上，原文用过哪些说法。
///
/// 这是本体扩展建议的证据基础——比 `ontology_misses` 的纯计数强的地方在于：
/// 它连着具体事实，所以采纳一个说法时能直接说"将重新归类 57 条"并真的去改。
pub async fn proposed_predicates(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ProposedPredicate>> {
    Ok(sqlx::query_as(
        // **普遍程度从全量证据里数，不从积压里数。**
        //
        // 下面那些 WHERE 把行集收窄到「还没有谓词、还活着、宾语是实体」
        // ——那是**采纳要改写的东西**，`fact_count` 该这么数。但 `doc_count`
        // 回答的是另一个问题：这个说法在语料里有多普遍。拿残渣去数它会系统性
        // 偏低，而且越用越低——说法一旦被采纳、被谓词匹配接住、或被修正作废，
        // 它的行就离开积压了。一篇一篇往里灌的库尤其吃亏：每轮搬走一批，
        // 剩下的永远攒不够两篇，本体于是永远长不起来。
        //
        // 实测（ai-timeline 348 块）：两种口径下 8 个说法分处门槛两侧，
        // 按积压数是「只在 1 篇」、按全量数是「≥2 篇」。
        //
        // 走 CTE 而不是相关子查询：后者每组重扫一遍证据表，同一份数据上
        // 360ms 对 7ms。这个函数每次 Suggest 和每次自动扩本体都要跑。
        "WITH spread AS (
             SELECT e.proposed_predicate AS form,
                    count(DISTINCT e.document_id) AS doc_count
             FROM fact_evidence e
             JOIN facts ff ON ff.id = e.fact_id
             WHERE ff.kb_id = $1 AND e.proposed_predicate IS NOT NULL
             GROUP BY 1
         )
         SELECT fe.proposed_predicate AS form,
                count(DISTINCT f.id) AS fact_count,
                max(sp.doc_count) AS doc_count,
                (SELECT s.canonical_name || ' → ' || o.canonical_name
                 FROM fact_evidence e2
                 JOIN facts f2 ON f2.id = e2.fact_id
                 JOIN entities s ON s.id = f2.subject_id
                 JOIN entities o ON o.id = f2.object_id
                 WHERE e2.proposed_predicate = fe.proposed_predicate
                   AND f2.kb_id = $1 AND f2.predicate_id IS NULL AND f2.invalidated_at IS NULL
                 LIMIT 1) AS example
         FROM fact_evidence fe
         JOIN facts f ON f.id = fe.fact_id
         JOIN spread sp ON sp.form = fe.proposed_predicate
         WHERE f.kb_id = $1 AND f.predicate_id IS NULL
           AND f.invalidated_at IS NULL AND fe.proposed_predicate IS NOT NULL
           -- 字面值宾语的不算：它们同样没有谓词、也带原文说法，但要的是
           -- 一个属性而不是一个关系。混进来提案就会照着建关系，然后
           -- `founding_date` 变成一条指向「2015」的边——正是这条路要修掉的东西
           AND f.object_id IS NOT NULL
           -- 用户拒绝过的说法不再出现在候选里（人工与自动两条路都据此绕开）
           AND NOT EXISTS (SELECT 1 FROM ontology_misses m
                           WHERE m.kb_id = $1 AND m.kind = 'relation_type'
                             AND m.key = fe.proposed_predicate AND m.dismissed_at IS NOT NULL)
         GROUP BY fe.proposed_predicate
         ORDER BY fact_count DESC, form",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 每个待认领说法出现在**哪些文档**里。
///
/// [`proposed_predicates`] 已经给了 `doc_count`，但采纳那条路要先按屈折基把说法
/// 归并（`sued` 与 `sues` 是一个关系），归并之后的篇数是**并集**而不是相加——
/// 同一篇文档完全可能两种写法都用过，相加就成了重复计数，一篇文档能把一个说法
/// 顶过「≥2 篇」的门槛。
///
/// **不筛兜底谓词。** 这条查询与 [`proposed_predicates`] 回答的是两个问题：
/// 那条问「还有哪些说法等着被采纳」，看的是积压；这条问「这个说法有多普遍」，
/// 看的是全量证据，条件与它内部那个 `spread` CTE 一致。
///
/// 第一版照抄了 `rt.key = 'related_to'`（当时还有那个兜底关系），理由写的是「两处条件要一致」——错的。
/// 那样数出来的还是残渣：说法一旦被采纳、被谓词匹配接住、或被修正作废，
/// 它的行就离开积压，篇数随之下降。一篇一篇往里灌的库因此永远攒不够两篇。
/// 测试当场抓住了（两篇里只回来一篇）。
pub async fn proposed_predicate_documents(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<Vec<(String, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT fe.proposed_predicate, fe.document_id
         FROM fact_evidence fe
         JOIN facts f ON f.id = fe.fact_id
         WHERE f.kb_id = $1
           AND fe.proposed_predicate IS NOT NULL
           AND fe.document_id IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM ontology_misses m
                           WHERE m.kb_id = $1 AND m.kind = 'relation_type'
                             AND m.key = fe.proposed_predicate AND m.dismissed_at IS NOT NULL)",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 把由 `forms` 认出来的无谓词事实改写到 `predicate_id`。
/// 返回 (批次 id, 改写条数)——批次 id 是撤销的把手。
///
/// **追加而非原地改**：插入带 `supersedes` 的新行并作废旧行，与人工纠正、
/// 时态闭合走同一条路——认知变更本身是信息，实体历史里读得到
/// "先记成 related to，后精化成 available on"。
///
/// 只改写说法**全部**落在 `forms` 内的事实：一条事实可能积累多种说法
/// （甲块 "runs on"、乙块 "optimized for"），只认领了其中一种就改写等于替
/// 另一种也做了决定。实测这类事实占比不到 1%，宁可漏也不猜。
///
/// 每条去向都写进 `fact_adoptions`。`supersedes` 一个指针不够用——目标断言
/// 已存在时走的是"并入"，旧行被作废却没有后继，于是既撤不回来、实体历史
/// 又会把它判成 rejected 而对外宣称"这条被撤回了"（它其实一字未少地并进了
/// 另一条）。
/// `swap = true`：这些说法是目标关系的**被动形**，改写时主宾要对调。
///
/// `X produced_by Y` 与 `Y produces X` 是同一条边。不对调就会在图上多出一条
/// 反着的箭头，而且它跟正向那些永远合不到一起——同一件事分在两个方向上。
pub async fn adopt_proposed_predicates(
    pool: &PgPool,
    kb_id: Uuid,
    predicate_id: Uuid,
    forms: &[String],
    swap: bool,
) -> AppResult<Adopted> {
    adopt(pool, kb_id, predicate_id, AdoptTargets::ByForm(forms), swap).await
}

/// 一次采纳的账。`left_off` 必须往上传：改写了 20 条、留下 3 条，只报前半句是报喜不报忧。
#[derive(Debug, Clone, Copy)]
pub struct Adopted {
    pub batch_id: Uuid,
    /// 改写过去的条数
    pub moved: u32,
    /// 签名两边都对不上、**没有**挂上谓词的条数（#190）。它们照旧留在空谓词上，
    /// 原文说法还在证据里——与抽取遇到同样情形的处置一致
    pub left_off: u32,
    /// 按签名对调了主宾的条数。
    ///
    /// **掰正不该静默。** 抽取那条路每掰一次就落一条 `direction_corrected`
    /// 丢弃信号（#138 的原话是「绝不静默」：用可能错的声明驱动的自动动作，
    /// 留痕才不属于 0001 判据 2 反对的那一类）。采纳走的是同一道判断，
    /// 也该说出来自己动了几条，否则台账上只剩「改写了 N 条」，看不出其中
    /// 有几条是被本体掉了个头的
    pub corrected: u32,
}

/// 要改写哪些事实，以及新行的宾语从哪来。
pub enum AdoptTargets<'a> {
    /// 关系那一路：按表层说法去找，宾语原样搬走。
    ByForm(&'a [String]),
    /// 属性那一路：调用方已经挑好事实、并把值按 datatype 归一化过。
    ///
    /// **归一化必须在调用方做**：那套规则（"2015" → 日期、"1,200" → 数字）
    /// 住在抽取模块，store 够不着也不该够得着。更要紧的是它会**失败**——
    /// 一个换算不出来的值不该硬塞进一个日期属性里，那条事实宁可继续没有
    /// 谓词（原词还在证据里）。所以由调用方筛完再交回来。
    WithValues(&'a [(Uuid, serde_json::Value)]),
}

async fn adopt(
    pool: &PgPool,
    kb_id: Uuid,
    predicate_id: Uuid,
    targets: AdoptTargets<'_>,
    swap: bool,
) -> AppResult<Adopted> {
    let batch_id = Uuid::now_v7();
    let nothing = Adopted {
        batch_id,
        moved: 0,
        left_off: 0,
        corrected: 0,
    };
    let targets: Vec<(Uuid, Uuid, Option<Uuid>, Option<serde_json::Value>)> = match targets {
        AdoptTargets::ByForm(forms) => {
            if forms.is_empty() {
                return Ok(nothing);
            }
            sqlx::query_as(
                "SELECT f.id, f.subject_id, f.object_id, f.object_value
                 FROM facts f
                 WHERE f.kb_id = $1 AND f.predicate_id IS NULL AND f.invalidated_at IS NULL
                   -- **只碰宾语是实体的。** 同一个说法可能既有指向实体的事实
                   -- 又有带字面值的（location 两种都用），后者归属性那条路：
                   -- 把它改挂到一条关系上，那个值就再也不是值了
                   AND f.object_id IS NOT NULL
                   AND EXISTS (SELECT 1 FROM fact_evidence e
                               WHERE e.fact_id = f.id AND e.proposed_predicate = ANY($2))
                   AND NOT EXISTS (SELECT 1 FROM fact_evidence e
                                   WHERE e.fact_id = f.id AND e.proposed_predicate IS NOT NULL
                                     AND NOT (e.proposed_predicate = ANY($2)))
                 ORDER BY f.recorded_at",
            )
            .bind(kb_id)
            .bind(forms)
            .fetch_all(pool)
            .await?
        }
        AdoptTargets::WithValues(items) => {
            if items.is_empty() {
                return Ok(nothing);
            }
            // 主语要从库里读回来（调用方给的是 fact_id 与新值），顺带确认这些
            // 事实还活着——挑选与采纳之间可能隔着一次重抽
            let ids: Vec<Uuid> = items.iter().map(|(id, _)| *id).collect();
            let live: Vec<(Uuid, Uuid)> = sqlx::query_as(
                "SELECT id, subject_id FROM facts
                 WHERE kb_id = $1 AND id = ANY($2) AND invalidated_at IS NULL",
            )
            .bind(kb_id)
            .bind(&ids)
            .fetch_all(pool)
            .await?;
            let subject_of: std::collections::HashMap<Uuid, Uuid> = live.into_iter().collect();
            items
                .iter()
                .filter_map(|(id, value)| {
                    Some((*id, *subject_of.get(id)?, None, Some(value.clone())))
                })
                .collect()
        }
    };

    let mut moved = 0u32;
    let mut left_off = 0u32;
    let mut corrected = 0u32;
    for (old_id, subject_id, object_id, object_value) in targets {
        // 被动形改写：主宾对调。字面值宾语的事实换不了（值不能当主语），
        // 而 ByForm 那条查询本来就只取 object_id 非空的，所以这里只可能是实体宾语
        let (subject_id, object_id) = match (swap, object_id) {
            (true, Some(o)) => (o, Some(subject_id)),
            _ => (subject_id, object_id),
        };
        // **挂谓词之前过一遍签名**（#190）。抽取写入时按 domain 掰正方向或留空，
        // 而采纳是第二条写谓词的路——从前直接把谓词挂回去，实测把违反率从 0 抬到
        // 12.3%，全在包关系上。同一道判断（`ontology::judge_direction`），同样三种
        // 结果：合就挂，主语不合宾语合就对调着挂，都不合就**不挂**——那条事实留在
        // 空谓词上，原文说法还在证据里，与抽取遇到同样情形的处置一致
        let (subject_id, object_id) = match object_id {
            Some(o) => {
                match crate::ontology::judge_direction(pool, predicate_id, subject_id, o).await? {
                    crate::ontology::Fit::Swap => {
                        corrected += 1;
                        (o, Some(subject_id))
                    }
                    crate::ontology::Fit::Neither => {
                        left_off += 1;
                        continue;
                    }
                    crate::ontology::Fit::Keep | crate::ontology::Fit::Unchecked => {
                        (subject_id, Some(o))
                    }
                }
            }
            None => (subject_id, None),
        };
        let mut tx = pool.begin().await?;
        // 目标断言可能已存在（同主宾已有一条真关系）：那就并进去，别造重复。
        //
        // **宾语两侧都要比。** 字面值事实的 object_id 都是 NULL，只比它就等于
        // 把同主同谓的所有值当成同一条断言——(星云科技, founding_date, 2015) 与
        // (星云科技, founding_date, 2016) 会被并成一条，后一个值静默消失
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3
               AND object_id IS NOT DISTINCT FROM $4
               AND object_value IS NOT DISTINCT FROM $5
               AND invalidated_at IS NULL",
        )
        .bind(kb_id)
        .bind(subject_id)
        .bind(predicate_id)
        .bind(object_id)
        .bind(&object_value)
        .fetch_optional(&mut *tx)
        .await?;

        let (new_id, mode) = match existing {
            Some((id,)) => (id, ADOPT_MERGED),
            None => {
                let id = Uuid::now_v7();
                // 宾语显式绑定而不是从旧行复制：属性那一路的新值是归一化过的
                //（"2015" → 日期），照抄旧行就等于把没换算的原值塞进去。
                // 关系那一路绑的就是旧行的值，行为一字不变
                let inserted: Option<(Uuid,)> = sqlx::query_as(
                    "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                                        valid_from, valid_from_precision,
                                        valid_to, valid_to_precision, confidence, supersedes)
                     SELECT $1, kb_id, $6, $3, $4, $5,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence, id
                     FROM facts WHERE id = $2 AND invalidated_at IS NULL
                     RETURNING id",
                )
                .bind(id)
                .bind(old_id)
                .bind(predicate_id)
                .bind(object_id)
                .bind(&object_value)
                // 主语也显式绑定，不再从旧行复制——被动形改写要的正是换掉它。
                // 第一版漏了这一处：局部变量换了、SQL 里还写着 subject_id，
                // 于是宾语换了、主语没换，凭空造出一条 `OpenAI produces OpenAI`
                .bind(subject_id)
                .fetch_optional(&mut *tx)
                .await?;
                // 已被并发改写：不重复动手
                let Some((id,)) = inserted else {
                    tx.rollback().await?;
                    continue;
                };
                (id, ADOPT_SUPERSEDED)
            }
        };

        // 证据整体搬过去，表层谓词一并保留——它是这次改写的依据，不该在改写中丢失
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version)
             SELECT $1, chunk_id, quote, proposed_predicate, document_id, doc_version
             FROM fact_evidence WHERE fact_id = $2
             ON CONFLICT DO NOTHING",
        )
        .bind(new_id)
        .bind(old_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO fact_adoptions
                (batch_id, kb_id, predicate_id, old_fact_id, new_fact_id, mode)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(batch_id)
        .bind(kb_id)
        .bind(predicate_id)
        .bind(old_id)
        .bind(new_id)
        .bind(mode)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        moved += 1;
    }
    Ok(Adopted {
        batch_id,
        moved,
        left_off,
        corrected,
    })
}

/// 撤销一次采纳：新写的行作废、旧行复活。
///
/// 关系类型**不删**——已有事实指向过它（`delete_relation_type` 也会拒绝），
/// 而按 append-only 的规矩"它存在过"本身是历史；一个没人用的关系是惰性的。
/// 证据也不清：新行已作废，其证据随之惰性，删掉反而抹掉"我们曾经这么认为"。
///
/// 并入那种（mode = merged）只复活旧行，不动被并入的目标——它本来就在，
/// 复制过去的证据留着无害（`ON CONFLICT DO NOTHING` 本就可能是它自己的）。
pub async fn unadopt(pool: &PgPool, kb_id: Uuid, batch_id: Uuid) -> AppResult<u32> {
    let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT old_fact_id, new_fact_id, mode FROM fact_adoptions
         WHERE batch_id = $1 AND kb_id = $2 AND reverted_at IS NULL",
    )
    .bind(batch_id)
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(AppError::NotFound);
    }

    let mut tx = pool.begin().await?;
    let mut reverted = 0u32;
    for (old_id, new_id, mode) in &rows {
        if mode == ADOPT_SUPERSEDED {
            sqlx::query(
                "UPDATE facts SET invalidated_at = now() WHERE id = $1 AND invalidated_at IS NULL",
            )
            .bind(new_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("UPDATE facts SET invalidated_at = NULL WHERE id = $1")
            .bind(old_id)
            .execute(&mut *tx)
            .await?;
        reverted += 1;
    }
    // 标记而不是删除：这次采纳发生过，撤销也发生过，两件都是历史
    sqlx::query(
        "UPDATE fact_adoptions SET reverted_at = now()
         WHERE batch_id = $1 AND kb_id = $2 AND reverted_at IS NULL",
    )
    .bind(batch_id)
    .bind(kb_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(reverted)
}

/// 词表外的**字面值**说法：还没有谓词、宾语是值而不是实体的那些。
///
/// 跟 [`proposed_predicates`] 互补，两边用 `object_id` 是否为空严格分开。
/// 混在一起提案就会把 `founding_date` 提成一条关系，而那正是这条路要修掉的。
pub async fn proposed_attributes(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<Vec<utopia_core::models::ProposedAttribute>> {
    Ok(sqlx::query_as(
        // 同 proposed_predicates：普遍程度从全量证据数，改写量从积压数；
        // 走 CTE 而不是相关子查询，后者每组重扫一遍证据表
        "WITH spread AS (
             SELECT e.proposed_predicate AS form,
                    count(DISTINCT e.document_id) AS doc_count
             FROM fact_evidence e
             JOIN facts ff ON ff.id = e.fact_id
             WHERE ff.kb_id = $1 AND e.proposed_predicate IS NOT NULL
             GROUP BY 1
         )
         SELECT fe.proposed_predicate AS form,
                count(DISTINCT f.id) AS fact_count,
                max(sp.doc_count) AS doc_count,
                (SELECT f2.object_value::text
                 FROM fact_evidence e2
                 JOIN facts f2 ON f2.id = e2.fact_id
                 WHERE e2.proposed_predicate = fe.proposed_predicate
                   AND f2.kb_id = $1 AND f2.predicate_id IS NULL
                   AND f2.object_id IS NULL AND f2.invalidated_at IS NULL
                 LIMIT 1) AS example,
                -- 主语实际是什么类：属性的 domain 从这里来，不靠猜
                ARRAY(SELECT DISTINCT t.key
                      FROM fact_evidence e3
                      JOIN facts f3 ON f3.id = e3.fact_id
                      JOIN entities s ON s.id = f3.subject_id
                      JOIN entity_types t ON t.id = s.type_id
                      WHERE e3.proposed_predicate = fe.proposed_predicate
                        AND f3.kb_id = $1 AND f3.predicate_id IS NULL
                        AND f3.object_id IS NULL AND f3.invalidated_at IS NULL) AS domain_keys
         FROM fact_evidence fe
         JOIN facts f ON f.id = fe.fact_id
         JOIN spread sp ON sp.form = fe.proposed_predicate
         WHERE f.kb_id = $1 AND f.predicate_id IS NULL
           AND f.invalidated_at IS NULL AND fe.proposed_predicate IS NOT NULL
           AND f.object_id IS NULL
           -- 拒绝过的说法不再出现在候选里
           AND NOT EXISTS (SELECT 1 FROM ontology_misses m
                           WHERE m.kb_id = $1 AND m.kind = 'attribute_type'
                             AND m.key = fe.proposed_predicate AND m.dismissed_at IS NOT NULL)
         GROUP BY fe.proposed_predicate
         ORDER BY fact_count DESC, form",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 某几个字面值说法当前挂着的事实：id、主语的类型、原始值。
///
/// 给采纳那一步用。**归一化不在这里做**——它按 datatype 把 "2015" 变成日期、
/// 把 "1,200" 变成数字，那套规则住在抽取模块，store 够不着也不该够得着。
/// 调用方归一化完，把结果原样交回来。
pub async fn value_facts_for_forms(
    pool: &PgPool,
    kb_id: Uuid,
    forms: &[String],
) -> AppResult<Vec<(Uuid, Uuid, serde_json::Value)>> {
    if forms.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT DISTINCT f.id, s.type_id, f.object_value
         FROM facts f
         JOIN entities s ON s.id = f.subject_id
         WHERE f.kb_id = $1 AND f.predicate_id IS NULL AND f.invalidated_at IS NULL
           AND f.object_id IS NULL AND f.object_value IS NOT NULL
           AND EXISTS (SELECT 1 FROM fact_evidence e
                       WHERE e.fact_id = f.id AND e.proposed_predicate = ANY($2))",
    )
    .bind(kb_id)
    .bind(forms)
    .fetch_all(pool)
    .await?)
}

/// 采纳一批**字面值**说法：把它们的事实改挂到某个属性上。
///
/// 与 [`adopt_proposed_predicates`] 共用改写、批次与撤销——对图做的事是同一件，
/// 只有"新宾语从哪来"不同：这里的值由调用方按属性的 datatype 归一化过，
/// 换算不出来的那些根本不会传进来（它们继续没有谓词，等下一次）。
pub async fn adopt_value_facts(
    pool: &PgPool,
    kb_id: Uuid,
    attribute_id: Uuid,
    rewrites: &[(Uuid, serde_json::Value)],
) -> AppResult<Adopted> {
    // 属性那一路没有对调可言：宾语是字面值，值不能当主语
    adopt(
        pool,
        kb_id,
        attribute_id,
        AdoptTargets::WithValues(rewrites),
        false,
    )
    .await
}
