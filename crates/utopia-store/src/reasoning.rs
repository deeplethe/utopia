//! 一致性检查的取数与落库。判断本身在 `utopia-reason`——那一层不碰数据库。
//!
//! 三步：把活事实取成边、把 `relation_types` 的公理列取成 `Axioms`、把
//! `check()` 吐出来的违规写进 `axiom_violations`。
//!
//! **重跑是幂等的，而且以「重算」为准。** 每次跑完，这个库里没被重新算出来的
//! `open` 行会被删掉——它们是派生状态，事实撤了、公理放宽了，那条违规就不该
//! 还挂在 Review 页上。已经有人表态的（`resolved`）一行不动：那是人的决定，
//! 不是算出来的东西。
//!
//! 这条规矩踩过一次坑的反面（见 `ontology_proposals`）：那边重跑会把被拒绝过的
//! 提案刷回待看，等于每跑一次就把人的否决抹掉一次。所以这里 `ON CONFLICT`
//! 什么都不做——已经在库里的那一行，无论 open 还是 resolved，都按原样留着。

use sqlx::PgPool;
use std::collections::HashMap;
use utopia_core::models::{AxiomViolation, DerivedFactView, OntologyDefect};
/// 规则种类的字面量。用 &'static str 而不是枚举:它直接进 SQL 也直接做键
type RuleKind = &'static str;
use utopia_core::AppResult;
use utopia_reason::derive::TimedEdge;
use utopia_reason::{check, Axioms, Edge, Kind, Violation};
use uuid::Uuid;

/// 一次检查的产出，给调用方写审计与告诉用户。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// 参与检查的边数
    pub edges: usize,
    /// 声明了至少一条公理的谓词数。**为零时结论是「没有判据」而不是「没有矛盾」**
    pub predicates_with_axioms: usize,
    /// 这次算出来的违规总数
    pub found: usize,
    /// 其中是新的（此前没在库里）
    pub inserted: usize,
    /// 清掉的陈旧 open 行
    pub cleared: usize,
}

/// 取这个库里所有能参与检查的边。
///
/// 三个过滤条件都是必要的：
///
/// - `invalidated_at IS NULL`——被推翻的事实不该再报矛盾，它已经不是我们的断言了
/// - `predicate_id IS NOT NULL`——没有谓词就没有公理可依（见 `facts.predicate_id`）
/// - `object_id IS NOT NULL`——属性事实的宾语是字面值，公理谈的是实体之间的关系
async fn edges(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Edge>> {
    let rows: Vec<(Uuid, Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, predicate_id, subject_id, object_id
           FROM facts
          WHERE kb_id = $1
            AND invalidated_at IS NULL
            AND predicate_id IS NOT NULL
            AND object_id IS NOT NULL",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(fact, predicate, subject, object)| Edge {
            fact,
            predicate,
            subject,
            object,
        })
        .collect())
}

