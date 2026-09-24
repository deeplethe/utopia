//! 导出用的读取面（0020）。
//!
//! 与界面那几个视图分开写，因为问的东西不一样：界面要「现在是什么样」，
//! 导出要**全部**——闭合的区间、撤回的行、修正链，一条都不能少，否则导出的
//! 是一张干净自信的图，而那正是审计要看的东西被抹掉的样子。
//!
//! 除本体外一律**按 id 分页**：一个库的事实可以有几十万条，全读进内存再序列化
//! 会在最需要它的那种部署上炸掉。id 是 uuid v7，按它排序即按写入顺序排序。
//! 第一页没有下界（`$2 IS NULL OR id > $2`）：uuid 没有更小的哨兵可垫——
//! `id > NIL` 会把主键恰为 NIL 的合法行永远挡在导出外，而指向它的
//! (document_id, version) 定位器照样解析，留下一条没有本体的边。
//!
//! **同一份快照**：所有取数口吃调用方的事务（`&mut Transaction`），路由侧把它
//! 定成只读 REPEATABLE READ——体检、词汇表与每一页读的是同一个时刻的库。
//! 若每页各自向连接池要一条连接，词汇表发完之后才提交的规则会在派生页里
//! 留下一条指向它的 `wasGeneratedBy`——图里就出现没有本体的引用。
//!
//! **逐页校验用的是留下的那几行自己**：每个 page 查询把被引行的
//! kb_id 与行本体**原子地一并选出**，校验在内存里跑。不能「先取一页、再去库里
//! 问一次」——第二次问的是另一个时刻的状态，留下的行早已不是它。
//!
//! 体检范围覆盖 **0070 保护的每一条结构引用边**——不只这份导出真正解析的
//! 那些。导出还没读的边（规则表自身、陈述属性、时间提及等）也在同一个快照里
//! 先查：catalog 守卫（migration_0070_runs_under_any_search_path.rs）核对
//! export_provenance_integrity.sql 的每条扫描分支都对应一条受保护的
//! catalog 边，将来新加的受保护引用漏登记体检就直接红。

use chrono::{DateTime, Utc};
use sqlx::{Postgres, Transaction};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 一次取多少行。够大以免把往返次数拉满，够小以免一页就撑爆内存。
pub const PAGE: i64 = 500;

/// 出处体检的唯一一份 SQL：运行时就跑它，catalog 守卫读的也是它——
/// 边集合只有这一处登记，没有第二份要手工对齐的清单
const PREFLIGHT_SQL: &str = include_str!("export_provenance_integrity.sql");

/// 出处链越界的一类引用。同库外键只认 id、不认库：A 库的
/// 行可以引用 B 库的对象，schema 什么都不拦。0070 的触发器挡新行；这里拦的是
/// **存量坏行**与绕过触发器写进来的行。
///
/// 处置一律是**整份拒导**：把别库对象的 id 铸进本库 IRI
/// （`urn:utopia:kb:A:document:{B 的文档}`）等于伪造身份——那份文件看着
/// 完整，实则悬空。被词汇表解析的引用（谓词、属性类型、实体类型）越库则
/// 静默消失——坏行一样不许放行。少导一行、换个 IRI 都不在选项里
#[derive(Debug, sqlx::FromRow)]
pub struct CrossKbViolation {
    /// 哪条边：evidence.chunk | derivation.premise_fact | …
    pub edge: String,
    pub rows: i64,
}

fn cross_kb_error(violations: &[CrossKbViolation]) -> AppError {
    let detail = violations
        .iter()
        .map(|v| format!("{}: {} row(s)", v.edge, v.rows))
        .collect::<Vec<_>>()
        .join("; ");
    AppError::invalid_detail(
        "cross_kb_provenance",
        "export refused: KB-scoped provenance points outside the knowledge base",
        detail,
    )
}

