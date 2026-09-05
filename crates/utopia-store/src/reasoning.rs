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

use serde_json::json;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use utopia_core::models::{AxiomViolation, DerivedFactView, OntologyDefect};
/// 规则种类的字面量。用 &'static str 而不是枚举:它直接进 SQL 也直接做键
type RuleKind = &'static str;
use utopia_core::AppResult;
use utopia_reason::derive::{Contradictions, Derivation, TimedEdge};
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
    /// 派生撞上断言的条数（0017），已含在 `found` 里
    pub contradictions: usize,
    /// 撞上单谓词上限、没进队列的矛盾条数。**不为零时说明根子在规则**：
    /// 一条谓词上上百条派生都撞了，逐条看是没有意义的
    pub contradictions_capped: usize,
    /// 互撞的规则对数——进 `ontology_defects`，不进这张表
    pub rules_disagree: usize,
    /// 重新打开的 resolved 行：人曾说撤了、闭合了、要去改本体，而违规又算出来了——
    /// 承诺没兑现，队列不替人沉默（#202）
    pub reopened: usize,
}

/// 单个谓词上进队列的矛盾上限（0017 §1）。超出的部分只计数。
const MAX_CLASHES_PER_PREDICATE: usize = 50;

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
    let (timed, spans, _) = timed_edges(pool, kb_id).await?;
    let edges: Vec<Edge> = timed.iter().map(|t| t.edge).collect();
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

    // 第六类（0017）：推出来却落不了地的派生。与 `materialize` 用同一个函数算，
    // 所以这里报的正是那边拦下的——两边各算一套的话，队列会跟图对不上
    let derivation = utopia_reason::derive::derive(&timed, &axioms);
    let clashes = utopia_reason::derive::contradictions(&derivation, &timed, &axioms, &spans);
    let names = names_for(pool, &derivation, &clashes).await?;
    let mut details: HashMap<(Uuid, Uuid), serde_json::Value> = HashMap::new();
    let mut per_pred: HashMap<Uuid, usize> = HashMap::new();
    let mut contradictions_capped = 0usize;
    for c in &clashes.with_assertions {
        let d = &derivation.facts[c.derived];
        let Some(&last) = d.premises.last() else {
            continue;
        };
        let key = (c.against, last);
        if details.contains_key(&key) {
            continue;
        }
        let n = per_pred.entry(d.predicate).or_default();
        if *n >= MAX_CLASHES_PER_PREDICATE {
            contradictions_capped += 1;
            continue;
        }
        *n += 1;
        let span = utopia_reason::derive::validity(&d.premises, &spans);
        details.insert(
            key,
            json!({
                "axiom": c.axiom.as_str(),
                "rule": d.rule.as_str(),
                "via": d.via,
                "via_label": names.predicate(d.via),
                "subject_id": d.subject,
                "subject": names.entity(d.subject),
                "predicate_id": d.predicate,
                "predicate": names.predicate(d.predicate),
                "object_id": d.object,
                "object": names.entity(d.object),
                "valid_from": span.and_then(|s| s.0).map(|t| stamp(t).to_rfc3339()),
                "valid_to": span.and_then(|s| s.1).map(|t| stamp(t).to_rfc3339()),
                "premises": d.premises,
            }),
        );
        violations.push(Violation {
            kind: Kind::DerivedContradiction,
            left: c.against,
            right: last,
            path: d.premises.clone(),
        });
    }

    let mut report = Report {
        edges: edges.len(),
        predicates_with_axioms: axioms.len(),
        found: violations.len(),
        contradictions: details.len(),
        contradictions_capped,
        rules_disagree: clashes.between_derivations.len(),
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
        let detail = details
            .get(&(*left, *right))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let id: Option<(Uuid,)> = sqlx::query_as(
            "INSERT INTO axiom_violations (id, kb_id, kind, left_fact, right_fact, path, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (kb_id, kind, left_fact, right_fact) DO NOTHING
             RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(kind.as_str())
        .bind(left)
        .bind(right)
        .bind(path)
        .bind(&detail)
        .fetch_optional(&mut *tx)
        .await?;
        if id.is_some() {
            report.inserted += 1;
        }
        // 无论新插的还是本来就在的，都算「这一轮仍然成立」。
        //
        // 本来就在而且 resolved 的要再看一眼：`fact_retracted` / `fact_closed` /
        // `axiom_relaxed` 都是「世界会变」的承诺——事实没了、区间闭了、公理放宽了，
        // 违规就不该再算出来。又算出来了，承诺就是没兑现，那行回到 open，人再看一次。
        // `accepted` 是有意并存，重算多少次都沉默（#202）
        let (keep, status, resolution): (Uuid, String, Option<String>) = sqlx::query_as(
            "SELECT id, status, resolution FROM axiom_violations
              WHERE kb_id = $1 AND kind = $2 AND left_fact = $3 AND right_fact = $4",
        )
        .bind(kb_id)
        .bind(kind.as_str())
        .bind(left)
        .bind(right)
        .fetch_one(&mut *tx)
        .await?;
        if status == "resolved"
            && matches!(
                resolution.as_deref(),
                Some("fact_retracted" | "fact_closed" | "axiom_relaxed")
            )
        {
            sqlx::query(
                "UPDATE axiom_violations
                    SET status = 'open', resolution = NULL, decided_by = NULL,
                        decided_at = NULL, detected_at = now()
                  WHERE id = $1",
            )
            .bind(keep)
            .execute(&mut *tx)
            .await?;
            report.reopened += 1;
        }
        fresh.push(keep);
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

    // 派生之间互撞的按规则对进 `ontology_defects`——根子是那两条声明，不是哪条事实。
    // 同一对谓词上可能有几种撞法（functional 与 asymmetric 各撞各的），唯一键只到
    // 谓词对，所以合成一行，几种撞法都写进 detail
    let mut by_pair: HashMap<(Uuid, Uuid), Vec<serde_json::Value>> = HashMap::new();
    let mut order: Vec<(Uuid, Uuid)> = Vec::new();
    for rc in &clashes.between_derivations {
        let triple = |i: usize| {
            let d = &derivation.facts[i];
            format!(
                "{} · {} · {}",
                names.entity(d.subject),
                names.predicate(d.predicate),
                names.entity(d.object)
            )
        };
        let examples: Vec<serde_json::Value> = rc
            .pairs
            .iter()
            .take(3)
            .map(|(i, j)| json!([triple(*i), triple(*j)]))
            .collect();
        let key = (rc.a.0, rc.b.0);
        if !by_pair.contains_key(&key) {
            order.push(key);
        }
        by_pair.entry(key).or_default().push(json!({
            "rule_a": rc.a.1.as_str(),
            "via_a": names.predicate(rc.a.0),
            "rule_b": rc.b.1.as_str(),
            "via_b": names.predicate(rc.b.0),
            "axiom": rc.axiom.as_str(),
            "count": rc.pairs.len(),
            "examples": examples,
        }));
    }
    let mut fresh_defects: Vec<Uuid> = Vec::with_capacity(order.len());
    for key in order {
        let rules = by_pair.remove(&key).unwrap_or_default();
        let count: usize = rules
            .iter()
            .map(|r| r["count"].as_u64().unwrap_or(0) as usize)
            .sum();
        // 已经有人认可过的那一行保持 resolved，只刷 detail：0017 说认可之后不再报
        let (id,): (Uuid,) = sqlx::query_as(
            "INSERT INTO ontology_defects (id, kb_id, kind, subject, other, path, detail)
             VALUES ($1, $2, 'rules_disagree', $3, $4, '{}', $5)
             ON CONFLICT (kb_id, kind, subject, other) DO UPDATE SET detail = EXCLUDED.detail
             RETURNING id",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(key.0)
        .bind(key.1)
        .bind(json!({ "count": count, "rules": rules }))
        .fetch_one(&mut *tx)
        .await?;
        fresh_defects.push(id);
    }
    sqlx::query(
        "DELETE FROM ontology_defects
          WHERE kb_id = $1 AND kind = 'rules_disagree' AND status = 'open'
            AND NOT (id = ANY($2))",
    )
    .bind(kb_id)
    .bind(&fresh_defects)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(report)
}

/// 矛盾要写成人能读的话，而派生没有落库、没有文本可查——名字在这里补。
struct Names {
    entities: HashMap<Uuid, String>,
    predicates: HashMap<Uuid, String>,
}

impl Names {
    fn entity(&self, id: Uuid) -> String {
        self.entities
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "?".into())
    }
    fn predicate(&self, id: Uuid) -> String {
        self.predicates
            .get(&id)
            .cloned()
            .unwrap_or_else(|| "?".into())
    }
}

async fn names_for(
    pool: &PgPool,
    derivation: &Derivation,
    clashes: &Contradictions,
) -> AppResult<Names> {
    let mut ents: HashSet<Uuid> = HashSet::new();
    let mut preds: HashSet<Uuid> = HashSet::new();
    let mut want = |i: usize| {
        let d = &derivation.facts[i];
        ents.insert(d.subject);
        ents.insert(d.object);
        preds.insert(d.predicate);
        preds.insert(d.via);
    };
    for c in &clashes.with_assertions {
        want(c.derived);
    }
    for rc in &clashes.between_derivations {
        for (i, j) in rc.pairs.iter().take(3) {
            want(*i);
            want(*j);
        }
    }
    for rc in &clashes.between_derivations {
        preds.insert(rc.a.0);
        preds.insert(rc.b.0);
    }
    let ents: Vec<Uuid> = ents.into_iter().collect();
    let preds: Vec<Uuid> = preds.into_iter().collect();
    let entities: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, canonical_name FROM entities WHERE id = ANY($1)")
            .bind(&ents)
            .fetch_all(pool)
            .await?;
    let predicates: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, label FROM relation_types WHERE id = ANY($1)")
            .bind(&preds)
            .fetch_all(pool)
            .await?;
    Ok(Names {
        entities: entities.into_iter().collect(),
        predicates: predicates.into_iter().collect(),
    })
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
    Ok(sqlx::query_as(&format!(
        "WITH triple AS (
             SELECT f.id,
                    s.canonical_name || ' · '
                      || COALESCE(r.label, fact_surface_predicate(f.id), '?') || ' · '
                      || COALESCE(o.canonical_name, f.object_value ->> 'summary',
                                  f.object_value #>> '{{}}', '?') AS text,
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
                v.detected_at, v.detail,
                COALESCE((SELECT jsonb_agg(jsonb_build_object('id', x.id, 'text', pt.text)
                                           ORDER BY x.ord)
                            FROM unnest(v.path) WITH ORDINALITY AS x(id, ord)
                            JOIN triple pt ON pt.id = x.id), '[]'::jsonb) AS path,
                {left_holds_to} IS NULL AS left_open,
                lf.confidence AS left_confidence,
                EXISTS (
                    SELECT 1 FROM entities e
                    JOIN entities x ON x.kb_id = e.kb_id AND x.id <> e.id
                                   AND x.merged_into IS NULL
                                   AND lower(x.canonical_name) = lower(e.canonical_name)
                    WHERE e.id IN (lf.subject_id, lf.object_id)
                ) AS same_name_peers
           FROM axiom_violations v
           JOIN triple l  ON l.id  = v.left_fact
           JOIN triple rt ON rt.id = v.right_fact
           JOIN facts lf ON lf.id = v.left_fact
          WHERE v.kb_id = $1 AND v.status = 'open'
          ORDER BY v.detected_at DESC
          LIMIT $2 OFFSET $3",
        // 「左边还开着」按读出来的终点判（0022）：结束了不知哪天的不算开着
        left_holds_to = crate::world_axis::facts_holds_to("lf"),
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r: ViolationRow| {
        let hint = if r.kind == "derived_contradiction" {
            hint_for(&r).map(String::from)
        } else {
            None
        };
        AxiomViolation {
            id: r.id,
            kind: r.kind,
            predicate: r.predicate,
            left_fact: r.left_fact,
            left_text: r.left_text,
            right_fact: r.right_fact,
            right_text: r.right_text,
            path_len: r.path_len,
            detected_at: r.detected_at,
            detail: r.detail,
            hint,
            path: serde_json::from_value(r.path).unwrap_or_default(),
        }
    })
    .collect())
}

#[derive(sqlx::FromRow)]
struct ViolationRow {
    id: Uuid,
    kind: String,
    predicate: Option<String>,
    left_fact: Uuid,
    left_text: String,
    right_fact: Uuid,
    right_text: String,
    path_len: i32,
    detected_at: chrono::DateTime<chrono::Utc>,
    detail: serde_json::Value,
    path: serde_json::Value,
    left_open: bool,
    left_confidence: f32,
    same_name_peers: bool,
}

/// 线索按最常见的错法排（0017 §2）：旧断言没写结束日期、两个同名实体、抽取本来就
/// 没把握。一次只给一条——三条并列等于没给
fn hint_for(r: &ViolationRow) -> Option<&'static str> {
    if r.left_open && r.detail.get("valid_from").is_some_and(|v| !v.is_null()) {
        Some("stale")
    } else if r.same_name_peers {
        Some("duplicate")
    } else if r.left_confidence < 0.75 {
        Some("unsure")
    } else {
        None
    }
}

/// 人裁决一处违规。
///
/// **三个出路,不是两个。** 时态冲突问「哪条对」,而这里可能是定义错了——
/// 用户导的本体把某个属性声明成反对称,而他自己的语料里那关系其实双向。
/// `axiom_relaxed` 记的就是这种:该改的是本体,不是二十条事实。
///
/// 改状态不删行,与账本同一个规矩:表过态这件事本身要留痕,而且 `run` 靠
/// `status = 'open'` 判断哪些是派生的、可以重算掉——人的决定必须活过重跑。
/// 一处违规里该撤哪条事实。
///
/// 单事实的种类（自环、签名、派生撞断言）只有一条，不用说；双事实与环上的要人指名，
/// 而且只能指违规自己列出的那几条——撤一条不相干的事实不是裁决，是误操作
pub fn pick_retraction(
    left: Uuid,
    right: Uuid,
    path: &[Uuid],
    requested: Option<Uuid>,
) -> Option<Uuid> {
    if left == right {
        return match requested {
            None => Some(left),
            Some(r) if r == left => Some(left),
            Some(_) => None,
        };
    }
    let r = requested?;
    (r == left || r == right || path.contains(&r)).then_some(r)
}

/// 「数据错了」：**真的撤掉那条事实**，再把违规标成 resolved（#202）。
///
/// 此前只改 `axiom_violations`，事实照样活在图里；重跑撞上 resolved 行又什么都不做，
/// 违规既没消失也不再出现。撤走的是 `reject_fact` 那条路——`invalidated_at`，
/// 证据不动，账本留痕。回撤掉的那条 id，调用方据此记审计
pub async fn retract_from_violation(
    pool: &PgPool,
    kb_id: Uuid,
    violation_id: Uuid,
    requested: Option<Uuid>,
    actor: Uuid,
) -> AppResult<Uuid> {
    let row: Option<(Uuid, Uuid, Vec<Uuid>)> = sqlx::query_as(
        "SELECT left_fact, right_fact, path FROM axiom_violations
          WHERE id = $1 AND kb_id = $2 AND status = 'open'",
    )
    .bind(violation_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    let Some((left, right, path)) = row else {
        return Err(utopia_core::AppError::NotFound);
    };
    let Some(target) = pick_retraction(left, right, &path, requested) else {
        return Err(utopia_core::AppError::invalid(
            "fact_required",
            "这处违规涉及多条事实，要说撤哪一条，且只能是它列出的那几条",
        ));
    };
    crate::graph::reject_fact(pool, kb_id, target).await?;
    decide(pool, kb_id, violation_id, "fact_retracted", actor).await?;
    Ok(target)
}

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
          WHERE kb_id = $1 AND status = 'open' AND kind <> 'rules_disagree'
            AND NOT (id = ANY($2))",
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
    /// 推出来了却撞上断言或别的派生、这一轮拦下没落的（0017）。**拦下的每一条
    /// 都在 Review 里有对应的一行**——`run` 与这里用同一个函数算
    pub blocked: usize,
    /// 参与求值的业务规则条数（0021）
    pub attribute_rules: usize,
    /// 业务规则命中数
    pub rule_hits: usize,
    /// 前提组合太多、没展开完的 (规则, 实体) 对数。**与「不满足」区分开报**
    pub rule_capped: usize,
}

/// 一次取数，三样东西：带区间的边、每条事实的区间、精度与置信度。
/// `run` 与 `materialize` 共用——两边看到的边必须是同一批
/// 前提的精度、置信度，以及两端各自**是不是锚点**（0022）：来自证据日期而不是原文
/// 日期的那一端，推出来的派生行在那一端没有精度可言
type PremiseMeta = (Option<String>, Option<String>, f32, bool, bool);

type TimedEdges = (
    Vec<TimedEdge>,
    HashMap<Uuid, (Option<i64>, Option<i64>)>,
    HashMap<Uuid, PremiseMeta>,
);

/// 一条前提**读出来的**区间（0022）：原文没给起点就从锚点起，说结束了不知哪天就到
/// 锚点为止。两端都不知道的行读成空区间，求交时自然掉出去——它支撑不了任何派生。
/// 返回 `(from, to, from_anchored, to_anchored)`。
fn read_span(
    from: Option<chrono::DateTime<chrono::Utc>>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    to_precision: Option<&str>,
    attested_at: chrono::DateTime<chrono::Utc>,
) -> (Option<i64>, Option<i64>, bool, bool) {
    let anchor = attested_at.timestamp();
    let (f, from_anchored) = match from {
        Some(x) => (Some(x.timestamp()), false),
        None => (Some(anchor), true),
    };
    let (t, to_anchored) = match (to, to_precision) {
        (Some(x), _) => (Some(x.timestamp()), false),
        (None, Some(p)) if p == crate::graph::ENDED_UNKNOWN => (Some(anchor), true),
        (None, _) => (None, false),
    };
    (f, t, from_anchored, to_anchored)
}

async fn timed_edges(pool: &PgPool, kb_id: Uuid) -> AppResult<TimedEdges> {
    // 输入**只有断言**。派生住在另一张表，所以这里连过滤都不必写——那正是
    // 分表买到的东西：忘了排除的后果是推不出东西，不是把自己的输出喂回自己
    let rows: Vec<EdgeRow> = sqlx::query_as(
        "SELECT id, predicate_id, subject_id, object_id,
                valid_from, valid_to, valid_from_precision, valid_to_precision, confidence,
                attested_at
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
    let mut meta: HashMap<Uuid, PremiseMeta> = HashMap::new();
    let mut spans: HashMap<Uuid, (Option<i64>, Option<i64>)> = HashMap::new();
    for (id, pred, subj, obj, from, to, fp, tp, conf, attested) in rows {
        // 按读出来的区间推（0022）：没起点的前提从最早的证据起，结束了不知哪天的
        // 到说出它的那份文档为止。读成开放的话，一条经过 "former CEO" 的链会推出
        // 一条今天还成立的边
        let (f, t, fa, ta) = read_span(from, to, tp.as_deref(), attested);
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
        meta.insert(id, (fp, tp, conf, fa, ta));
    }
    Ok((edges, spans, meta))
}

/// 人认可过并存的（派生三元组, 断言）对：这些派生下一轮照常落地（0017 §2）。
async fn accepted_clashes(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<HashSet<(Uuid, Uuid, Uuid, Uuid)>> {
    let rows: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT left_fact, detail FROM axiom_violations
          WHERE kb_id = $1 AND kind = 'derived_contradiction' AND resolution = 'accepted'",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let id = |v: &serde_json::Value, k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .and_then(|s| s.parse::<Uuid>().ok())
    };
    Ok(rows
        .into_iter()
        .filter_map(|(against, d)| {
            Some((
                id(&d, "subject_id")?,
                id(&d, "predicate_id")?,
                id(&d, "object_id")?,
                against,
            ))
        })
        .collect())
}

/// 派生事实的身份：主 + 谓 + 宾 + 区间。
///
/// **区间进键**是有意的：区间变了就是另一条断言，老的作废、新的落地，
/// 因为账本不许原地改。
///
/// 宾语两格,与 `derived_facts` 拓宽后的两条通道一一对应(0021 决策 1):实体宾语
/// 走 `Option<Uuid>`,字面值结论走那串规范化过的 JSON。两格都参与比较——否则
/// 同一个类上的两条不同结论会被认成同一条。
type DerivedKey = (
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// 这一轮要落库的一条派生。公理推出来的与规则推出来的在这里合流——
/// **合流是必须的**：陈旧行的对账扫的是整张表，两趟各做各的 diff 会把对方的
/// 行每轮都判成陈旧作废掉。
struct Wanted {
    subject: Uuid,
    predicate: Uuid,
    object_id: Option<Uuid>,
    object_value: Option<serde_json::Value>,
    from: Option<i64>,
    to: Option<i64>,
    premises: Vec<Uuid>,
    /// 公理规则（`rules.id`）或业务规则（`attribute_rules.id`），恰好一个
    rule_id: Option<Uuid>,
    attribute_rule_id: Option<Uuid>,
}

/// JSON 值的规范化文本形态，只用来做键。
///
/// `serde_json::Value` 自己不是 `Hash`，而键里必须带上字面值宾语；序列化成
/// 字符串是最省事且稳定的做法——`Map` 在 serde_json 默认是 `BTreeMap`，
/// 同样的内容序列化出来逐字节相同。
fn value_key(v: Option<&serde_json::Value>) -> Option<String> {
    v.map(|v| v.to_string())
}

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
    chrono::DateTime<chrono::Utc>,
);

type LiveRow = (
    Uuid,
    Uuid,
    Uuid,
    Option<Uuid>,
    Option<serde_json::Value>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

/// 一条业务规则连同它的条件，已经解析成求值器的形状。
struct LoadedRule {
    rule: utopia_reason::rules::BusinessRule,
    /// 规则只看这个类**及其子类**的实体
    subject_types: Vec<Uuid>,
    /// 结论落在哪个谓词上：归类落 `is_a`，属性落它自己那个
    conclude_predicate: Uuid,
}

/// 求值要用的一行规则：id、主类、结论种类、结论那三格，外加结论类的 IRI 与 key
/// （归类结论按 IRI 记，没有才退回 key）
type RuleDefRow = (
    Uuid,
    Uuid,
    String,
    Option<Uuid>,
    Option<Uuid>,
    Option<serde_json::Value>,
    Option<String>,
    Option<String>,
);

/// 取业务规则。条件形状不合法的规则**整条跳过而不是报错退出**——一条写坏的
/// 规则不该让整轮物化停摆，而它不产出这件事在报告的条数里看得见。
async fn attribute_rules(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<LoadedRule>> {
    use utopia_reason::rules::{BusinessRule, Conclusion, Condition, Op};

    let rows: Vec<RuleDefRow> = sqlx::query_as(
        "SELECT r.id, r.subject_type_id, r.conclusion,
                r.conclude_type_id, r.conclude_predicate_id, r.conclude_value,
                ct.iri, ct.key
           FROM attribute_rules r
           LEFT JOIN entity_types ct ON ct.id = r.conclude_type_id
          WHERE r.kb_id = $1 AND r.enabled",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // 归类结论要落在内建 `is_a` 上。规则存在就意味着它已经被建出来了
    // （建规则那一步负责），这里取不到就说明库被手改过——跳过而不是造一个
    let is_a: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM relation_types WHERE kb_id = $1 AND key = 'is_a'")
            .bind(kb_id)
            .fetch_optional(pool)
            .await?;

    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    let conds: Vec<(Uuid, Uuid, String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT rule_id, predicate_id, op, operand
           FROM attribute_rule_conditions
          WHERE rule_id = ANY($1)
          ORDER BY rule_id, seq",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let mut by_rule: HashMap<Uuid, Vec<Condition>> = HashMap::new();
    let mut broken: HashSet<Uuid> = HashSet::new();
    for (rule_id, predicate, op, operand) in conds {
        let Some(op) = Op::parse(&op) else {
            broken.insert(rule_id);
            continue;
        };
        let Some(operand) = parse_operand(op, operand.as_ref()) else {
            broken.insert(rule_id);
            continue;
        };
        by_rule.entry(rule_id).or_default().push(Condition {
            predicate,
            op,
            operand,
        });
    }

    let mut out = Vec::new();
    for (id, subject_type, conclusion, conclude_type, conclude_pred, conclude_value, iri, key) in
        rows
    {
        if broken.contains(&id) {
            continue;
        }
        let Some(conditions) = by_rule.remove(&id) else {
            // 一条没有条件的规则什么都不推（求值器也这么判），不必往下走
            continue;
        };
        let (conclusion, predicate) = match conclusion.as_str() {
            "typing" => {
                let (Some(_), Some((is_a,))) = (conclude_type, is_a) else {
                    continue;
                };
                // **按 IRI 记，没有 IRI 才退回 key**：改标签不该让已推出的结论
                // 变成另一条（0021 决策 2）
                let Some(class) = iri.or(key) else { continue };
                (Conclusion::Typing { class }, is_a)
            }
            "attribute" => {
                let (Some(p), Some(v)) = (conclude_pred, conclude_value) else {
                    continue;
                };
                (
                    Conclusion::Attribute {
                        predicate: p,
                        value: v,
                    },
                    p,
                )
            }
            _ => continue,
        };
        let subject_types = descendants_of(pool, kb_id, subject_type).await?;
        out.push(LoadedRule {
            rule: BusinessRule {
                id,
                conclusion,
                conditions,
            },
            subject_types,
            conclude_predicate: predicate,
        });
    }
    Ok(out)
}

/// 操作数按 op 解析。形状不对返回 None，调用方整条规则跳过。
fn parse_operand(
    op: utopia_reason::rules::Op,
    raw: Option<&serde_json::Value>,
) -> Option<utopia_reason::rules::Operand> {
    use utopia_reason::rules::{Op, Operand};
    match op {
        Op::Present => Some(Operand::None),
        Op::Between => {
            let arr = raw?.as_array()?;
            let (lo, hi) = (arr.first()?.as_f64()?, arr.get(1)?.as_f64()?);
            Some(Operand::Range(lo.min(hi), lo.max(hi)))
        }
        Op::In => {
            let arr = raw?.as_array()?;
            let set: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => s.trim().to_string(),
                    other => other.to_string(),
                })
                .collect();
            (!set.is_empty()).then_some(Operand::Set(set))
        }
        _ => Some(Operand::Num(raw?.as_f64().or_else(|| {
            raw.and_then(|v| v.as_str())
                .and_then(|s| s.trim().parse().ok())
        })?)),
    }
}

/// 一个类连同它的全部子类。规则写在 `Well` 上，`HorizontalWell` 的实体也该被看。
async fn descendants_of(pool: &PgPool, kb_id: Uuid, root: Uuid) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "WITH RECURSIVE sub AS (
             SELECT id FROM entity_types WHERE id = $2 AND kb_id = $1
             UNION
             SELECT p.child_id FROM entity_type_parents p JOIN sub ON p.parent_id = sub.id
         )
         SELECT id FROM sub",
    )
    .bind(kb_id)
    .bind(root)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 属性事实查询回来的一行：事实、主语、谓词、字面值、区间两端与精度、置信度，
/// 以及主语当下的断言类型（规则要按主类过滤）
type AttrFactRow = (
    Uuid,
    Uuid,
    Uuid,
    serde_json::Value,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
    Option<String>,
    f32,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
);

/// 属性事实：字面值通道上的活事实，连同区间与精度。
///
/// 与 `timed_edges` 是对偶的一份——那边取 `object_id IS NOT NULL` 的边，
/// 这边取 `object_value IS NOT NULL` 的字面值。两边都只看断言。
async fn attribute_facts(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<(
    Vec<utopia_reason::rules::AttrFact>,
    HashMap<Uuid, (Option<i64>, Option<i64>)>,
    HashMap<Uuid, PremiseMeta>,
    HashMap<Uuid, Option<Uuid>>,
)> {
    let rows: Vec<AttrFactRow> = sqlx::query_as(
        "SELECT f.id, f.subject_id, f.predicate_id, f.object_value,
                f.valid_from, f.valid_to, f.valid_from_precision, f.valid_to_precision,
                f.confidence, e.type_id, f.attested_at
           FROM facts f
           JOIN entities e ON e.id = f.subject_id
          WHERE f.kb_id = $1
            AND f.invalidated_at IS NULL
            AND f.predicate_id IS NOT NULL
            AND f.object_value IS NOT NULL
            AND e.merged_into IS NULL",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;

    let mut facts = Vec::with_capacity(rows.len());
    let mut spans = HashMap::new();
    let mut meta = HashMap::new();
    let mut type_of = HashMap::new();
    for (id, subject, predicate, value, from, to, fp, tp, conf, type_id, attested) in rows {
        // 与公理那一路同一种读法（0022）：读数没日期就从它的文档起算
        let (f, t, fa, ta) = read_span(from, to, tp.as_deref(), attested);
        // 属性事实的字面值是 `{"value": …, "unit": …}`；比较的是里面那个 value。
        // 取不到就把整个对象交给求值器——它对认不出的形状一律判不满足
        let inner = value.get("value").cloned().unwrap_or_else(|| value.clone());
        facts.push(utopia_reason::rules::AttrFact {
            id,
            subject,
            predicate,
            value: inner,
        });
        spans.insert(id, (f, t));
        meta.insert(id, (fp, tp, conf, fa, ta));
        type_of.insert(subject, type_id);
    }
    Ok((facts, spans, meta, type_of))
}

/// 推一遍，把派生事实落进账本。
///
/// **调用方负责检查 `materialize_inferences` 开关。** 这一层不判——它也被
/// 「预览一下会推出什么」那条路用，而预览不该受开关约束。
pub async fn materialize(pool: &PgPool, kb_id: Uuid) -> AppResult<DeriveReport> {
    let ax = axioms(pool, kb_id).await?;
    let rules = compile_rules(pool, kb_id, &ax).await?;
    let (edges, mut spans, mut meta) = timed_edges(pool, kb_id).await?;

    let derivation = utopia_reason::derive::derive(&edges, &ax);
    // asserted > derived 是硬性的（0002）：撞上断言的派生不落地。人认可过并存的
    // 除外；派生之间互撞的两边都不落，认可与否只影响报不报（0017）
    let clashes = utopia_reason::derive::contradictions(&derivation, &edges, &ax, &spans);
    let accepted = accepted_clashes(pool, kb_id).await?;
    let mut blocked: HashSet<usize> = HashSet::new();
    for c in &clashes.with_assertions {
        let d = &derivation.facts[c.derived];
        if !accepted.contains(&(d.subject, d.predicate, d.object, c.against)) {
            blocked.insert(c.derived);
        }
    }
    for rc in &clashes.between_derivations {
        for (i, j) in &rc.pairs {
            blocked.insert(*i);
            blocked.insert(*j);
        }
    }

    let mut report = DeriveReport {
        rules: rules.len(),
        edges: edges.len(),
        derived: derivation.facts.len(),
        capped: derivation.capped.len(),
        blocked: blocked.len(),
        ..Default::default()
    };

    let mut wanted: HashMap<DerivedKey, Wanted> = HashMap::new();
    for (i, d) in derivation.facts.iter().enumerate() {
        if blocked.contains(&i) {
            continue;
        }
        let Some((from, to)) = utopia_reason::derive::validity(&d.premises, &spans) else {
            continue;
        };
        // **按 `via` 查，不是 `predicate`。** 规则行是给「声明了公理的那个
        // 谓词」编的；跨谓词的两条规则里，派生出来的谓词是另一个
        let Some(&rule_id) = rules.get(&(d.via, d.rule.as_str())) else {
            // 查不到规则是**编译与推导不一致**，不是正常情况。数出来，
            // 别再让它静默消失一次
            report.unruled += 1;
            continue;
        };
        wanted.insert(
            (d.subject, d.predicate, Some(d.object), None, from, to),
            Wanted {
                subject: d.subject,
                predicate: d.predicate,
                object_id: Some(d.object),
                object_value: None,
                from,
                to,
                premises: d.premises.clone(),
                rule_id: Some(rule_id),
                attribute_rule_id: None,
            },
        );
    }

    // 第二趟：属性事实上的业务规则（0021）。**并进同一个 `wanted`**——
    // 下面的陈旧对账扫的是整张 `derived_facts`，两趟各做各的 diff 会把对方
    // 落的行每一轮都判成陈旧
    let loaded = attribute_rules(pool, kb_id).await?;
    report.attribute_rules = loaded.len();
    if !loaded.is_empty() {
        let (attr_facts, attr_spans, attr_meta, type_of) = attribute_facts(pool, kb_id).await?;
        for lr in &loaded {
            // 规则只看自己主类（含子类）的实体
            let scoped: Vec<utopia_reason::rules::AttrFact> = attr_facts
                .iter()
                .filter(|f| {
                    type_of
                        .get(&f.subject)
                        .and_then(|t| *t)
                        .is_some_and(|t| lr.subject_types.contains(&t))
                })
                .cloned()
                .collect();
            let (hits, rr) = utopia_reason::rules::evaluate(
                std::slice::from_ref(&lr.rule),
                &scoped,
                &attr_spans,
            );
            report.rule_hits += rr.hits;
            // 展不完的组合数按规则写回：这个数字在卡片上常驻，而不只在
            // 「跑完那一刻」的提示里闪一下（少推几条与「不满足」长得一样）
            sqlx::query("UPDATE attribute_rules SET capped_at_last_run = $2 WHERE id = $1")
                .bind(lr.rule.id)
                .bind(rr.capped as i32)
                .execute(pool)
                .await?;
            //  是「这一轮算出来的派生总数」，公理与规则都算在内。
            // **别改写它的原义**：被拦下的那些也算「算出来了」，队列里那一行
            // 正是凭它对上的
            report.derived += rr.hits;
            report.rule_capped += rr.capped;
            for h in hits {
                let value = match &lr.rule.conclusion {
                    utopia_reason::rules::Conclusion::Typing { class } => {
                        serde_json::json!({ "class": class })
                    }
                    utopia_reason::rules::Conclusion::Attribute { value, .. } => {
                        serde_json::json!({ "value": value })
                    }
                };
                let key = (
                    h.subject,
                    lr.conclude_predicate,
                    None,
                    value_key(Some(&value)),
                    h.from,
                    h.to,
                );
                wanted.entry(key).or_insert(Wanted {
                    subject: h.subject,
                    predicate: lr.conclude_predicate,
                    object_id: None,
                    object_value: Some(value),
                    from: h.from,
                    to: h.to,
                    premises: h.premises,
                    rule_id: None,
                    attribute_rule_id: Some(lr.rule.id),
                });
            }
        }
        // 前提的精度与置信度：两趟共用下面那段，所以两份 meta 也要合起来；
        // 区间也要——落地时要对着前提的区间认出派生的哪一端是锚点顶上的
        meta.extend(attr_meta);
        spans.extend(attr_spans);
    }

    let mut tx = pool.begin().await?;
    let live: Vec<LiveRow> = sqlx::query_as(
        "SELECT id, subject_id, predicate_id, object_id, object_value, valid_from, valid_to
           FROM derived_facts
          WHERE kb_id = $1 AND invalidated_at IS NULL",
    )
    .bind(kb_id)
    .fetch_all(&mut *tx)
    .await?;

    let mut stale: Vec<Uuid> = Vec::new();
    for (id, s, p, o, ov, from, to) in &live {
        let key = (
            *s,
            *p,
            *o,
            value_key(ov.as_ref()),
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

    for (_, d) in wanted {
        // 精度与置信度都取前提里最保守的那一个
        let mut fp: Option<String> = None;
        let mut tp: Option<String> = None;
        let mut conf = 1.0f32;
        // 派生的那一端是不是某条前提的**锚点**顶上来的（0022）：是，就没有精度可言
        let mut from_anchored = false;
        let mut to_anchored = false;
        for p in &d.premises {
            if let Some((pf, pt, pc, fa, ta)) = meta.get(p) {
                fp = coarsest(fp.as_deref(), pf.as_deref());
                // 'unknown' 不是粒度，是「结束了不知哪天」的标记；它顶上来的那一端
                // 走下面 to_anchored 那条路，不进粒度的比较
                tp = coarsest(
                    tp.as_deref(),
                    pt.as_deref().filter(|p| *p != crate::graph::ENDED_UNKNOWN),
                );
                conf = conf.min(*pc);
                if let Some((sf, st)) = spans.get(p) {
                    from_anchored |= *fa && *sf == d.from;
                    to_anchored |= *ta && *st == d.to;
                }
            }
        }
        // 约束是「有精度必有日期」（0022 放宽了反向）：交集把某一端算成无界时，那一端
        // 的精度清掉；那一端若来自证据日期而不是原文的日期，也没有精度——在无知的地方
        // 填一个确定的值，正是 `facts.valid_from_precision` 那条注释说的病
        let fp = if from_anchored { None } else { d.from.and(fp) };
        let tp = if to_anchored { None } else { d.to.and(tp) };
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id,
                                        object_value, valid_from, valid_to,
                                        valid_from_precision, valid_to_precision,
                                        confidence, rule_id, attribute_rule_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(id)
        .bind(kb_id)
        .bind(d.subject)
        .bind(d.predicate)
        .bind(d.object_id)
        .bind(&d.object_value)
        .bind(d.from.map(stamp))
        .bind(d.to.map(stamp))
        .bind(&fp)
        .bind(&tp)
        .bind(conf)
        .bind(d.rule_id)
        .bind(d.attribute_rule_id)
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
        "SELECT d.id, d.kind, d.detail,
                COALESCE(st.label, sr.label) AS subject_label,
                COALESCE(ot.label, orr.label) AS other_label,
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
           LEFT JOIN relation_types orr ON orr.id = d.other
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
    let premises: Vec<Uuid> = sqlx::query_scalar(
        "SELECT premise_fact_id FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
    )
    .bind(derived_id)
    .fetch_all(pool)
    .await?;
    let steps = steps_for(pool, &premises).await?;
    Ok(Some(utopia_core::models::Proof { derived, steps }))
}

/// 一串前提展开成证明的步：三元组、区间、撤没撤、证据。
///
/// 落了地的派生（`fact_derivations`）与没落地的（`axiom_violations.path`）都从这里
/// 走——前提是同一种东西，证明链没有理由长两个样
async fn steps_for(
    pool: &PgPool,
    premises: &[Uuid],
) -> AppResult<Vec<utopia_core::models::ProofStep>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        i64,
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
        "SELECT x.ord - 1, f.id, f.subject_id, s.canonical_name,
                f.predicate_id, r.label, f.object_id, o.canonical_name,
                f.valid_from, f.valid_to, f.confidence,
                f.invalidated_at IS NOT NULL
           FROM unnest($1::uuid[]) WITH ORDINALITY AS x(id, ord)
           JOIN facts f ON f.id = x.id
           JOIN entities s ON s.id = f.subject_id
           LEFT JOIN relation_types r ON r.id = f.predicate_id
           LEFT JOIN entities o ON o.id = f.object_id
          ORDER BY x.ord",
    )
    .bind(premises)
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
            seq: seq as i32,
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
    Ok(steps)
}

/// 没落地的派生里，与这个实体有关的那些（0017 §3）——面板「推出来的」一档的
/// 「没落地的」小节。
pub async fn blocked_for_entity(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
) -> AppResult<Vec<utopia_core::models::BlockedDerivation>> {
    Ok(sqlx::query_as(
        "SELECT v.id AS violation_id,
                (v.detail->>'subject_id')::uuid AS subject_id,
                COALESCE(v.detail->>'subject', '?') AS subject,
                (v.detail->>'object_id')::uuid AS object_id,
                COALESCE(v.detail->>'object', '?') AS object,
                COALESCE(v.detail->>'predicate', '?') AS predicate,
                COALESCE(v.detail->>'rule', '?') AS rule,
                COALESCE(v.detail->>'via_label', '?') AS via_label,
                (v.detail->>'valid_from')::timestamptz AS valid_from,
                (v.detail->>'valid_to')::timestamptz AS valid_to,
                v.left_fact AS against_fact,
                s.canonical_name || ' · '
                  || COALESCE(r.label, fact_surface_predicate(f.id), '?') || ' · '
                  || COALESCE(o.canonical_name, '?') AS against_text,
                v.path AS premises
           FROM axiom_violations v
           JOIN facts f ON f.id = v.left_fact
           JOIN entities s ON s.id = f.subject_id
           LEFT JOIN relation_types r ON r.id = f.predicate_id
           LEFT JOIN entities o ON o.id = f.object_id
          WHERE v.kb_id = $1 AND v.kind = 'derived_contradiction' AND v.status = 'open'
            AND (v.detail->>'subject_id' = $2::text OR v.detail->>'object_id' = $2::text)
          ORDER BY v.detected_at DESC",
    )
    .bind(kb_id)
    .bind(entity_id)
    .fetch_all(pool)
    .await?)
}

/// 没落地的派生的证明链：它的前提就在违规的 `path` 里。找不到那条违规时 `None`
pub async fn blocked_proof(
    pool: &PgPool,
    kb_id: Uuid,
    violation_id: Uuid,
) -> AppResult<Option<Vec<utopia_core::models::ProofStep>>> {
    let path: Option<(Vec<Uuid>,)> = sqlx::query_as(
        "SELECT path FROM axiom_violations
          WHERE id = $1 AND kb_id = $2 AND kind = 'derived_contradiction'",
    )
    .bind(violation_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    match path {
        None => Ok(None),
        Some((p,)) => Ok(Some(steps_for(pool, &p).await?)),
    }
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
                d.object_id,
                COALESCE(o.canonical_name, ct.label,
                         d.object_value ->> 'class',
                         d.object_value #>> '{value}',
                         d.object_value #>> '{}') AS object,
                r.label AS predicate,
                COALESCE(ru.kind, 'business') AS rule,
                ar.name AS rule_name,
                d.valid_from, d.valid_to, d.confidence, d.derived_at,
                COALESCE(
                    (SELECT array_agg(
                                ps.canonical_name || ' · '
                                || COALESCE(pr.label, '?') || ' · '
                                || COALESCE(po.canonical_name,
                                            pf.object_value #>> '{value}',
                                            '?')
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
           LEFT JOIN entities o ON o.id = d.object_id
           JOIN relation_types r ON r.id = d.predicate_id
           LEFT JOIN rules ru ON ru.id = d.rule_id
           LEFT JOIN attribute_rules ar ON ar.id = d.attribute_rule_id
           LEFT JOIN entity_types ct ON ct.id = ar.conclude_type_id
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
    at: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<DerivedFactView>> {
    // **宾语与规则两侧都是 LEFT JOIN。** 表拓宽之后（0021）一条派生的宾语可能
    // 是字面值而不是实体，规则可能是业务规则而不是公理——内连接会把这两种
    // 结论**静默地**从面板上抹掉，而它们恰恰是最需要解释的那种。
    //
    // 宾语的显示文本因此有三个来源：实体名、归类结论里的类标签、属性结论的值。
    Ok(sqlx::query_as(&format!(
        "SELECT d.id,
                d.subject_id, s.canonical_name AS subject,
                d.object_id,
                COALESCE(o.canonical_name,
                         ct.label,
                         d.object_value ->> 'class',
                         d.object_value #>> '{{value}}',
                         d.object_value #>> '{{}}') AS object,
                r.label AS predicate,
                COALESCE(ru.kind, 'business') AS rule,
                ar.name AS rule_name,
                d.valid_from, d.valid_to, d.confidence, d.derived_at,
                COALESCE(
                    (SELECT array_agg(
                                ps.canonical_name || ' · '
                                || COALESCE(pr.label, '?') || ' · '
                                || COALESCE(po.canonical_name,
                                            pf.object_value #>> '{{value}}',
                                            '?')
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
           LEFT JOIN entities o ON o.id = d.object_id
           JOIN relation_types r ON r.id = d.predicate_id
           LEFT JOIN rules ru ON ru.id = d.rule_id
           LEFT JOIN attribute_rules ar ON ar.id = d.attribute_rule_id
           LEFT JOIN entity_types ct ON ct.id = ar.conclude_type_id
          WHERE d.kb_id = $1 AND d.invalidated_at IS NULL
            AND (d.subject_id = $2 OR d.object_id = $2)
            AND {derived_hold}
          ORDER BY d.derived_at DESC",
        derived_hold = crate::world_axis::derived_hold_at("d", 3),
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(at)
    .fetch_all(pool)
    .await?)
}