/// 取这个库的谓词公理。
///
/// **只取声明了至少一位的**：一位都没声明的谓词进了表也不会被检查（`says_nothing`
/// 会跳过），白白占内存。而且这个条数本身有意义——它是「有没有判据」的度量，
/// 报告里要用。
#[allow(clippy::type_complexity)]
async fn axioms(pool: &PgPool, kb_id: Uuid) -> AppResult<HashMap<Uuid, Axioms>> {
    let rows: Vec<(
        Uuid,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        Option<Uuid>,
        Option<Uuid>,
    )> = sqlx::query_as(
        "SELECT id, is_transitive, is_symmetric, is_asymmetric, is_irreflexive,
                    functional, inverse_functional, inverse_of, sub_property_of
               FROM relation_types
              WHERE kb_id = $1
                AND (is_transitive OR is_symmetric OR is_asymmetric OR is_irreflexive
                     OR functional OR inverse_functional
                     OR inverse_of IS NOT NULL OR sub_property_of IS NOT NULL)",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let mut map: HashMap<Uuid, Axioms> = rows
        .into_iter()
        .map(
            |(
                id,
                transitive,
                symmetric,
                asymmetric,
                irreflexive,
                functional,
                inverse_functional,
                inverse_of,
                sub_property_of,
            )| {
                (
                    id,
                    Axioms {
                        transitive,
                        symmetric,
                        asymmetric,
                        irreflexive,
                        functional,
                        inverse_functional,
                        inverse_of,
                        sub_property_of,
                    },
                )
            },
        )
        .collect();

    // **逆是相互的，而库里只存单向。** 声明了 `p⁻¹ = q` 却没回填
    // `q⁻¹ = p` 的话，`A p B` 推得出 `B q A`，`B q A` 却推不回 `A p B`
    // ——「问工作和问雇佣答案不同」正是 R1 要消灭的东西，只修一半等于没修。
    //
    // 归一化放在这里而不是数据库触发器：绕过触发器的路不止一条（RDF 导入、
    // 直接改表），而载入公理只有这一处，谁也绕不过去。
    let pairs: Vec<(Uuid, Uuid)> = map
        .iter()
        .filter_map(|(id, ax)| ax.inverse_of.map(|inv| (inv, *id)))
        .collect();
    for (target, source) in pairs {
        // 已经声明了自己的逆就不动它——**人写的优先于推出来的**，
        // 两边指得不一样是本体自己的矛盾，交给 R0 报，不在这里悄悄改
        map.entry(target)
            .or_default()
            .inverse_of
            .get_or_insert(source);
    }
    Ok(map)
}

/// 主语不在谓词声明的 domain 里、或宾语不在 range 里的活事实（#190 / #196）。
///
/// 这是签名检查在**账本层**的那一半：抽取与采纳在写入时按 `ontology::judge_direction`
/// 掰正或留空，但合并会换掉主语、本体会事后改 domain，写入时的守卫挡不住写入之后
/// 的改动。所以这里对着库量一遍，任何一条路写反了都在 Review 里看得见。
///
/// **没有类型的实体不算**：它没有类型可比，「不知道」不是「不符合」——按 0009，
/// 未分类是一种诚实的状态，不该因此被报成矛盾。声明了 domain / range 的谓词才查，
/// 与其它四类同一条纪律：没有公理就没有判据。
///
/// `only` 给了就只看这些事实（合并之后对搬动过的那几条立刻查）；None 是全量。
pub async fn signature_breaks(
    pool: &PgPool,
    kb_id: Uuid,
    only: Option<&[Uuid]>,
) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "WITH RECURSIVE anc(type_id, anc_id) AS (
             SELECT id, id FROM entity_types WHERE kb_id = $1
             UNION
             SELECT a.type_id, p.parent_id
               FROM anc a JOIN entity_type_parents p ON p.child_id = a.anc_id
         )
         SELECT f.id
           FROM facts f
           JOIN relation_types r ON r.id = f.predicate_id
           JOIN entities s ON s.id = f.subject_id
           JOIN entities o ON o.id = f.object_id
          WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
            AND ($2::uuid[] IS NULL OR f.id = ANY($2))
            AND (
              (s.type_id IS NOT NULL
               AND EXISTS (SELECT 1 FROM relation_type_domains d WHERE d.relation_type_id = r.id)
               AND NOT EXISTS (SELECT 1 FROM relation_type_domains d
                                 JOIN anc a ON a.anc_id = d.entity_type_id
                                WHERE d.relation_type_id = r.id AND a.type_id = s.type_id))
              OR
              (o.type_id IS NOT NULL
               AND EXISTS (SELECT 1 FROM relation_type_ranges g WHERE g.relation_type_id = r.id)
               AND NOT EXISTS (SELECT 1 FROM relation_type_ranges g
                                 JOIN anc a ON a.anc_id = g.entity_type_id
                                WHERE g.relation_type_id = r.id AND a.type_id = o.type_id))
            )
          ORDER BY f.recorded_at",
    )
    .bind(kb_id)
    .bind(only)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 把签名违规落进 `axiom_violations`（kind = `signature`，left 与 right 同一条事实）。