/// 引用指着的东西**在本库，却不在导出集里**（合并掉的实体是唯一会缺席的
/// 实体——`entities_page` 滤掉 `merged_into` 非空的行）。同库但缺席的引用
/// 不能换 IRI、也不能静默省略：整份拒导，与越库同一处置。
fn unexported_error(violations: &[CrossKbViolation]) -> AppError {
    let detail = violations
        .iter()
        .map(|v| format!("{}: {} row(s)", v.edge, v.rows))
        .collect::<Vec<_>>()
        .join("; ");
    AppError::invalid_detail(
        "unexported_target",
        "export refused: a reference points at a row that is not in this KB's exported set",
        detail,
    )
}

fn tally(violations: &mut Vec<CrossKbViolation>, edge: &str, rows: i64) {
    if rows > 0 {
        violations.push(CrossKbViolation {
            edge: edge.into(),
            rows,
        });
    }
}

/// 引用列的判定：**留下的是谁，就查谁**。`ref_kb` 由 page 查询与行本体原子地
/// 一并选出——别库、悬空（NULL）都不算本库，一律判违规
fn foreign(ref_kb: Option<Uuid>, kb_id: Uuid) -> bool {
    ref_kb != Some(kb_id)
}

/// 导出前的出处体检。逐类数一遍越界引用，有一行就整份拒导。
/// 越界有两类，各自一条错：**别库/悬空**（`cross_kb`）与**同库但不在导出集**
/// （`unexported`——合并掉的实体）。只报哪条边坏了、坏了几行——具体哪些行
/// 坏是库里的事，不进面向导出的报错
///
/// 扫的边 = **0070 保护的全部结构引用边**（export_provenance_integrity.sql，
/// 每条分支带一条 `@edge`/`@filter` 标记给 catalog 守卫核对）——连导出尚未
/// 序列化的边也算：一份账本上任何一条受保护的同库引用断了，这份导出都不可信，
/// 宁可整份拒。**必须在导出用的那条事务里跑**（REPEATABLE READ）：
/// 体检与每一页查询看的是同一个快照，先体检后换连接会在两个时刻之间
/// 漏掉刚提交的坏行
pub async fn provenance_integrity(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
) -> AppResult<()> {
    #[derive(sqlx::FromRow)]
    struct ScanViolation {
        edge: String,
        kind: String,
        rows: i64,
    }
    let violations: Vec<ScanViolation> = sqlx::query_as(PREFLIGHT_SQL)
        .bind(kb_id)
        .fetch_all(&mut **tx)
        .await?;
    let cross_kb: Vec<CrossKbViolation> = violations
        .iter()
        .filter(|v| v.kind == "cross_kb")
        .map(|v| CrossKbViolation {
            edge: v.edge.clone(),
            rows: v.rows,
        })
        .collect();
    let unexported: Vec<CrossKbViolation> = violations
        .iter()
        .filter(|v| v.kind == "unexported")
        .map(|v| CrossKbViolation {
            edge: v.edge.clone(),
            rows: v.rows,
        })
        .collect();
    if !cross_kb.is_empty() {
        return Err(cross_kb_error(&cross_kb));
    }
    if !unexported.is_empty() {
        return Err(unexported_error(&unexported));
    }
    Ok(())
}

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
    /// 同表自指：导出 owl:inverseOf / rdfs:subPropertyOf。存量的合法性在
    /// 迁移的递延约束里管，这里只负责把集合带上（越库/悬空 → 拒导）
    pub inverse_of: Option<Uuid>,
    pub sub_property_of: Option<Uuid>,
    pub domains: Vec<Uuid>,
    pub ranges: Vec<Uuid>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExportEntity {
    pub id: Uuid,
    pub canonical_name: String,
    pub type_id: Option<Uuid>,
    /// type_id 指着的类的 kb（LEFT JOIN 一并选出）。别库/悬空 → 拒导：
    /// 序列化按 id 进本库词汇表查类，查不着就是静默丢类型
    pub type_kb: Option<Uuid>,
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
    /// 边上的属性（0037），加载后按事实 id 补
    #[sqlx(skip)]
    pub qualifiers: Vec<utopia_core::models::FactQualifier>,
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
    /// 规则算出来的隐含行（0044 决定 3 第五片）：读的人要能分辨它不是陈述直接说的
    pub implied: bool,
    pub supersedes: Option<Uuid>,
    pub documents: Vec<Uuid>,
    pub quotes: Vec<String>,
    /// 证据文字的来源（0040），去重：一条陈述的引文里有没有扫描、转写、看图描述来的
    pub quote_origins: Vec<String>,
    /// 以下各列是被引行的 kb，与行本体原子地一并选出。
    /// 别库或悬空（NULL）的引用不许被铸成本库 IRI，也不许静默跳过
    pub subject_kb: Option<Uuid>,
    pub object_kb: Option<Uuid>,
    pub predicate_kb: Option<Uuid>,
    pub supersedes_kb: Option<Uuid>,
    /// documents[] 里是否有别库或悬空的文档指针
    pub foreign_document: bool,
    /// 引文 JOIN 的段落里是否有别库或悬空的（quote_origins 的来源会静默消失）
    pub foreign_chunk: bool,
    /// 主语/宾语指着**已合并**的实体：同库但不在导出集（merged_into IS NOT NULL
    /// 的行 entities_page 不导）。与行本体原子地一并选出
    pub subject_merged: bool,
    pub object_merged: bool,
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
    /// 前提里是**另一条派生**的那些（0030）。与上面那列分开，是因为读回来的人
    /// 要知道该去哪张表接着往下走；合成一列的话，一条链在导出里就断了
    pub premises_derived: Vec<Uuid>,
    /// 被引行的 kb，与行本体原子地一并选出
    pub subject_kb: Option<Uuid>,
    pub object_kb: Option<Uuid>,
    pub predicate_kb: Option<Uuid>,
    /// rule_id / attribute_rule_id 指着的规则行的 kb——它铸成 wasGeneratedBy
    /// 的 Activity IRI
    pub rule_kb: Option<Uuid>,
    pub attribute_rule_kb: Option<Uuid>,
    /// premises[] / premises_derived[] 里是否有别库或悬空的前提（两种前提分开报边）
    pub foreign_fact_premise: bool,
    pub foreign_derived_premise: bool,
    /// 主语/宾语指着已合并的实体：同库但不在导出集
    pub subject_merged: bool,
    pub object_merged: bool,
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

pub async fn classes(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
) -> AppResult<Vec<ExportClass>> {
    let classes: Vec<ExportClass> = sqlx::query_as(
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
    .fetch_all(&mut **tx)
    .await?;
    // 父类与互斥类稍后要进本库词汇表按 id 查——查不着就是被静默丢掉。
    // 能解析的就地解析：词汇表全集就在手里，不在集合里的引用就是越库/悬空
    let own: std::collections::HashSet<Uuid> = classes.iter().map(|c| c.id).collect();
    let mut violations = Vec::new();
    for c in &classes {
        let bad_parents = c.parents.iter().filter(|p| !own.contains(p)).count() as i64;
        let bad_disjoint = c.disjoint.iter().filter(|p| !own.contains(p)).count() as i64;
        tally(&mut violations, "class.parent", bad_parents);
        tally(&mut violations, "class.disjoint", bad_disjoint);
    }
    if !violations.is_empty() {
        return Err(cross_kb_error(&violations));
    }
    Ok(classes)
}

pub async fn relations(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
) -> AppResult<Vec<ExportRelation>> {
    let relations: Vec<ExportRelation> = sqlx::query_as(
        "SELECT r.id, r.key, r.label, r.description, r.iri, r.kind, r.datatype, r.unit,
                r.temporal, r.functional, r.inverse_functional,
                r.is_transitive, r.is_symmetric, r.is_asymmetric, r.is_irreflexive,
                r.inverse_of, r.sub_property_of,
                COALESCE(ARRAY(SELECT d.entity_type_id FROM relation_type_domains d
                                WHERE d.relation_type_id = r.id ORDER BY 1), '{}') AS domains,
                COALESCE(ARRAY(SELECT g.entity_type_id FROM relation_type_ranges g
                                WHERE g.relation_type_id = r.id ORDER BY 1), '{}') AS ranges
           FROM relation_types r WHERE r.kb_id = $1 ORDER BY r.key",
    )
    .bind(kb_id)
    .fetch_all(&mut **tx)
    .await?;
    // domain/range 的类 id 与 inverse/sub_property 的关系 id 都要进本库词汇表
    // 查——查不着就静默丢公理。**两行全集都在手里**，不在集合里的引用就是
    // 越库/悬空
    let own_types: Vec<Uuid> = sqlx::query_scalar(
        "SELECT COALESCE(ARRAY(SELECT id FROM entity_types WHERE kb_id = $1), '{}')",
    )
    .bind(kb_id)
    .fetch_one(&mut **tx)
    .await?;
    let own: std::collections::HashSet<Uuid> = own_types.into_iter().collect();
    let own_rel: std::collections::HashSet<Uuid> = relations.iter().map(|r| r.id).collect();
    let mut violations = Vec::new();
    for r in &relations {
        let bad_domains = r.domains.iter().filter(|t| !own.contains(t)).count() as i64;
        let bad_ranges = r.ranges.iter().filter(|t| !own.contains(t)).count() as i64;
        tally(&mut violations, "relation.domain", bad_domains);
        tally(&mut violations, "relation.range", bad_ranges);
        if let Some(t) = r.inverse_of {
            tally(
                &mut violations,
                "relation.inverse",
                (!own_rel.contains(&t)) as i64,
            );
        }
        if let Some(t) = r.sub_property_of {
            tally(
                &mut violations,
                "relation.sub_property",
                (!own_rel.contains(&t)) as i64,
            );
        }
    }
    if !violations.is_empty() {
        return Err(cross_kb_error(&violations));
    }
    Ok(relations)
}

/// 合并掉的实体不导出：它已经不是一个东西了，它的事实早已搬到留下的那个身上。
pub async fn entities_page(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportEntity>> {
    let page: Vec<ExportEntity> = sqlx::query_as(
        "SELECT e.id, e.canonical_name, e.type_id, t.kb_id AS type_kb
           FROM entities e LEFT JOIN entity_types t ON t.id = e.type_id
          WHERE e.kb_id = $1 AND e.merged_into IS NULL
            AND ($2 IS NULL OR e.id > $2)
          ORDER BY e.id LIMIT $3",
    )
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(&mut **tx)
    .await?;
    let mut violations = Vec::new();
    for e in &page {
        if e.type_id.is_some() && foreign(e.type_kb, kb_id) {
            tally(&mut violations, "entity.type", 1);
        }
    }
    if !violations.is_empty() {
        return Err(cross_kb_error(&violations));
    }
    Ok(page)
}

/// **不过滤 `invalidated_at`。** 撤回的、被修正顶掉的、区间早已闭合的，全在里面
/// ——它们各自带着两根轴上的时刻，读的人自己判断当时成立不成立（0019、0020）。
pub async fn facts_page(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportFact>> {
    let mut facts: Vec<ExportFact> = sqlx::query_as(&format!(
        "SELECT f.id, f.subject_id, f.predicate_id,
                fact_surface_predicate(f.id) AS surface_predicate,
                f.object_id, f.object_value,
                f.valid_from, f.valid_from_precision, f.valid_to, f.valid_to_precision,
                {holds_from} AS holds_from, {holds_to} AS holds_to,
                f.recorded_at, f.invalidated_at, f.confidence, f.implied, f.supersedes,
                COALESCE(ARRAY(SELECT DISTINCT e.document_id FROM fact_evidence e
                                WHERE e.fact_id = f.id AND e.document_id IS NOT NULL), '{{}}')
                  AS documents,
                COALESCE(ARRAY(SELECT e.quote FROM fact_evidence e
                                WHERE e.fact_id = f.id AND e.quote IS NOT NULL
                                ORDER BY e.chunk_id), '{{}}') AS quotes,
                COALESCE(ARRAY(SELECT DISTINCT c.origin FROM fact_evidence e
                                JOIN chunks c ON c.id = e.chunk_id
                                WHERE e.fact_id = f.id ORDER BY c.origin), '{{}}')
                  AS quote_origins,
                s.kb_id AS subject_kb, o.kb_id AS object_kb,
                p.kb_id AS predicate_kb, sp.kb_id AS supersedes_kb,
                EXISTS(SELECT 1 FROM fact_evidence e
                        LEFT JOIN documents ed ON ed.id = e.document_id
                        WHERE e.fact_id = f.id AND e.document_id IS NOT NULL
                          AND ed.kb_id IS DISTINCT FROM f.kb_id) AS foreign_document,
                EXISTS(SELECT 1 FROM fact_evidence e
                        LEFT JOIN chunks ec ON ec.id = e.chunk_id
                        WHERE e.fact_id = f.id
                          AND ec.kb_id IS DISTINCT FROM f.kb_id) AS foreign_chunk,
                (s.merged_into IS NOT NULL) AS subject_merged,
                (o.merged_into IS NOT NULL) AS object_merged
           FROM facts f
           LEFT JOIN entities s ON s.id = f.subject_id
           LEFT JOIN entities o ON o.id = f.object_id
           LEFT JOIN relation_types p ON p.id = f.predicate_id
           LEFT JOIN facts sp ON sp.id = f.supersedes
          WHERE f.kb_id = $1 AND ($2 IS NULL OR f.id > $2)
          ORDER BY f.id LIMIT $3",
        holds_from = crate::world_axis::facts_holds_from("f"),
        holds_to = crate::world_axis::facts_holds_to("f"),
    ))
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(&mut **tx)
    .await?;
    // 留下的行逐个过：被铸成本库 IRI 的引用不许越库，进词汇表的引用不许查空。
    // 检查用的是随行原子选出的 ref_kb——不是再去库里问一次的另一个时刻
    let mut violations = Vec::new();
    let mut unexported = Vec::new();
    for f in &facts {
        tally(
            &mut violations,
            "fact.subject",
            foreign(f.subject_kb, kb_id) as i64,
        );
        tally(
            &mut unexported,
            "fact.subject(merged)",
            f.subject_merged as i64,
        );
        if f.object_id.is_some() {
            tally(
                &mut violations,
                "fact.object",
                foreign(f.object_kb, kb_id) as i64,
            );
            tally(
                &mut unexported,
                "fact.object(merged)",
                f.object_merged as i64,
            );
        }
        if f.predicate_id.is_some() {
            tally(
                &mut violations,
                "fact.predicate",
                foreign(f.predicate_kb, kb_id) as i64,
            );
        }
        if f.supersedes.is_some() {
            tally(
                &mut violations,
                "fact.supersedes",
                foreign(f.supersedes_kb, kb_id) as i64,
            );
        }
        tally(
            &mut violations,
            "evidence.document",
            f.foreign_document as i64,
        );
        tally(&mut violations, "evidence.chunk", f.foreign_chunk as i64);
    }
    if !violations.is_empty() {
        return Err(cross_kb_error(&violations));
    }
    if !unexported.is_empty() {
        return Err(unexported_error(&unexported));
    }
    // 边上的属性另一张表（0037），按事实 id 一次取回补上——把属性类型的 kb 与
    // 实体值的 kb 一并选出：别库类型会被词汇表静默跳过，别库实体会被铸进本库
    // IRI，两种行都得在序列化之前拦下来
    {
        let ids: Vec<Uuid> = facts.iter().map(|f| f.id).collect();
        let mut by_fact = qualifiers_for_export(tx, &ids, kb_id).await?;
        for f in facts.iter_mut() {
            if let Some(q) = by_fact.remove(&f.id) {
                f.qualifiers = q;
            }
        }
    }
    Ok(facts)
}

#[derive(sqlx::FromRow)]
struct ExportQualifierRow {
    fact_id: Uuid,
    qualifier_type_id: Uuid,
    key: Option<String>,
    label: Option<String>,
    value: Option<serde_json::Value>,
    entity_id: Option<Uuid>,
    entity_name: Option<String>,
    type_kb: Option<Uuid>,
    entity_kb: Option<Uuid>,
    entity_merged: bool,
}

/// 导出专用的属性取数：与 graph::fact_qualifiers_for 同一形状，多选两列 kb。
/// 那边用 INNER JOIN——被引类型不在时整行属性**静默消失**；导出不能这么吞。
/// 别库的属性类型会被词汇表查空而静默跳过，别库的实体值会被铸进本库
/// entity IRI——两种都按坏行拒
async fn qualifiers_for_export(
    tx: &mut Transaction<'_, Postgres>,
    fact_ids: &[Uuid],
    kb_id: Uuid,
) -> AppResult<std::collections::HashMap<Uuid, Vec<utopia_core::models::FactQualifier>>> {
    let mut out: std::collections::HashMap<Uuid, Vec<utopia_core::models::FactQualifier>> =
        std::collections::HashMap::new();
    if fact_ids.is_empty() {
        return Ok(out);
    }
    let rows: Vec<ExportQualifierRow> = sqlx::query_as(
        "SELECT q.fact_id, q.qualifier_type_id, r.key, r.label, q.value, q.entity_id,
                e.canonical_name AS entity_name,
                r.kb_id AS type_kb, e.kb_id AS entity_kb,
                (e.merged_into IS NOT NULL) AS entity_merged
             FROM fact_qualifiers q
             LEFT JOIN relation_types r ON r.id = q.qualifier_type_id
             LEFT JOIN entities e ON e.id = q.entity_id
             WHERE q.fact_id = ANY($1)
             ORDER BY q.fact_id, r.key",
    )
    .bind(fact_ids)
    .fetch_all(&mut **tx)
    .await?;
    let mut violations = Vec::new();
    let mut unexported = Vec::new();
    for r in &rows {
        tally(
            &mut violations,
            "qualifier.type",
            foreign(r.type_kb, kb_id) as i64,
        );
        if r.entity_id.is_some() {
            tally(
                &mut violations,
                "qualifier.entity",
                foreign(r.entity_kb, kb_id) as i64,
            );
            tally(
                &mut unexported,
                "qualifier.entity(merged)",
                r.entity_merged as i64,
            );
        }
    }
    if !violations.is_empty() {
        return Err(cross_kb_error(&violations));
    }
    if !unexported.is_empty() {
        return Err(unexported_error(&unexported));
    }
    for r in rows {
        out.entry(r.fact_id)
            .or_default()
            .push(utopia_core::models::FactQualifier {
                qualifier_type_id: r.qualifier_type_id,
                // 过了校验 r 必然在：key/label 不会取不到
                key: r.key.unwrap_or_default(),
                label: r.label.unwrap_or_default(),
                value: r.value,
                entity_id: r.entity_id,
                entity_name: r.entity_name,
            });
    }
    Ok(out)
}

pub async fn derived_page(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportDerived>> {
    let page: Vec<ExportDerived> = sqlx::query_as(
        // **两个 LEFT JOIN。** 表拓宽之后（0021）派生可能没有实体宾语、
        // 也可能来自业务规则而不是公理——内连接会把这类结论整条挡在导出之外，
        // 而 0020 承诺的正是「审计员不靠我们也能读全」
        "SELECT d.id, d.subject_id, d.predicate_id, d.object_id, d.object_value,
                d.rule_id, d.attribute_rule_id,
                d.valid_from, d.valid_from_precision, d.valid_to, d.valid_to_precision,
                d.derived_at, d.invalidated_at, d.confidence,
                COALESCE(ru.kind, 'business') AS rule, ar.name AS rule_name,
                COALESCE(ARRAY(SELECT fd.premise_fact_id FROM fact_derivations fd
                                WHERE fd.derived_fact_id = d.id
                                  AND fd.premise_fact_id IS NOT NULL
                                ORDER BY fd.seq), '{}') AS premises,
                COALESCE(ARRAY(SELECT fd.premise_derived_id FROM fact_derivations fd
                                WHERE fd.derived_fact_id = d.id
                                  AND fd.premise_derived_id IS NOT NULL
                                ORDER BY fd.seq), '{}') AS premises_derived,
                s.kb_id AS subject_kb, o.kb_id AS object_kb, p.kb_id AS predicate_kb,
                ru.kb_id AS rule_kb, ar.kb_id AS attribute_rule_kb,
                EXISTS(SELECT 1 FROM fact_derivations fd
                        LEFT JOIN facts pf ON pf.id = fd.premise_fact_id
                        WHERE fd.derived_fact_id = d.id AND fd.premise_fact_id IS NOT NULL
                          AND pf.kb_id IS DISTINCT FROM d.kb_id) AS foreign_fact_premise,
                EXISTS(SELECT 1 FROM fact_derivations fd
                        LEFT JOIN derived_facts pd ON pd.id = fd.premise_derived_id
                        WHERE fd.derived_fact_id = d.id AND fd.premise_derived_id IS NOT NULL
                          AND pd.kb_id IS DISTINCT FROM d.kb_id) AS foreign_derived_premise,
                (s.merged_into IS NOT NULL) AS subject_merged,
                (o.merged_into IS NOT NULL) AS object_merged
           FROM derived_facts d
           LEFT JOIN rules ru ON ru.id = d.rule_id
           LEFT JOIN attribute_rules ar ON ar.id = d.attribute_rule_id
           LEFT JOIN entities s ON s.id = d.subject_id
           LEFT JOIN entities o ON o.id = d.object_id
           LEFT JOIN relation_types p ON p.id = d.predicate_id
          WHERE d.kb_id = $1 AND ($2 IS NULL OR d.id > $2)
          ORDER BY d.id LIMIT $3",
    )
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(&mut **tx)
    .await?;
    let mut violations = Vec::new();
    let mut unexported = Vec::new();
    for d in &page {
        tally(
            &mut violations,
            "derived.subject",
            foreign(d.subject_kb, kb_id) as i64,
        );
        tally(
            &mut unexported,
            "derived.subject(merged)",
            d.subject_merged as i64,
        );
        if d.object_id.is_some() {
            tally(
                &mut violations,
                "derived.object",
                foreign(d.object_kb, kb_id) as i64,
            );
            tally(
                &mut unexported,
                "derived.object(merged)",
                d.object_merged as i64,
            );
        }
        // 派生的谓词非空（CHECK 保证），NULL 的 ref_kb 一样是越界/悬空
        tally(
            &mut violations,
            "derived.predicate",
            foreign(d.predicate_kb, kb_id) as i64,
        );
        if d.rule_id.is_some() {
            tally(
                &mut violations,
                "derived.rule",
                foreign(d.rule_kb, kb_id) as i64,
            );
        }
        if d.attribute_rule_id.is_some() {
            tally(
                &mut violations,
                "derived.attribute_rule",
                foreign(d.attribute_rule_kb, kb_id) as i64,
            );
        }
        tally(
            &mut violations,
            "derivation.premise_fact",
            d.foreign_fact_premise as i64,
        );
        tally(
            &mut violations,
            "derivation.premise_derived",
            d.foreign_derived_premise as i64,
        );
    }
    if !violations.is_empty() {
        return Err(cross_kb_error(&violations));
    }
    if !unexported.is_empty() {
        return Err(unexported_error(&unexported));
    }
    Ok(page)
}

pub async fn documents_page(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    after: Option<Uuid>,
) -> AppResult<Vec<ExportDocument>> {
    Ok(sqlx::query_as(
        "SELECT id, filename, external_key, doc_time, created_at, deleted_at
           FROM documents
          WHERE kb_id = $1 AND ($2 IS NULL OR id > $2)
          ORDER BY id LIMIT $3",
    )
    .bind(kb_id)
    .bind(after)
    .bind(PAGE)
    .fetch_all(&mut **tx)
    .await?)
}
