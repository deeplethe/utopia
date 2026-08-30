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
use utopia_core::models::AxiomViolation;
use utopia_core::AppResult;
use utopia_reason::{check, Axioms, Edge, Violation};
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
async fn axioms(pool: &PgPool, kb_id: Uuid) -> AppResult<HashMap<Uuid, Axioms>> {
    let rows: Vec<(Uuid, bool, bool, bool, bool, bool, bool)> = sqlx::query_as(
        "SELECT id, is_transitive, is_symmetric, is_asymmetric, is_irreflexive,
                functional, inverse_functional
           FROM relation_types
          WHERE kb_id = $1
            AND (is_transitive OR is_symmetric OR is_asymmetric OR is_irreflexive
                 OR functional OR inverse_functional)",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
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
                    },
                )
            },
        )
        .collect())
}

/// 跑一遍检查，把结果落库。
pub async fn run(pool: &PgPool, kb_id: Uuid) -> AppResult<Report> {
    let edges = edges(pool, kb_id).await?;
    let axioms = axioms(pool, kb_id).await?;
    let violations = check(&edges, &axioms);

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
          LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
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
