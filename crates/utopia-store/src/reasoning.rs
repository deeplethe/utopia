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
use utopia_reason::{check_all, Axioms, Edge, Kind, Violation};
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
    /// 环没搜完的谓词数（#642）。不为零时这些谓词上的环只报了一部分，而且上一轮的
    /// 环这一轮不清——没搜完不能说没有
    pub cycles_capped: usize,
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
             ON CONFLICT (kb_id, kind, left_fact, right_fact) WHERE kind <> 'cycle' DO NOTHING
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
    let axioms = axioms(pool, kb_id).await?;
    // 带着区间查：互斥的三类只在同时成立时才算（#634）。从前这里把区间剥掉再查，
    // 每一次调薪、每一次换负责人都进了 Review
    let checked = check_all(&timed, &axioms);
    let cycles_capped = checked.cycles_capped;
    let mut violations = checked.violations;
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
        edges: timed.len(),
        predicates_with_axioms: axioms.len(),
        found: violations.len(),
        contradictions: details.len(),
        contradictions_capped,
        rules_disagree: clashes.between_derivations.len(),
        cycles_capped: cycles_capped.len(),
        ..Default::default()
    };

    let accepted = accepted_groups(pool, kb_id).await?;

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
        let grouped = is_grouped(*kind);
        // 人认可过「这几条可以并存」，这一轮的组又全在那几条里——比如其中一条后来
        // 撤了，剩下的组首尾变了、键也变了——仍然是那句认可管着，不再端上来。
        // 组里有一条认可时没见过的，就不在这里拦：那是新情况
        if grouped
            && accepted
                .iter()
                .any(|(k, facts)| k == kind.as_str() && path.iter().all(|f| facts.contains(f)))
        {
            continue;
        }
        let detail = details
            .get(&(*left, *right))
            .cloned()
            .unwrap_or_else(|| json!({}));
        // **证据跟着这一轮走**（#619）。从前这里是 `DO NOTHING`，而除此之外没有任何
        // 地方写 `path` / `detail`——重开那一支也不写。于是一行只要不被删，它带的
        // 证据就永远是**头一次**记下这个键时算出来的那一份：同一处派生矛盾后来经由另一组
        // 前提再被算出来，行上写的还是最早那组。重开时更难看，`detected_at`
        // 换成 now() 而 path 不动，一行上于是一个新时间戳配一份别的轮次的证据。
        //
        // **环不走首尾两条的键**（#641）。两个环共用最小的那条和收尾那条、中间不同，
        // 从前落在同一个键上，后一个覆盖前一个的 path——那不是「同一个环换了证据」，
        // 是另一个环被吞掉了。环就是它的事实集合，按整条 path 定键（0054）。
        //
        // 插与更新合成一条语句：从前是插一次再 SELECT 一次，两条语句问的是同一行。
        // `xmax = 0` 只有真正新插的行才成立——`DO UPDATE` 会让 RETURNING 对更新也
        // 出一行，不这么分就会把「本来就在」数成「新插的」。
        //
        // 无论新插的还是本来就在的，都算「这一轮仍然成立」。
        // 本来就在而且 resolved 的要再看一眼：`fact_retracted` / `fact_closed` /
        // `axiom_relaxed` 都是「世界会变」的承诺——事实没了、区间闭了、公理放宽了，
        // 违规就不该再算出来。又算出来了，承诺就是没兑现，那行回到 open，人再看一次。
        // `accepted` 是有意并存，重算多少次都沉默（#202）——只要还是认可时那几条。
        // 互斥的组走到这里说明上面没拦住：同一个键下多了一条，那行也回到 open
        // 两条部分唯一索引各管一类，冲突目标要把索引的 WHERE 原样写出来才推断得到
        let upsert = if *kind == Kind::Cycle {
            "INSERT INTO axiom_violations
                 (id, kb_id, kind, left_fact, right_fact, path, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (kb_id, path) WHERE kind = 'cycle' DO UPDATE
                 SET path = EXCLUDED.path, detail = EXCLUDED.detail
             RETURNING id, status, resolution, xmax = 0"
        } else {
            "INSERT INTO axiom_violations
                 (id, kb_id, kind, left_fact, right_fact, path, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (kb_id, kind, left_fact, right_fact) WHERE kind <> 'cycle' DO UPDATE
                 SET path = EXCLUDED.path, detail = EXCLUDED.detail
             RETURNING id, status, resolution, xmax = 0"
        };
        let (keep, status, resolution, is_new): (Uuid, String, Option<String>, bool) =
            sqlx::query_as(upsert)
                .bind(Uuid::now_v7())
                .bind(kb_id)
                .bind(kind.as_str())
                .bind(left)
                .bind(right)
                .bind(path)
                .bind(&detail)
                .fetch_one(&mut *tx)
                .await?;
        if is_new {
            report.inserted += 1;
        }
        let broken = match resolution.as_deref() {
            Some("fact_retracted" | "fact_closed" | "axiom_relaxed") => true,
            Some("accepted") => grouped,
            _ => false,
        };
        if status == "resolved" && broken {
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

    // 环没搜完的谓词，上一轮的 open 环这一轮不清（#642）：没搜到不等于不存在。
    // 撞上上限时报出的是按数据排定的那一批，数据不变就是同一批；数据变了，旧的那几行
    // 也宁可留着等人看，不能因为这一轮搜不到就当它没了
    if !cycles_capped.is_empty() {
        let kept: Vec<Uuid> = sqlx::query_scalar(
            "SELECT v.id FROM axiom_violations v
               JOIN facts f ON f.id = v.left_fact
              WHERE v.kb_id = $1 AND v.kind = 'cycle' AND v.status = 'open'
                AND f.predicate_id = ANY($2)",
        )
        .bind(kb_id)
        .bind(&cycles_capped)
        .fetch_all(&mut *tx)
        .await?;
        fresh.extend(kept);
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

/// 互斥的三类：一处违规是一组同时成立、彼此冲突的事实（`path` 是整组）。
fn is_grouped(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Asymmetry | Kind::Functional | Kind::InverseFunctional
    )
}

/// 人认可过并存的互斥组：(种类, 组里的事实)。
///
/// 认可说的是「这几条可以同时成立」，所以按事实集合比，不按键比——组里撤掉一条，
/// 剩下的首尾换了、键也换了，那句认可照样管着它们（#624）。
async fn accepted_groups(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<(String, HashSet<Uuid>)>> {
    let rows: Vec<(String, Uuid, Uuid, Vec<Uuid>)> = sqlx::query_as(
        "SELECT kind, left_fact, right_fact, path FROM axiom_violations
          WHERE kb_id = $1 AND status = 'resolved' AND resolution = 'accepted'
            AND kind IN ('asymmetry', 'functional', 'inverse_functional')",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(kind, left, right, path)| {
            let mut facts: HashSet<Uuid> = path.into_iter().collect();
            // 首尾本来就在 path 里；0053 之前记下的一对 path 是空的，靠这两列
            facts.insert(left);
            facts.insert(right);
            (kind, facts)
        })
        .collect())
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
          -- id 做第二键（#646）：一轮检查插下的行 detected_at 全都相同，只按它排，
          -- 翻页时每一页的先后可以不同——有的行出现两次，有的一次也不出现
          ORDER BY v.detected_at DESC, v.id DESC
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
    /// 业务规则跑了几轮（0030）。一轮的结论进下一轮的输入，直到某一轮不再
    /// 产出新的结论。链上一环一轮，所以 1 就是「没有链」，2 就是 `A → B`
    pub rule_rounds: usize,
    /// 跑满 `MAX_DEPTH` 轮还在产出：链比上限长，后面的没接上。**与「不满足」
    /// 区分开报**——没推到与不成立在结果里长得一模一样
    pub rule_rounds_capped: bool,
    /// 结论没变、证明变了、于是重写了前提链的行数（0030）。同一句话可以有
    /// 第二条依据，而对账的键里没有前提——不重写的话那一行会一直挂着上一轮
    /// 的理由，链上还可能挂着一条刚刚作废的前提
    pub reproved: usize,
}

/// 一条派生从它的前提上得到的精度与置信度（0024）。
///
/// **抽出来是因为链**（0030）：链中间那一层既要拿它算自己的两端，又要作为
/// 下一层的前提被同一段代码读一遍。落库那一处与不动点那一处各写一份的话，
/// 两处对「哪一端是锚点顶上来的」的判断迟早会长得不一样。
///
/// 精度跟着**赢下这一端的那条前提**走：派生的起点就是前提里最晚的那个起点，
/// 它的精度就是那条前提的精度。几条前提并列时取其中最粗的；那一端若是某条
/// 前提的**锚点**顶上来的（0022），没有精度可言。
fn premise_meta(
    premises: &[Uuid],
    from: Option<i64>,
    to: Option<i64>,
    spans: &HashMap<Uuid, (Option<i64>, Option<i64>)>,
    meta: &HashMap<Uuid, PremiseMeta>,
) -> PremiseMeta {
    let mut fp: Option<String> = None;
    let mut tp: Option<String> = None;
    let mut conf = 1.0f32;
    let mut from_anchored = false;
    let mut to_anchored = false;
    for p in premises {
        let Some((pf, pt, pc, fa, ta)) = meta.get(p) else {
            continue;
        };
        // 置信度取前提里最低的：一条链只和它最弱的一环一样可信
        conf = conf.min(*pc);
        let Some((sf, st)) = spans.get(p) else {
            continue;
        };
        if from.is_some() && *sf == from {
            if *fa {
                from_anchored = true;
            } else {
                fp = coarsest(fp.as_deref(), pf.as_deref());
            }
        }
        if to.is_some() && *st == to {
            if *ta {
                to_anchored = true;
            } else {
                // 'unknown' 不是粒度，是「结束了不知哪天」的标记；它顶上来的
                // 那一端是锚点，走上面那条路
                tp = coarsest(
                    tp.as_deref(),
                    pt.as_deref().filter(|p| *p != crate::graph::ENDED_UNKNOWN),
                );
            }
        }
    }
    (fp, tp, conf, from_anchored, to_anchored)
}

/// 主类靠**派生归类**才够得着的那一份规则：范围从筛子变成一条条件（0030）。
///
/// `范围 与 ((A 且 B) 或 C)` 展开就是 `(范围且A且B) 或 (范围且C)`——所以每组
/// 各加一条，析取仍然只有一层（0029）。写成条件而不是筛子，是为了让归类那条
/// 派生事实进前提：结论的区间跟着它收窄，它作废时结论也跟着退场。
fn scoped_by_conclusion(
    rule: &utopia_reason::rules::BusinessRule,
    is_a: Uuid,
    classes: &[String],
) -> utopia_reason::rules::BusinessRule {
    use utopia_reason::rules::{Condition, Op, Operand};
    let groups: std::collections::BTreeSet<i32> = rule.conditions.iter().map(|c| c.group).collect();
    let mut conditions = rule.conditions.clone();
    for group in groups {
        conditions.push(Condition {
            group,
            predicate: is_a,
            op: Op::In,
            operand: Operand::Set(classes.to_vec()),
        });
    }
    utopia_reason::rules::BusinessRule {
        id: rule.id,
        conclusion: rule.conclusion.clone(),
        conditions,
    }
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
    temporal: crate::graph::Temporal,
    from: Option<chrono::DateTime<chrono::Utc>>,
    from_precision: Option<&str>,
    to: Option<chrono::DateTime<chrono::Utc>>,
    to_precision: Option<&str>,
    attested_from: chrono::DateTime<chrono::Utc>,
    attested_to: Option<chrono::DateTime<chrono::Utc>>,
) -> (Option<i64>, Option<i64>, bool, bool) {
    use crate::graph::Temporal;
    match temporal {
        // 恒常每一刻都成立，证据日期不闸它（0031）
        Temporal::Eternal => return (None, None, false, false),
        // 事件在它命名的那个桶里成立；没日期的事件区间为空——`overlap` 对空交集不推，
        // 所以一条经过「不知何时收购」的链推不出东西，与读出侧一致（0031）。
        // 0031 之前写下的事件行终点是空的，按起点那个桶读
        Temporal::Event => {
            return match from {
                None => {
                    let a = attested_from.timestamp();
                    (Some(a), Some(a), true, true)
                }
                Some(f) => {
                    let precision = to_precision
                        .filter(|p| *p != crate::graph::ENDED_UNKNOWN)
                        .or(from_precision);
                    let end = crate::graph::bucket_end(to.unwrap_or(f), precision);
                    (Some(f.timestamp()), Some(end.timestamp()), false, false)
                }
            };
        }
        Temporal::State => {}
    }
    let (f, from_anchored) = match from {
        Some(x) => (Some(x.timestamp()), false),
        None => (Some(attested_from.timestamp()), true),
    };
    // 结束未知的行按 CHECK 必带 attested_to；万一没有，按开放读——宁可多推一点，
    // 也不凭空造一个终点
    let (t, to_anchored) = match (to, to_precision, attested_to) {
        (Some(x), _, _) => (Some(x.timestamp()), false),
        (None, Some(p), Some(a)) if p == crate::graph::ENDED_UNKNOWN => (Some(a.timestamp()), true),
        (None, _, _) => (None, false),
    };
    (f, t, from_anchored, to_anchored)
}

async fn timed_edges(pool: &PgPool, kb_id: Uuid) -> AppResult<TimedEdges> {
    // 输入**只有断言**。派生住在另一张表，所以这里连过滤都不必写——那正是
    // 分表买到的东西：忘了排除的后果是推不出东西，不是把自己的输出喂回自己
    let rows: Vec<EdgeRow> = sqlx::query_as(
        "SELECT id, predicate_id, subject_id, object_id,
                valid_from, valid_to, valid_from_precision, valid_to_precision, confidence,
                attested_from, attested_to
           FROM facts
          WHERE kb_id = $1
            AND invalidated_at IS NULL
            AND predicate_id IS NOT NULL
            AND object_id IS NOT NULL",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    // 谓词的时间语义（0031）：事件按它的桶读，恒常两端开放。一次取全，按谓词查
    let temporal_of: HashMap<Uuid, crate::graph::Temporal> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, temporal FROM relation_types WHERE kb_id = $1",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|(id, t)| (id, crate::graph::Temporal::parse(&t)))
    .collect();

    let mut edges = Vec::with_capacity(rows.len());
    let mut meta: HashMap<Uuid, PremiseMeta> = HashMap::new();
    let mut spans: HashMap<Uuid, (Option<i64>, Option<i64>)> = HashMap::new();
    for (id, pred, subj, obj, from, to, fp, tp, conf, attested_from, attested_to) in rows {
        // 按读出来的区间推（0022）：没起点的前提从最早的证据起，结束了不知哪天的
        // 到说出它的那份文档为止。读成开放的话，一条经过 "former CEO" 的链会推出
        // 一条今天还成立的边
        let temporal = temporal_of.get(&pred).copied().unwrap_or_default();
        let (f, t, fa, ta) = read_span(
            temporal,
            from,
            fp.as_deref(),
            to,
            tp.as_deref(),
            attested_from,
            attested_to,
        );
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
/// 一条前提在 `fact_derivations` 上的两格（0030）：断言一格、派生一格，
/// 恰好一个有值——数据库那条 CHECK 说的就是这句
type PremiseCols = (Option<Uuid>, Option<Uuid>);

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
    // 梯子的顺序（0024）：year 最粗，second 最细；不认识的当最细，免得盖过认识的
    let rank = |p: &str| {
        crate::graph::WORLD_PRECISIONS
            .iter()
            .position(|w| *w == p)
            .unwrap_or(crate::graph::WORLD_PRECISIONS.len())
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
    Option<chrono::DateTime<chrono::Utc>>,
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
    /// 上面那几个类的名字（IRI，没有才 key）。派生归类记的是名字，主类范围记的
    /// 是 id——链要接上就得两边都有一份（0030）
    subject_classes: Vec<String>,
    /// 结论落在哪个谓词上：归类落 `is_a`，属性落它自己那个
    conclude_predicate: Uuid,
}

/// 编译出来的一批规则，外加接链要用的两样东西（0030）。
struct LoadedRules {
    rules: Vec<LoadedRule>,
    /// 内建 `is_a`。派生归类落在它上面，链上再读回来也从它上面读
    is_a: Option<Uuid>,
    /// 类名（IRI，没有才 key）→ 类 id。一条派生归类要变成「这个实体属于哪个类」，
    /// 必须走这一步
    class_ids: HashMap<String, Uuid>,
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
    // 算出来的结论那棵树（0032）
    Option<serde_json::Value>,
    Option<String>,
    Option<String>,
);

/// 取业务规则。条件形状不合法的规则**整条跳过而不是报错退出**——一条写坏的
/// 规则不该让整轮物化停摆，而它不产出这件事在报告的条数里看得见。
async fn attribute_rules(pool: &PgPool, kb_id: Uuid) -> AppResult<LoadedRules> {
    use utopia_reason::rules::{BusinessRule, Conclusion, Condition, Op};

    // **按 id 排序**：规则之间撞上同一个结论时留下的是先到的那条证明，而
    // 「先到」不该由 HashMap 的顺序决定——同一个库两次物化要给出同一份证明
    // （0029 在组之间讲的是同一件事，0030 让链把它放大了）
    let rows: Vec<RuleDefRow> = sqlx::query_as(
        "SELECT r.id, r.subject_type_id, r.conclusion,
                r.conclude_type_id, r.conclude_predicate_id, r.conclude_value,
                r.conclude_expr, ct.iri, ct.key
           FROM attribute_rules r
           LEFT JOIN entity_types ct ON ct.id = r.conclude_type_id
          WHERE r.kb_id = $1 AND r.enabled
          ORDER BY r.id",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(LoadedRules {
            rules: Vec::new(),
            is_a: None,
            class_ids: HashMap::new(),
        });
    }

    // 类名 ↔ 类 id，两个方向各一份：归类结论按名字记（改标签不该让已推出的结论
    // 变成另一条，0021 决策 2），而主类范围与实体的类都是 id
    let type_rows: Vec<(Uuid, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id, iri, key FROM entity_types WHERE kb_id = $1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?;
    let mut class_ids: HashMap<String, Uuid> = HashMap::new();
    let mut class_name: HashMap<Uuid, String> = HashMap::new();
    for (id, iri, key) in type_rows {
        let Some(name) = iri.or(key) else { continue };
        class_ids.insert(name.clone(), id);
        class_name.insert(id, name);
    }

    // 归类结论要落在内建 `is_a` 上。规则存在就意味着它已经被建出来了
    // （建规则那一步负责），这里取不到就说明库被手改过——跳过而不是造一个
    let is_a: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM relation_types WHERE kb_id = $1 AND key = 'is_a'")
            .bind(kb_id)
            .fetch_optional(pool)
            .await?;

    let ids: Vec<Uuid> = rows.iter().map(|r| r.0).collect();
    // 组序在前：两组推出同一区间时，留下的证明得是稳定的那一条（0029）
    let conds: Vec<(Uuid, i32, Uuid, String, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT rule_id, group_seq, predicate_id, op, operand
           FROM attribute_rule_conditions
          WHERE rule_id = ANY($1)
          ORDER BY rule_id, group_seq, seq",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let mut by_rule: HashMap<Uuid, Vec<Condition>> = HashMap::new();
    let mut broken: HashSet<Uuid> = HashSet::new();
    for (rule_id, group, predicate, op, operand) in conds {
        let Some(op) = Op::parse(&op) else {
            broken.insert(rule_id);
            continue;
        };
        let Some(operand) = parse_operand(op, operand.as_ref()) else {
            broken.insert(rule_id);
            continue;
        };
        by_rule.entry(rule_id).or_default().push(Condition {
            group,
            predicate,
            op,
            operand,
        });
    }

    let mut out = Vec::new();
    for (
        id,
        subject_type,
        conclusion,
        conclude_type,
        conclude_pred,
        conclude_value,
        conclude_expr,
        iri,
        key,
    ) in rows
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
            // 算出来的结论（0032）：谓词照旧，值由算式在求值时按选中的读数算
            "computed" => {
                let (Some(p), Some(e)) = (conclude_pred, conclude_expr) else {
                    continue;
                };
                let Some(expr) = parse_expr(&e, 0) else {
                    continue;
                };
                (Conclusion::Computed { predicate: p, expr }, p)
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
        let subject_classes = subject_types
            .iter()
            .filter_map(|t| class_name.get(t).cloned())
            .collect();
        out.push(LoadedRule {
            rule: BusinessRule {
                id,
                conclusion,
                conditions,
            },
            subject_types,
            subject_classes,
            conclude_predicate: predicate,
        });
    }
    Ok(LoadedRules {
        rules: out,
        is_a: is_a.map(|(id,)| id),
        class_ids,
    })
}

/// 算式的 JSON 形状 → 树（0032）。
///
/// `{"attr": "<uuid>"} | {"const": 12.5} | {"op": "sub", "l": {…}, "r": {…}}`
///
/// **认不出来返回 None**，调用方整条规则跳过——一棵读不懂的算式算不出数，
/// 而算不出数的规则不该带着半棵树去求值。深度也在这里拦：太深的树是
/// 「有人在这里写程序」的信号（0032）。
fn parse_expr(raw: &serde_json::Value, depth: usize) -> Option<utopia_reason::rules::Expr> {
    use utopia_reason::rules::{Arith, Expr, MAX_EXPR_DEPTH};
    if depth > MAX_EXPR_DEPTH {
        return None;
    }
    let obj = raw.as_object()?;
    if let Some(a) = obj.get("attr") {
        return Some(Expr::Attr(a.as_str()?.parse().ok()?));
    }
    if let Some(c) = obj.get("const") {
        let n = c.as_f64().or_else(|| c.as_str()?.trim().parse().ok())?;
        return n.is_finite().then_some(Expr::Const(n));
    }
    let op = Arith::parse(obj.get("op")?.as_str()?)?;
    Some(Expr::Arith {
        op,
        l: Box::new(parse_expr(obj.get("l")?, depth + 1)?),
        r: Box::new(parse_expr(obj.get("r")?, depth + 1)?),
    })
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
        // **In 与 NotIn 同一支。** 漏掉后者的下场不是「这个条件判错了」，是
        // `parse_operand` 返回 None、整条规则被当成写坏的跳过——一条用了
        // 「不属于」的规则从此什么都不推，而界面上它看着好好的（0029 / #476）
        Op::In | Op::NotIn => {
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
        // 门槛可以是算出来的（0032）：一个对象是算式，别的照旧是一个数。
        // 四种操作数形状互不相同——数、两元数组、字符串数组、对象——所以
        // 认得出来，不必再加一列说「这是哪一种」
        _ => {
            let raw = raw?;
            if raw.is_object() {
                return parse_expr(raw, 0).map(Operand::Calc);
            }
            Some(Operand::Num(raw.as_f64().or_else(|| {
                raw.as_str().and_then(|s| s.trim().parse().ok())
            })?))
        }
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
    Option<chrono::DateTime<chrono::Utc>>,
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
                f.confidence, e.type_id, f.attested_from, f.attested_to
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
    for (
        id,
        subject,
        predicate,
        value,
        from,
        to,
        fp,
        tp,
        conf,
        type_id,
        attested_from,
        attested_to,
    ) in rows
    {
        // 与公理那一路同一种读法（0022）：读数没日期就从它的文档起算。
        // 属性一律是状态（建属性时固定 state，界面也不给改）
        let (f, t, fa, ta) = read_span(
            crate::graph::Temporal::State,
            from,
            fp.as_deref(),
            to,
            tp.as_deref(),
            attested_from,
            attested_to,
        );
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
    report.attribute_rules = loaded.rules.len();
    // 一条规则结论的临时 id → 它最后落在哪一行。链上的前提指的是前者，
    // `fact_derivations` 要存的是后者（0030）。键是派生键，值是临时 id
    let mut provisional: HashMap<DerivedKey, Uuid> = HashMap::new();
    if !loaded.rules.is_empty() {
        let (asserted, attr_spans, attr_meta, type_of) = attribute_facts(pool, kb_id).await?;
        // 前提的精度与置信度：落地那一段与不动点这一段共用，所以两份 meta 先合起来；
        // 区间也要——认出派生的哪一端是被前提的锚点顶上来的，靠的就是它
        meta.extend(attr_meta);
        spans.extend(attr_spans);

        // ---- 不动点：这一轮的结论进下一轮的输入（0030）
        //
        // **全量重跑而不是半朴素**：链通常只有一两环，收敛靠「这一轮没产出新键」
        // 那一下，真实代价是单趟的两三倍。半朴素要维护每条规则读哪些谓词，
        // 为一个两三轮的循环换一份索引，不划算。
        //
        // 输入侧一行 `derived_facts` 都不读：反馈全在这个内存池子里，一次物化
        // 仍然是 (facts, rules, axioms) 的纯函数——0013 那条反对意见守的是这个
        let mut fact_pool = asserted;
        // 每个实体被**推**出来的类。断言的类在 `type_of` 里，两者进规则的方式
        // 不一样：断言的类没有区间，是个筛子；推出来的类有区间，得当条件
        let mut derived_types: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        let mut capped_by_rule: HashMap<Uuid, usize> = HashMap::new();
        let mut rounds = 0usize;
        for _ in 0..utopia_reason::MAX_DEPTH {
            rounds += 1;
            // 一轮之内先算完再入池：同一轮里规则读到的是上一轮结束时的池子，
            // 谁先谁后就不影响结果
            let mut fresh: Vec<(usize, utopia_reason::rules::RuleHit)> = Vec::new();
            capped_by_rule.clear();
            for (ri, lr) in loaded.rules.iter().enumerate() {
                // 主类是断言的：范围是个筛子，不带区间、不进前提
                let mut by_assertion: Vec<utopia_reason::rules::AttrFact> = Vec::new();
                // 只靠派生归类够得着的：范围变成一个条件（下面那段）
                let mut by_conclusion: Vec<utopia_reason::rules::AttrFact> = Vec::new();
                for f in &fact_pool {
                    if type_of
                        .get(&f.subject)
                        .and_then(|t| *t)
                        .is_some_and(|t| lr.subject_types.contains(&t))
                    {
                        by_assertion.push(f.clone());
                    } else if derived_types
                        .get(&f.subject)
                        .is_some_and(|ts| ts.iter().any(|t| lr.subject_types.contains(t)))
                    {
                        by_conclusion.push(f.clone());
                    }
                }
                let (hits, rr) = utopia_reason::rules::evaluate(
                    std::slice::from_ref(&lr.rule),
                    &by_assertion,
                    &spans,
                );
                let mut capped = rr.capped;
                fresh.extend(hits.into_iter().map(|h| (ri, h)));

                // 主类是推出来的那一份：**范围写成一条条件**，而不是再当筛子。
                // 归类那条派生事实因此进了前提，区间跟着它收窄（一个实体
                // 2019–2022 是 B，就不该拿 2024 的读数满足一条 B 上的规则），
                // 它作废时这条结论也就跟着退场——这一条不用另写代码
                if !by_conclusion.is_empty() {
                    if let (Some(is_a), false) = (loaded.is_a, lr.subject_classes.is_empty()) {
                        let scoped = scoped_by_conclusion(&lr.rule, is_a, &lr.subject_classes);
                        let (h2, rr2) = utopia_reason::rules::evaluate(
                            std::slice::from_ref(&scoped),
                            &by_conclusion,
                            &spans,
                        );
                        capped += rr2.capped;
                        fresh.extend(h2.into_iter().map(|h| (ri, h)));
                    }
                }
                *capped_by_rule.entry(lr.rule.id).or_default() += capped;
            }

            let before = provisional.len();
            for (ri, h) in fresh {
                let lr = &loaded.rules[ri];
                let (value, inner) = match &lr.rule.conclusion {
                    utopia_reason::rules::Conclusion::Typing { class } => (
                        serde_json::json!({ "class": class }),
                        serde_json::Value::String(class.clone()),
                    ),
                    utopia_reason::rules::Conclusion::Attribute { value, .. } => {
                        (serde_json::json!({ "value": value }), value.clone())
                    }
                    // 算出来的结论：值在命中里，**每个组合各一个**（0032）。
                    // 求值器算不出数的组合根本不会产出命中，所以这里不会没有值
                    utopia_reason::rules::Conclusion::Computed { .. } => {
                        let Some(n) = h.value else { continue };
                        let Some(v) = serde_json::Number::from_f64(n) else {
                            continue;
                        };
                        let v = serde_json::Value::Number(v);
                        (serde_json::json!({ "value": v }), v)
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
                // 上一轮已经推出过同一条：不再进池子，也就不会再进 frontier。
                // 自反馈的环正是在这里停下的——它推出的是同一个键
                if wanted.contains_key(&key) {
                    continue;
                }
                // 临时 id：这一条现在就要当事实用，可它落在哪一行要等对账之后
                // 才知道（不变的结论保留原来那一行）。落库前统一换过来
                let prov = Uuid::now_v7();
                let pm = premise_meta(&h.premises, h.from, h.to, &spans, &meta);
                spans.insert(prov, (h.from, h.to));
                meta.insert(prov, pm);
                fact_pool.push(utopia_reason::rules::AttrFact {
                    id: prov,
                    subject: h.subject,
                    predicate: lr.conclude_predicate,
                    value: inner,
                });
                if let utopia_reason::rules::Conclusion::Typing { class } = &lr.rule.conclusion {
                    if let Some(&t) = loaded.class_ids.get(class) {
                        derived_types.entry(h.subject).or_default().push(t);
                    }
                }
                provisional.insert(key.clone(), prov);
                wanted.insert(
                    key,
                    Wanted {
                        subject: h.subject,
                        predicate: lr.conclude_predicate,
                        object_id: None,
                        object_value: Some(value),
                        from: h.from,
                        to: h.to,
                        premises: h.premises,
                        rule_id: None,
                        attribute_rule_id: Some(lr.rule.id),
                    },
                );
            }
            // 这一轮什么新东西都没推出来：不动点到了
            if provisional.len() == before {
                break;
            }
        }
        report.rule_rounds = rounds;
        // 跑满了轮数还在产出：链比 MAX_DEPTH 长，后面的没接上。**得报出来**——
        // 「没推到」与「不满足」在结果里长得一模一样（组合封顶那条是同一个道理）
        report.rule_rounds_capped = rounds == utopia_reason::MAX_DEPTH;
        for lr in &loaded.rules {
            // 展不完的组合数按规则写回：这个数字在表里常驻，而不只在「跑完那一刻」
            // 的提示里闪一下。取最后一轮的数——那一轮扫的是最全的池子
            let capped = capped_by_rule.get(&lr.rule.id).copied().unwrap_or(0);
            sqlx::query("UPDATE attribute_rules SET capped_at_last_run = $2 WHERE id = $1")
                .bind(lr.rule.id)
                .bind(capped as i32)
                .execute(pool)
                .await?;
            report.rule_capped += capped;
        }
        // 命中数按**不同的结论**数，不按算出来多少次：不动点里同一条结论每轮都
        // 会被重新算出来，累加就成了轮数的函数
        report.rule_hits = provisional.len();
        report.derived += provisional.len();
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
    // 每个还成立的结论最后落在哪一行。**不变的结论保留原来那一行**，所以链上
    // 指向它的前提要指向这个 id，而不是这一轮新造的（0030）
    let mut settled: HashMap<DerivedKey, Uuid> = HashMap::new();
    // 这一轮还成立、行也留着的那些，连同它们**这一轮的**前提。对账的键里没有
    // 前提，所以「结论没变、理由变了」在这里是看不出来的——下面单独对一遍
    let mut kept: Vec<(Uuid, Wanted)> = Vec::new();
    for (id, s, p, o, ov, from, to) in &live {
        let key = (
            *s,
            *p,
            *o,
            value_key(ov.as_ref()),
            from.map(|x| x.timestamp()),
            to.map(|x| x.timestamp()),
        );
        if let Some(d) = wanted.remove(&key) {
            settled.insert(key, *id);
            kept.push((*id, d));
        } else {
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

    // 剩下的都是新行。**先把 id 全定下来再插**：链上的前提要指向的那一行，
    // 可能是这一批里还没插的另一条（0030）
    let fresh: Vec<(Uuid, Wanted)> = wanted
        .into_iter()
        .map(|(key, d)| {
            let id = Uuid::now_v7();
            settled.insert(key, id);
            (id, d)
        })
        .collect();
    // 临时 id → 真正的行 id。链上的前提在不动点里用的是临时 id，落库要换过来；
    // 断言前提不在这张表里，原样通过
    let resolve: HashMap<Uuid, Uuid> = provisional
        .iter()
        .filter_map(|(key, prov)| settled.get(key).map(|id| (*prov, *id)))
        .collect();

    for (id, d) in &fresh {
        // 约束是「有精度必有日期」（0022 放宽了反向）：交集把某一端算成无界时，那一端
        // 的精度清掉；那一端若来自证据日期而不是原文的日期，也没有精度——在无知的地方
        // 填一个确定的值，正是 `facts.valid_from_precision` 那条注释说的病
        let (fp, tp, conf, from_anchored, to_anchored) =
            premise_meta(&d.premises, d.from, d.to, &spans, &meta);
        let fp = if from_anchored { None } else { d.from.and(fp) };
        let tp = if to_anchored { None } else { d.to.and(tp) };
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
        report.inserted += 1;
    }

    // 结论没变、理由变了：**同一句话可以有第二条依据**（换了一条读数，或者换了
    // 一组条件）。对账的键是主宾谓加区间，前提不在里面，所以那一行会带着上一轮
    // 的证明留下来——链让这件事更容易撞上：站在它上面的那条前提可能刚刚作废。
    // 一次查完再逐条比，只有真不一样的才重写
    let kept_ids: Vec<Uuid> = kept.iter().map(|(id, _)| *id).collect();
    let mut stored: HashMap<Uuid, Vec<PremiseCols>> = HashMap::new();
    if !kept_ids.is_empty() {
        let rows: Vec<(Uuid, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT derived_fact_id, premise_fact_id, premise_derived_id
               FROM fact_derivations WHERE derived_fact_id = ANY($1)
              ORDER BY derived_fact_id, seq",
        )
        .bind(&kept_ids)
        .fetch_all(&mut *tx)
        .await?;
        for (d, f, dv) in rows {
            stored.entry(d).or_default().push((f, dv));
        }
    }
    let premise_cols = |d: &Wanted| -> Vec<PremiseCols> {
        d.premises
            .iter()
            .map(|p| match resolve.get(p) {
                Some(x) => (None, Some(*x)),
                None => (Some(*p), None),
            })
            .collect()
    };
    let mut reproved: Vec<(Uuid, Vec<PremiseCols>)> = Vec::new();
    for (id, d) in &kept {
        let want = premise_cols(d);
        if stored.get(id).map(|s| s.as_slice()) == Some(want.as_slice()) {
            continue;
        }
        reproved.push((*id, want));
    }
    for (id, want) in &reproved {
        sqlx::query("DELETE FROM fact_derivations WHERE derived_fact_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (seq, (f, d)) in want.iter().enumerate() {
            sqlx::query(
                "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id,
                                               premise_derived_id, seq)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(f)
            .bind(d)
            .bind(seq as i32)
            .execute(&mut *tx)
            .await?;
        }
    }
    report.reproved = reproved.len();

    // 证明**等派生全插完再写**：链上一条前提指的可能是这一批里的另一行，
    // 而这一批没有顺序可言（`wanted` 是个 HashMap）。一趟写完的话，先轮到
    // 的那一行会指着还不存在的行，外键当场拦下
    for (id, d) in &fresh {
        for (seq, premise) in d.premises.iter().enumerate() {
            // 前提是断言还是另一条派生：两列二选一，`seq` 是跨两种的一个序，
            // 证明读起来才是一条顺下来的路（0030）
            let derived_premise = resolve.get(premise).copied();
            sqlx::query(
                "INSERT INTO fact_derivations (derived_fact_id, premise_fact_id,
                                               premise_derived_id, seq)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(derived_premise.is_none().then_some(*premise))
            .bind(derived_premise)
            .bind(seq as i32)
            .execute(&mut *tx)
            .await?;
        }
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
          ORDER BY d.detected_at DESC, d.id DESC
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
/// `fact_derivations` 只记直接前提，顺着它一层层问下去就是完整的证明。一条
/// 前提要么是断言——那一步的叶子是它的原句——要么是**另一条派生**（0030），
/// 那一步要再问一次它凭什么。**撤了的前提照样列出并打上标记**：派生随前提
/// 失效，但「当时靠的是什么」要读得出来，那正是记录轴存在的理由。
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
    let steps = premise_steps(pool, derived_id, 0).await?;
    Ok(Some(utopia_core::models::Proof { derived, steps }))
}

/// 一条派生的直接前提，派生的那几步再往下展开一层（0030）。
///
/// 深度上限与推理是同一条 `MAX_DEPTH`：链最长这么长，证明也就最深这么深。
/// 到底了就停在那一步上——它自己的三元组还是列出来的，只是不再往下问。
fn premise_steps<'a>(
    pool: &'a PgPool,
    derived_id: Uuid,
    depth: usize,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = AppResult<Vec<utopia_core::models::ProofStep>>>
            + Send
            + 'a,
    >,
> {
    Box::pin(async move {
        let rows: Vec<(i32, Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT seq, premise_fact_id, premise_derived_id
               FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(derived_id)
        .fetch_all(pool)
        .await?;
        // 断言那几步一次取完（每一步还要各取一次证据），派生那几步逐条问
        let asserted: Vec<Uuid> = rows.iter().filter_map(|(_, f, _)| *f).collect();
        let mut by_fact: HashMap<Uuid, utopia_core::models::ProofStep> = steps_for(pool, &asserted)
            .await?
            .into_iter()
            .map(|s| (s.fact_id, s))
            .collect();

        let mut steps = Vec::with_capacity(rows.len());
        for (seq, fact, derived) in rows {
            if let Some(f) = fact {
                let Some(mut step) = by_fact.remove(&f) else {
                    continue;
                };
                step.seq = seq;
                steps.push(step);
            } else if let Some(d) = derived {
                let Some(mut step) = derived_step(pool, d).await? else {
                    continue;
                };
                step.seq = seq;
                if depth + 1 < utopia_reason::MAX_DEPTH {
                    step.premises = premise_steps(pool, d, depth + 1).await?;
                }
                steps.push(step);
            }
        }
        Ok(steps)
    })
}

/// 一条派生前提读成证明的一步。三元组与区间跟断言那一步同一副样子——读的人
/// 关心的是「这一句成不成立」，而不是它从哪张表来；`derived` 那一格答的是
/// 后者，也是「还能不能再往下点一层」的依据。
async fn derived_step(
    pool: &PgPool,
    id: Uuid,
) -> AppResult<Option<utopia_core::models::ProofStep>> {
    #[allow(clippy::type_complexity)]
    let row: Option<(
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
        "SELECT d.id, d.subject_id, s.canonical_name,
                d.predicate_id, r.label, d.object_id,
                COALESCE(o.canonical_name, ct.label,
                         d.object_value ->> 'class',
                         d.object_value #>> '{value}'),
                d.valid_from, d.valid_to, d.confidence,
                d.invalidated_at IS NOT NULL
           FROM derived_facts d
           JOIN entities s ON s.id = d.subject_id
           LEFT JOIN relation_types r ON r.id = d.predicate_id
           LEFT JOIN entities o ON o.id = d.object_id
           LEFT JOIN attribute_rules ar ON ar.id = d.attribute_rule_id
           LEFT JOIN entity_types ct ON ct.id = ar.conclude_type_id
          WHERE d.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(
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
        )| utopia_core::models::ProofStep {
            seq: 0,
            fact_id,
            derived: true,
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
            // 派生没有原句：它的「证据」就是下面那一层前提
            evidence: Vec::new(),
            premises: Vec::new(),
        },
    ))
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
            derived: false,
            premises: Vec::new(),
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
        "SELECT d.id, d.predicate_id, d.object_value, d.rule_id, d.attribute_rule_id,
                d.invalidated_at, d.valid_from_precision, d.valid_to_precision,
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
                                            fd.object_value #>> '{value}',
                                            fd.object_value #>> '{class}',
                                            '?')
                                ORDER BY fd.seq)
                       FROM derivation_premises fd
                       JOIN entities ps    ON ps.id = fd.subject_id
                       LEFT JOIN relation_types pr ON pr.id = fd.predicate_id
                       LEFT JOIN entities po ON po.id = fd.object_id
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
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<DerivedFactView>> {
    // 记录轴（0019 / #549）：断言那一半早就走 `held_at`，这一半曾写死
    // `invalidated_at IS NULL`——回放到三月的面板上挂着四月才推出的结论，
    // 前提一条都不在，结论却在。谓词只在 record_axis 里拼，这里不自己写。
    //
    // **宾语与规则两侧都是 LEFT JOIN。** 表拓宽之后（0021）一条派生的宾语可能
    // 是字面值而不是实体，规则可能是业务规则而不是公理——内连接会把这两种
    // 结论**静默地**从面板上抹掉，而它们恰恰是最需要解释的那种。
    //
    // 宾语的显示文本因此有三个来源：实体名、归类结论里的类标签、属性结论的值。
    Ok(sqlx::query_as(&format!(
        "SELECT d.id, d.predicate_id, d.object_value, d.rule_id, d.attribute_rule_id,
                d.invalidated_at, d.valid_from_precision, d.valid_to_precision,
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
                                            fd.object_value #>> '{{value}}',
                                            fd.object_value #>> '{{class}}',
                                            '?')
                                ORDER BY fd.seq)
                       FROM derivation_premises fd
                       JOIN entities ps    ON ps.id = fd.subject_id
                       LEFT JOIN relation_types pr ON pr.id = fd.predicate_id
                       LEFT JOIN entities po ON po.id = fd.object_id
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
          WHERE d.kb_id = $1 AND {derived_held}
            AND (d.subject_id = $2 OR d.object_id = $2)
            AND {derived_hold}
          ORDER BY d.derived_at DESC",
        derived_hold = crate::world_axis::derived_hold_at("d", 3),
        derived_held = crate::record_axis::derived_held_at("d", 4),
    ))
    .bind(kb_id)
    .bind(entity_id)
    .bind(at)
    .bind(as_of)
    .fetch_all(pool)
    .await?)
}
