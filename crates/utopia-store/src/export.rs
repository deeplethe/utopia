//! 导出用的读取面（0020）。
//!
//! 与界面那几个视图分开写，因为问的东西不一样：界面要「现在是什么样」，
//! 导出要**全部**——闭合的区间、撤回的行、修正链，一条都不能少，否则导出的
//! 是一张干净自信的图，而那正是审计要看的东西被抹掉的样子。
//!
//! 除本体外一律**按 id 分页**：一个库的事实可以有几十万条，全读进内存再序列化
//! 会在最需要它的那种部署上炸掉。id 是 uuid v7，按它排序即按写入顺序排序。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

/// 一次取多少行。够大以免把往返次数拉满，够小以免一页就撑爆内存。
pub const PAGE: i64 = 500;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportClass {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub description: String,
    /// 导入来的类留着它原来的 IRI——schema.org 的 Organization 导出去还是
    /// `schema:Organization`，读的人手里的词汇表对得上
    pub iri: Option<String>,
    pub parents: Vec<Uuid>,
    pub disjoint: Vec<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportRelation {
    pub id: Uuid,
    pub key: String,
    pub label: String,
    pub description: String,
    pub iri: Option<String>,
    /// relation | attribute（后者值域是字面值）
    pub kind: String,
    pub datatype: Option<String>,
    pub unit: Option<String>,
    pub temporal: String,
    pub functional: bool,
    pub inverse_functional: bool,
    pub is_transitive: bool,
    pub is_symmetric: bool,
    pub is_asymmetric: bool,
    pub is_irreflexive: bool,
    pub domains: Vec<Uuid>,
    pub ranges: Vec<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportEntity {
    pub id: Uuid,
    pub canonical_name: String,
    pub type_id: Option<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportFact {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub predicate_id: Option<Uuid>,
    /// 本体没接住这条关系时，模型的原话（0010）。导出去是为了让读的人看见
    /// 「系统当时听见的是这个词，而词汇表里没有」
    pub surface_predicate: Option<String>,
    pub object_id: Option<Uuid>,
    pub object_value: Option<serde_json::Value>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_from_precision: Option<String>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_to_precision: Option<String>,
    /// 读出来的区间（0022）：「现在仍成立」那条三元组按它判，不再自己解释 NULL
    pub holds_from: Option<DateTime<Utc>>,
    pub holds_to: Option<DateTime<Utc>>,
    pub recorded_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub supersedes: Option<Uuid>,
    pub documents: Vec<Uuid>,
    pub quotes: Vec<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportDerived {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub predicate_id: Uuid,
    /// 字面值结论（业务规则的归类与属性）没有实体宾语（0021）
    pub object_id: Option<Uuid>,
    pub object_value: Option<serde_json::Value>,
    /// 公理规则。业务规则推的为 None——它的身份在 attribute_rule_id 上
    pub rule_id: Option<Uuid>,
    pub attribute_rule_id: Option<Uuid>,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_from_precision: Option<String>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_to_precision: Option<String>,
    pub derived_at: DateTime<Utc>,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub confidence: f32,
    /// transitive | symmetric | inverse | sub_property，或 business
    pub rule: String,
    /// 业务规则的名字，进 RDF 当这条推理活动的标签
    pub rule_name: Option<String>,
    /// 前提事实。审计要顺着它往下走到句子
    pub premises: Vec<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportDocument {
    pub id: Uuid,
    pub filename: String,
    pub external_key: Option<String>,
    pub doc_time: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    /// 删过的文档留着墓碑（#268）。导出里它仍在，只是记录轴上已经结束
    pub deleted_at: Option<DateTime<Utc>>,
}

pub async fn classes(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ExportClass>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.key, t.label, t.description, t.iri,
                COALESCE(ARRAY(SELECT p.parent_id FROM entity_type_parents p
                                WHERE p.child_id = t.id ORDER BY p.parent_id), '{}') AS parents,
                COALESCE(ARRAY(SELECT CASE WHEN d.a_id = t.id THEN d.b_id ELSE d.a_id END
                                 FROM entity_type_disjoint d
                                WHERE d.kb_id = $1 AND (d.a_id = t.id OR d.b_id = t.id)
                                ORDER BY 1), '{}') AS disjoint
           FROM entity_types t WHERE t.kb_id = $1 ORDER BY t.key",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

pub async fn relations(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ExportRelation>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.key, r.label, r.description, r.iri, r.kind, r.datatype, r.unit,
                r.temporal, r.functional, r.inverse_functional,
                r.is_transitive, r.is_symmetric, r.is_asymmetric, r.is_irreflexive,
                COALESCE(ARRAY(SELECT d.entity_type_id FROM relation_type_domains d
                                WHERE d.relation_type_id = r.id ORDER BY 1), '{}') AS domains,
                COALESCE(ARRAY(SELECT g.entity_type_id FROM relation_type_ranges g
                                WHERE g.relation_type_id = r.id ORDER BY 1), '{}') AS ranges
           FROM relation_types r WHERE r.kb_id = $1 ORDER BY r.key",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 合并掉的实体不导出：它已经不是一个东西了，它的事实早已搬到留下的那个身上。
pub async fn entities_page(
    pool: &PgPool,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportEntity>> {
    Ok(sqlx::query_as(
        "SELECT id, canonical_name, type_id FROM entities
          WHERE kb_id = $1 AND merged_into IS NULL AND id > COALESCE($2, '00000000-0000-0000-0000-000000000000'::uuid)
          ORDER BY id LIMIT $3",
    )
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(pool)
    .await?)
}

/// **不过滤 `invalidated_at`。** 撤回的、被修正顶掉的、区间早已闭合的，全在里面
/// ——它们各自带着两根轴上的时刻，读的人自己判断当时成立不成立（0019、0020）。
pub async fn facts_page(
    pool: &PgPool,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportFact>> {
    Ok(sqlx::query_as(&format!(
        "SELECT f.id, f.subject_id, f.predicate_id,
                fact_surface_predicate(f.id) AS surface_predicate,
                f.object_id, f.object_value,
                f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                {holds_from} AS holds_from, {holds_to} AS holds_to,
                f.recorded_at, f.invalidated_at, f.confidence, f.supersedes,
                COALESCE(ARRAY(SELECT DISTINCT e.document_id FROM fact_evidence e
                                WHERE e.fact_id = f.id AND e.document_id IS NOT NULL), '{{}}')
                  AS documents,
                COALESCE(ARRAY(SELECT e.quote FROM fact_evidence e
                                WHERE e.fact_id = f.id AND e.quote IS NOT NULL
                                ORDER BY e.chunk_id), '{{}}') AS quotes
           FROM facts f
          WHERE f.kb_id = $1 AND f.id > COALESCE($2, '00000000-0000-0000-0000-000000000000'::uuid)
          ORDER BY f.id LIMIT $3",
        holds_from = crate::world_axis::facts_holds_from("f"),
        holds_to = crate::world_axis::facts_holds_to("f"),
    ))
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(pool)
    .await?)
}

pub async fn derived_page(
    pool: &PgPool,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportDerived>> {
    Ok(sqlx::query_as(
        // **两个 LEFT JOIN。** 表拓宽之后（0021）派生可能没有实体宾语、
        // 也可能来自业务规则而不是公理——内连接会把这类结论整条挡在导出之外，
        // 而 0020 承诺的正是「审计员不靠我们也能读全」
        "SELECT d.id, d.subject_id, d.predicate_id, d.object_id, d.object_value,
                d.rule_id, d.attribute_rule_id,
                d.valid_from, d.valid_from_precision, d.valid_to, d.valid_to_precision,
                d.derived_at, d.invalidated_at, d.confidence,
                COALESCE(ru.kind, 'business') AS rule, ar.name AS rule_name,
                COALESCE(ARRAY(SELECT fd.premise_fact_id FROM fact_derivations fd
                                WHERE fd.derived_fact_id = d.id ORDER BY fd.seq), '{}') AS premises
           FROM derived_facts d
           LEFT JOIN rules ru ON ru.id = d.rule_id
           LEFT JOIN attribute_rules ar ON ar.id = d.attribute_rule_id
          WHERE d.kb_id = $1 AND d.id > COALESCE($2, '00000000-0000-0000-0000-000000000000'::uuid)
          ORDER BY d.id LIMIT $3",
    )
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(pool)
    .await?)
}

pub async fn documents_page(
    pool: &PgPool,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportDocument>> {
    Ok(sqlx::query_as(
        "SELECT id, filename, external_key, doc_time, created_at, deleted_at
           FROM documents
          WHERE kb_id = $1 AND id > COALESCE($2, '00000000-0000-0000-0000-000000000000'::uuid)
          ORDER BY id LIMIT $3",
    )
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(pool)
    .await?)
}