/// 幂等：同一条事实重复报不重复入库。返回新插入的条数。
pub async fn record_signature_breaks(
    pool: &PgPool,
    kb_id: Uuid,
    facts: &[Uuid],
) -> AppResult<usize> {
    let mut inserted = 0usize;
    for fact in facts {
        let id: Option<(Uuid,)> = sqlx::query_as(
            "INSERT INTO axiom_violations (id, kb_id, kind, left_fact, right_fact)
             VALUES ($1, $2, 'signature', $3, $3)
             ON CONFLICT (kb_id, kind, left_fact, right_fact) DO NOTHING
             RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(fact)
        .fetch_optional(pool)
        .await?;
        if id.is_some() {
            inserted += 1;
        }
    }
    Ok(inserted)
}

/// 跑一遍检查，把结果落库。
pub async fn run(pool: &PgPool, kb_id: Uuid) -> AppResult<Report> {
    let edges = edges(pool, kb_id).await?;
    let axioms = axioms(pool, kb_id).await?;
    let mut violations = check(&edges, &axioms);
    // 第五类不在纯逻辑引擎里：它要看实体的类型与谓词的 domain / range，那是库里的
    // 东西。算出来后与其它四类走同一条落库与清陈规矩
    for fact in signature_breaks(pool, kb_id, None).await? {
        violations.push(Violation {
            kind: Kind::Signature,
            left: fact,
            right: fact,
            path: Vec::new(),
        });
    }

    let mut report = Report {
        edges: edges.len(),
        predicates_with_axioms: axioms.len(),
        found: violations.len(),
        ..Default::default()
    };

    // 事务里做，否则「插新的」与「清陈旧的」之间有个窗口，那一瞬间 Review 页
    // 会短暂地少东西
    let mut tx = pool.begin().await?;
    let mut fresh: Vec<Uuid> = Vec::with_capacity(violations.len());
    for v in &violations {
        let Violation {
            kind,
            left,
            right,
            path,
        } = v;
        let id: Option<(Uuid,)> = sqlx::query_as(
            "INSERT INTO axiom_violations (id, kb_id, kind, left_fact, right_fact, path)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (kb_id, kind, left_fact, right_fact) DO NOTHING
             RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(kind.as_str())
        .bind(left)
        .bind(right)
        .bind(path)
        .fetch_optional(&mut *tx)
        .await?;
        if id.is_some() {
            report.inserted += 1;
        }
        // 无论新插的还是本来就在的，都算「这一轮仍然成立」
        let keep: (Uuid,) = sqlx::query_as(
            "SELECT id FROM axiom_violations
              WHERE kb_id = $1 AND kind = $2 AND left_fact = $3 AND right_fact = $4",
        )
        .bind(kb_id)
        .bind(kind.as_str())
        .bind(left)
        .bind(right)
        .fetch_one(&mut *tx)
        .await?;
        fresh.push(keep.0);
    }

    // 这一轮没算出来的 open 行是陈的：事实被撤了，或者公理放宽了。
    // resolved 的不动——那是人的决定，不是派生状态
    let cleared = sqlx::query(
        "DELETE FROM axiom_violations
          WHERE kb_id = $1 AND status = 'open' AND NOT (id = ANY($2))",
    )
    .bind(kb_id)
    .bind(&fresh)
    .execute(&mut *tx)
    .await?;
    report.cleared = cleared.rows_affected() as usize;
    tx.commit().await?;
    Ok(report)
}

/// Review 页要看的:还没人表态的违规,连同两条事实的三元组文本。
///
/// 展开成文本在 SQL 里做而不是回来再查一遍:一页几十条,每条两个三元组,
/// 分开查就是上百次往返。谓词用 `fact_surface_predicate` 兜底——本体里没有
/// 对应关系的事实拿原文说法显示(见 `facts.predicate_id`)。
pub async fn open_violations(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<AxiomViolation>> {
    Ok(sqlx::query_as(
        "WITH triple AS (
             SELECT f.id,
                    s.canonical_name || ' · '
                      || COALESCE(r.label, fact_surface_predicate(f.id), '?') || ' · '
                      || COALESCE(o.canonical_name, f.object_value ->> 'summary',
                                  f.object_value #>> '{}', '?') AS text,
                    r.label AS predicate
               FROM facts f
               JOIN entities s ON s.id = f.subject_id
               LEFT JOIN relation_types r ON r.id = f.predicate_id
               LEFT JOIN entities o ON o.id = f.object_id
              WHERE f.kb_id = $1
         )
         SELECT v.id, v.kind, l.predicate,
                v.left_fact, l.text AS left_text,
                v.right_fact, rt.text AS right_text,
                coalesce(array_length(v.path, 1), 0) AS path_len,
                v.detected_at
           FROM axiom_violations v
           JOIN triple l  ON l.id  = v.left_fact
           JOIN triple rt ON rt.id = v.right_fact
          WHERE v.kb_id = $1 AND v.status = 'open'
          ORDER BY v.detected_at DESC
          LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// 人裁决一处违规。
///
/// **三个出路,不是两个。** 时态冲突问「哪条对」,而这里可能是定义错了——
/// 用户导的本体把某个属性声明成反对称,而他自己的语料里那关系其实双向。
/// `axiom_relaxed` 记的就是这种:该改的是本体,不是二十条事实。
///
/// 改状态不删行,与账本同一个规矩:表过态这件事本身要留痕,而且 `run` 靠
/// `status = 'open'` 判断哪些是派生的、可以重算掉——人的决定必须活过重跑。
pub async fn decide(
    pool: &PgPool,
    kb_id: Uuid,
    violation_id: Uuid,
    resolution: &str,
    actor: Uuid,
) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE axiom_violations
            SET status = 'resolved', resolution = $3, decided_by = $4, decided_at = now()
          WHERE id = $2 AND kb_id = $1 AND status = 'open'",
    )
    .bind(kb_id)
    .bind(violation_id)
    .bind(resolution)
    .bind(actor)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(utopia_core::AppError::NotFound);
    }
    Ok(())
}

// ===================== R0 的另一半：本体自己 =====================

/// 本体自洽性检查的产出。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OntologyReport {
    pub classes: usize,
    pub found: usize,
    pub inserted: usize,
    pub cleared: usize,
}

/// 量一遍本体自己：谓词的公理组合、subClassOf 的环、不可满足的类。
///
/// 与 [`run`] 同一套重跑规矩：`open` 是派生状态、可以被重算掉，`resolved`
/// 是人的决定、一行不动。
pub async fn check_ontology(pool: &PgPool, kb_id: Uuid) -> AppResult<OntologyReport> {
    let ax = axioms(pool, kb_id).await?;
    let parents: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT child_id, parent_id FROM entity_type_parents p
                          JOIN entity_types t ON t.id = p.child_id
                         WHERE t.kb_id = $1",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let disjoint: Vec<(Uuid, Uuid)> =
        sqlx::query_as("SELECT a_id, b_id FROM entity_type_disjoint WHERE kb_id = $1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?;
    let classes: i64 = sqlx::query_scalar("SELECT count(*) FROM entity_types WHERE kb_id = $1")
        .bind(kb_id)
        .fetch_one(pool)
        .await?;

    let defects = utopia_reason::ontology::check_ontology(&ax, &parents, &disjoint);
    let mut report = OntologyReport {
        classes: classes as usize,
        found: defects.len(),
        ..Default::default()
    };

    let mut tx = pool.begin().await?;
    let mut fresh: Vec<Uuid> = Vec::with_capacity(defects.len());
    for d in &defects {
        let existing: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM ontology_defects
              WHERE kb_id = $1 AND kind = $2 AND subject = $3
                AND other IS NOT DISTINCT FROM $4",
        )
        .bind(kb_id)
        .bind(d.kind.as_str())
        .bind(d.subject)
        .bind(d.other)
        .fetch_optional(&mut *tx)
        .await?;
        let id = match existing {
            Some((id,)) => id,
            None => {
                let id = Uuid::now_v7();
                sqlx::query(
                    "INSERT INTO ontology_defects (id, kb_id, kind, subject, other, path)
                     VALUES ($1, $2, $3, $4, $5, $6)",
                )
                .bind(id)
                .bind(kb_id)
                .bind(d.kind.as_str())
                .bind(d.subject)
                .bind(d.other)
                .bind(&d.path)
                .execute(&mut *tx)
                .await?;
                report.inserted += 1;
                id
            }
        };
        fresh.push(id);
    }
    let cleared = sqlx::query(
        "DELETE FROM ontology_defects
          WHERE kb_id = $1 AND status = 'open' AND NOT (id = ANY($2))",
    )
    .bind(kb_id)
    .bind(&fresh)
    .execute(&mut *tx)
    .await?;
    report.cleared = cleared.rows_affected() as usize;
    tx.commit().await?;
    Ok(report)
}

// ===================== R1：物化推导 =====================

/// 一次推导的产出。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeriveReport {
    /// 编译出来的规则条数。**为零时结论是「没有规则」而不是「推不出东西」**
    pub rules: usize,
    pub edges: usize,
    /// 这一轮算出来的派生总数
    pub derived: usize,
    /// 新落库的
    pub inserted: usize,
    /// 前提没了、跟着作废的
    pub invalidated: usize,
    /// 撞上单谓词上限、没推完的谓词个数
    pub capped: usize,
    /// **推出来了却找不到对应规则行的条数。正常应当恒为零。**
    ///
    /// 不为零意味着规则编译与推导对不上了。之前这里是一句 `continue`，
    /// 于是 `ceo_of ⊑ works_at` 推出的那条 `works_at` 事实**推出来了却不落库**，
    /// 而下游靠它推出的 `employs` 反倒进了库——一条派生的前提凭空消失。
    /// 数出来，别再让它静默一次
    pub unruled: usize,
}

/// 派生事实的身份：三元组 + 区间。
///
/// **区间进键**是有意的：区间变了就是另一条断言，老的作废、新的落地，
/// 因为账本不许原地改。
type DerivedKey = (Uuid, Uuid, Uuid, Option<i64>, Option<i64>);

/// 精度按「最粗的那个」取。
///
/// 派生区间的两端各来自某一条前提，严格说该各随各的精度。取最粗是**故意保守**：
/// 一条链只和它最不确定的那一环一样可信，而把 year 级的前提推出来的结论标成
/// day，正是 `facts.valid_from_precision` 那条注释里说的「在无知的地方填一个
/// 确定的值」。
fn coarsest(a: Option<&str>, b: Option<&str>) -> Option<String> {
    let rank = |p: &str| match p {
        "year" => 0,
        "month" => 1,
        _ => 2,
    };
    match (a, b) {
        (Some(x), Some(y)) => Some(if rank(x) <= rank(y) { x } else { y }.to_string()),
        (Some(x), None) | (None, Some(x)) => Some(x.to_string()),
        (None, None) => None,
    }
}

/// 按本体公理重编译规则，返回 `(谓词, 种类) → 规则 id`。
///
/// **幂等**：身份取 `(kb, 谓词, 种类)`，重编译认得出「还是那条规则」——否则每跑
/// 一次 `derived_facts.rule_id` 就指向一个新 id，历史全断。
///
/// **公理撤了的规则不删。** 已失效的派生行仍指着它，解释「当时是靠哪条规则推的」
/// 需要它还在；而它不再出现在返回值里，据它推出来的事实由下面的对账作废。
/// 规则一个库也就几条，留着不占地方。
async fn compile_rules(
    pool: &PgPool,
    kb_id: Uuid,
    ax: &HashMap<Uuid, Axioms>,
) -> AppResult<HashMap<(Uuid, RuleKind), Uuid>> {
    let mut want: Vec<(Uuid, RuleKind)> = Vec::new();
    for (&pred, a) in ax {
        if a.transitive {
            want.push((pred, "transitive"));
        }
        if a.symmetric {
            want.push((pred, "symmetric"));
        }
        // 后两种是迁移 0016（a_relation_can_name_its_inverse）补上的规则源。**规则挂在「有声明的那一侧」**——
        // 归一化过的逆两边都有声明，所以两个方向各得一条规则，与它们各自
        // 推出的派生对得上
        if a.inverse_of.is_some() {
            want.push((pred, "inverse"));
        }
        if a.sub_property_of.is_some() {
            want.push((pred, "sub_property"));
        }
    }
    want.sort();
    let mut out = HashMap::new();
    for (pred, kind) in want {
        sqlx::query(
            "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, $4)
             ON CONFLICT (kb_id, predicate_id, kind) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(pred)
        .bind(kind)
        .execute(pool)
        .await?;
        let (id,): (Uuid,) = sqlx::query_as(
            "SELECT id FROM rules WHERE kb_id = $1 AND predicate_id = $2 AND kind = $3",
        )
        .bind(kb_id)
        .bind(pred)
        .bind(kind)
        .fetch_one(pool)
        .await?;
        out.insert((pred, kind), id);
    }
    Ok(out)
}

/// 取边时一并拿回来的随行信息（精度与置信度，落库要用）。
type EdgeRow = (
    Uuid,
    Uuid,
    Uuid,
    Uuid,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
    Option<String>,
    f32,
);

type LiveRow = (
    Uuid,
    Uuid,
    Uuid,
    Uuid,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// 推一遍，把派生事实落进账本。
///
/// **调用方负责检查 `materialize_inferences` 开关。** 这一层不判——它也被
/// 「预览一下会推出什么」那条路用，而预览不该受开关约束。
pub async fn materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<DeriveReport> {
    let ax = axioms(pool, kb_id).await?;
    let rules = compile_rules(pool, kb_id, &ax).await?;

    // 输入**只有断言**。派生住在另一张表，所以这里连过滤都不必写——那正是
    // 分表买到的东西：忘了排除的后果是推不出东西，不是把自己的输出喂回自己
    let rows: Vec<EdgeRow> = sqlx::query_as(
        "SELECT id, predicate_id, subject_id, object_id,
                valid_from, valid_to, valid_from_precision, valid_to_precision, confidence
           FROM facts
          WHERE kb_id = $1
            AND invalidated_at IS NULL
            AND predicate_id IS NOT NULL
            AND object_id IS NOT NULL",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;

    let mut edges = Vec::with_capacity(rows.len());
    let mut meta: HashMap<Uuid, (Option<String>, Option<String>, f32)> = HashMap::new();
    let mut spans: HashMap<Uuid, (Option<i64>, Option<i64>)> = HashMap::new();
    for (id, pred, subj, obj, from, to, fp, tp, conf) in rows {
        let (f, t) = (from.map(|x| x.timestamp()), to.map(|x| x.timestamp()));
        edges.push(TimedEdge {
            edge: Edge {
                fact: id,
                predicate: pred,
                subject: subj,
                object: obj,
            },
            from: f,
            to: t,
        });
        spans.insert(id, (f, t));
        meta.insert(id, (fp, tp, conf));
    }

    let derivation = utopia_reason::derive::derive(&edges, &ax);
    let mut report = DeriveReport {
        rules: rules.len(),
        edges: edges.len(),
        derived: derivation.facts.len(),
        capped: derivation.capped.len(),
        ..Default::default()
    };

    let mut wanted: HashMap<DerivedKey, &utopia_reason::derive::Derived> = HashMap::new();
    for d in &derivation.facts {
        let Some((from, to)) = utopia_reason::derive::validity(&d.premises, &spans) else {
            continue;
        };
        wanted.insert((d.subject, d.predicate, d.object, from, to), d);
    }

    let mut tx = pool.begin().await?;
    let live: Vec<LiveRow> = sqlx::query_as(
        "SELECT id, subject_id, predicate_id, object_id, valid_from, valid_to
           FROM derived_facts
          WHERE kb_id = $1 AND invalidated_at IS NULL",
    )
    .bind(kb_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut stale: Vec<Uuid> = Vec::new();
    for (id, s, p, o, from, to) in &live {
        let key = (
            *s,
            *p,
            *o,
            from.map(|x| x.timestamp()),
            to.map(|x| x.timestamp()),
        );
        if wanted.remove(&key).is_none() {
            stale.push(*id);
        }
    }

    // 前提没了 → 派生跟着失效。**置 invalidated_at 而不是删**：与拒绝一条事实
    // 完全同构，记录轴上留下「我们曾据此推出，后来前提没了」，实体历史页面
    // 直接就能展示（0002 第 3 节）
    if !stale.is_empty() {
        sqlx::query("UPDATE derived_facts SET invalidated_at = now() WHERE id = ANY($1)")
            .bind(&stale)
            .execute(&mut *tx)
            .await?;
        report.invalidated = stale.len();
    }

    for ((subject, predicate, object, from, to), d) in wanted {
        // **按 `via` 查，不是 `predicate`。** 规则行是给「声明了公理的那个
        // 谓词」编的；跨谓词的两条规则里，派生出来的谓词是另一个。
        // 原先按 predicate 查，前两条规则一直对（两者相同），加了 inverse /
        // sub_property 之后查不到规则就 `continue`——推出来了却不落库
        let Some(&rule_id) = rules.get(&(d.via, d.rule.as_str())) else {
            // 查不到规则是**编译与推导不一致**，不是正常情况。数出来，
            // 别再让它静默消失一次
            report.unruled += 1;
            continue;
        };
        // 精度与置信度都取前提里最保守的那一个
        let mut fp: Option<String> = None;
        let mut tp: Option<String> = None;
        let mut conf = 1.0f32;
        for p in &d.premises {
            if let Some((pf, pt, pc)) = meta.get(p) {
                fp = coarsest(fp.as_deref(), pf.as_deref());
                tp = coarsest(tp.as_deref(), pt.as_deref());
                conf = conf.min(*pc);
            }
        }
        // 约束是「有日期才有精度」——交集把某一端算成无界时，那一端的精度也得清掉
        let fp = from.and(fp);
        let tp = to.and(tp);
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id,
                                        valid_from, valid_to,
                                        valid_from_precision, valid_to_precision,
                                        confidence, rule_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(id)
        .bind(kb_id)
        .bind(subject)
        .bind(predicate)
        .bind(object)
        .bind(from.map(stamp))
        .bind(to.map(stamp))
        .bind(&fp)
        .bind(&tp)
        .bind(conf)
        .bind(rule_id)
        .execute(&mut *tx)
        .await?;
        for (seq, premise) in d.premises.iter().enumerate() {
            sqlx::query(
                "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id, seq)
                 VALUES ($1, $2, $3)",
            )
            .bind(id)
            .bind(premise)
            .bind(seq as i32)
            .execute(&mut *tx)
            .await?;
        }
        report.inserted += 1;
    }
    tx.commit().await?;
    Ok(report)
}

fn stamp(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs, 0).unwrap_or_default()
}

/// Review 页要看的本体缺陷，连同标签。
///
/// 标签在 SQL 里取而不是回来再查：`subject` 那一列同一列指两张表（谓词或类），
/// 分开查就要先按 kind 分组、再发两批查询，而一次 LEFT JOIN 两张表就够——
/// 一个 id 只可能命中其中一张。
pub async fn open_defects(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<OntologyDefect>> {
    Ok(sqlx::query_as(
        "SELECT d.id, d.kind,
                COALESCE(st.label, sr.label) AS subject_label,
                ot.label AS other_label,
                COALESCE(
                    (SELECT array_agg(t.label ORDER BY x.ord)
                       FROM unnest(d.path) WITH ORDINALITY AS x(id, ord)
                       JOIN entity_types t ON t.id = x.id),
                    ARRAY[]::text[]
                ) AS path_labels,
                d.detected_at
           FROM ontology_defects d
           LEFT JOIN entity_types   st ON st.id = d.subject
           LEFT JOIN relation_types sr ON sr.id = d.subject
           LEFT JOIN entity_types   ot ON ot.id = d.other
          WHERE d.kb_id = $1 AND d.status = 'open'
          ORDER BY d.detected_at DESC
          LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// 人对一处本体缺陷表态。
///
/// 两个出路而不是三个：本体缺陷没有「数据错了」这一条——它压根没看数据。
/// `fixed` 是「我去改了本体」，`accepted` 是「看过，不必改」。
pub async fn decide_defect(
    pool: &PgPool,
    kb_id: Uuid,
    defect_id: Uuid,
    resolution: &str,
    actor: Uuid,
) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE ontology_defects
            SET status = 'resolved', resolution = $3, decided_by = $4, decided_at = now()
          WHERE id = $2 AND kb_id = $1 AND status = 'open'",
    )
    .bind(kb_id)
    .bind(defect_id)
    .bind(resolution)
    .bind(actor)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(utopia_core::AppError::NotFound);
    }
    Ok(())
}

/// 到点该重推的库。
///
/// 与来源同步同一个形状：一个间隔 + 一个上次时间。**从没推过的算到期**——
/// 刚打开开关的库不该等一个周期才第一次推。
pub async fn due_for_inference(pool: &PgPool) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM knowledge_bases
          WHERE materialize_inferences
            AND (last_inference_at IS NULL
                 OR last_inference_at < now()
                    - make_interval(mins => inference_interval_minutes))",
    )
    .fetch_all(pool)
    .await?)
}

/// 记下这一轮推完的时间。
///
/// **推完就记，哪怕什么都没变**：这一列答的是「上次看过没有」，不是「上次改过
/// 没有」。不记的话没变化的库会每分钟被扫起来重算一遍。
pub async fn mark_inference_ran(pool: &PgPool, kb_id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE knowledge_bases SET last_inference_at = now() WHERE id = $1")
        .bind(kb_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 一条派生事实的证明，展开到原句（0002 R2）。
///
/// `fact_derivations` 只记直接前提，而前提一律是断言，所以「递归展开」在这里
/// 退化成一条链：派生 → 按 `seq` 的断言 → 每条断言的证据。叶子是 chunk，
/// 界面上一路点到文档。**撤了的前提照样列出并打上标记**：派生随前提失效，
/// 但「当时靠的是什么」要读得出来，那正是记录轴存在的理由。
///
/// 派生已失效或不存在时回 None——不是错误，界面据此收起。
pub async fn proof(
    pool: &PgPool,
    kb_id: Uuid,
    derived_id: Uuid,
) -> AppResult<Option<utopia_core::models::Proof>> {
    let Some(derived) = derived_one(pool, kb_id, derived_id).await? else {
        return Ok(None);
    };
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        i32,
        Uuid,
        Uuid,
        String,
        Option<Uuid>,
        Option<String>,
        Option<Uuid>,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
        f32,
        bool,
    )> = sqlx::query_as(
        "SELECT fd.seq, f.id, f.subject_id, s.canonical_name,
                f.predicate_id, r.label, f.object_id, o.canonical_name,
                f.valid_from, f.valid_to, f.confidence,
                f.invalidated_at IS NOT NULL
           FROM fact_derivations fd
           JOIN facts f ON f.id = fd.premise_fact_id
           JOIN entities s ON s.id = f.subject_id
           LEFT JOIN relation_types r ON r.id = f.predicate_id
           LEFT JOIN entities o ON o.id = f.object_id
          WHERE fd.derived_fact_id = $1
          ORDER BY fd.seq",
    )
    .bind(derived_id)
    .fetch_all(pool)
    .await?;
    let mut steps = Vec::with_capacity(rows.len());
    for (
        seq,
        fact_id,
        subject_id,
        subject,
        predicate_id,
        predicate,
        object_id,
        object,
        valid_from,
        valid_to,
        confidence,
        retracted,
    ) in rows
    {
        // 一条链最多 MAX_DEPTH 步，逐条取证据是可数的几次往返
        let evidence = crate::graph::fact_evidence(pool, fact_id).await?;
        steps.push(utopia_core::models::ProofStep {
            seq,
            fact_id,
            subject_id,
            subject,
            predicate_id,
            predicate,
            object_id,
            object,
            valid_from,
            valid_to,
            confidence,
            retracted,
            evidence,
        });
    }
    Ok(Some(utopia_core::models::Proof { derived, steps }))
}

/// 按 id 取一条派生（失效的也取：证明要能回看）。
async fn derived_one(
    pool: &PgPool,
    kb_id: Uuid,
    derived_id: Uuid,
) -> AppResult<Option<DerivedFactView>> {
    Ok(sqlx::query_as(
        "SELECT d.id,
                d.subject_id, s.canonical_name AS subject,
                d.object_id,  o.canonical_name AS object,
                r.label AS predicate,
                ru.kind AS rule,
                d.valid_from, d.valid_to, d.confidence, d.derived_at,
                COALESCE(
                    (SELECT array_agg(
                                ps.canonical_name || ' · '
                                || COALESCE(pr.label, '?') || ' · '
                                || COALESCE(po.canonical_name, '?')
                                ORDER BY fd.seq)
                       FROM fact_derivations fd
                       JOIN facts pf       ON pf.id = fd.premise_fact_id
                       JOIN entities ps    ON ps.id = pf.subject_id
                       LEFT JOIN relation_types pr ON pr.id = pf.predicate_id
                       LEFT JOIN entities po ON po.id = pf.object_id
                      WHERE fd.derived_fact_id = d.id),
                    ARRAY[]::text[]
                ) AS premises
           FROM derived_facts d
           JOIN entities s ON s.id = d.subject_id
           JOIN entities o ON o.id = d.object_id
           JOIN relation_types r ON r.id = d.predicate_id
           JOIN rules ru ON ru.id = d.rule_id
          WHERE d.kb_id = $1 AND d.id = $2",
    )
    .bind(kb_id)
    .bind(derived_id)
    .fetch_optional(pool)
    .await?)
}

/// 一条派生事实，配好展示与证明所需的文本（实体面板的「推出来的」那一档）。
///
/// **证明一起取回来**：这一档存在的理由就是「这条边不是谁说的，是这么推出来的」，
/// 而不给出前提的话它跟一条普通的边看不出区别——那正是用户担心的污染。
pub async fn derived_for_entity(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
) -> AppResult<Vec<DerivedFactView>> {
    Ok(sqlx::query_as(
        "SELECT d.id,
                d.subject_id, s.canonical_name AS subject,
                d.object_id,  o.canonical_name AS object,
                r.label AS predicate,
                ru.kind AS rule,
                d.valid_from, d.valid_to, d.confidence, d.derived_at,
                COALESCE(
                    (SELECT array_agg(
                                ps.canonical_name || ' · '
                                || COALESCE(pr.label, '?') || ' · '
                                || COALESCE(po.canonical_name, '?')
                                ORDER BY fd.seq)
                       FROM fact_derivations fd
                       JOIN facts pf       ON pf.id = fd.premise_fact_id
                       JOIN entities ps    ON ps.id = pf.subject_id
                       LEFT JOIN relation_types pr ON pr.id = pf.predicate_id
                       LEFT JOIN entities po ON po.id = pf.object_id
                      WHERE fd.derived_fact_id = d.id),
                    ARRAY[]::text[]
                ) AS premises
           FROM derived_facts d
           JOIN entities s ON s.id = d.subject_id
           JOIN entities o ON o.id = d.object_id
           JOIN relation_types r ON r.id = d.predicate_id
           JOIN rules ru ON ru.id = d.rule_id
          WHERE d.kb_id = $1 AND d.invalidated_at IS NULL
            AND (d.subject_id = $2 OR d.object_id = $2)
          ORDER BY d.derived_at DESC",
    )
    .bind(kb_id)
    .bind(entity_id)
    .fetch_all(pool)
    .await?)
}
