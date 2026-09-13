//! 业务规则的存取（0021 / #277）。求值在 `utopia-reason::rules`，接进物化在
//! `reasoning::materialize`；这里只管「人写下来的那几条规则怎么进库、怎么取回」。
//!
//! **判据是人写的。** 这个模块没有任何一处由模型生成规则的入口——与 0002 对
//! 公理的态度同一条线。

use serde_json::json;
use sqlx::PgPool;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 内建 `is_a` 谓词：派生归类的结论落在它上面（0021 决策 2）。
///
/// **按需建，不在建库时铺。** 一个从不写规则的库不该多出一个它看不懂的谓词
/// ——与 #231 给 `metric` / `dimension` 做的选择一致。
pub async fn ensure_is_a(pool: &PgPool, kb_id: Uuid) -> AppResult<Uuid> {
    if let Some((id,)) =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM relation_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(IS_A)
            .fetch_optional(pool)
            .await?
    {
        return Ok(id);
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, builtin, description)
         VALUES ($1, $2, $3, 'is a', 'attribute', 'text', TRUE, $4)
         ON CONFLICT (kb_id, key) DO NOTHING",
    )
    .bind(id)
    .bind(kb_id)
    .bind(IS_A)
    .bind("A class a rule concluded for this entity. Derived, and it never replaces the asserted type.")
    .execute(pool)
    .await?;
    // ON CONFLICT 命中说明并发建过了，取回那一条
    let (id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM relation_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(IS_A)
            .fetch_one(pool)
            .await?;
    Ok(id)
}

pub const IS_A: &str = "is_a";

/// 列表查询回来的一行规则：id、名字、说明、主类及其标签、结论那几列、
/// 开关，以及「此刻凭它成立的结论条数」。
///
/// 起个名字而不是让它当匿名元组：这一行有十三格，读的人对不上位置
type RuleRow = (
    Uuid,
    String,
    String,
    Uuid,
    Option<String>,
    String,
    Option<Uuid>,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<serde_json::Value>,
    // 算出来的结论那棵树（0032）
    Option<serde_json::Value>,
    bool,
    i64,
    i32,
);

/// 条件查询回来的一行：规则、组号、属性谓词及其标签、比较方式、操作数
type ConditionRow = (
    Uuid,
    i32,
    Uuid,
    Option<String>,
    String,
    Option<serde_json::Value>,
);

/// 一条条件，界面与 API 共用的形状。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConditionInput {
    /// 组号：同组「与」，组间「或」（0029）。**缺省 0**——不带组的调用方
    /// 送来的就是一组合取，与从前一模一样
    #[serde(default)]
    pub group: i32,
    pub predicate_id: Uuid,
    pub op: String,
    #[serde(default)]
    pub operand: Option<serde_json::Value>,
}

/// 建一条规则。**校验在这里做完**：条件的 op 与操作数形状、谓词必须是属性、
/// 结论的两种形状各自完整——库里的 CHECK 是最后一道，报错信息却是给人看的。
#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    kb_id: Uuid,
    name: &str,
    description: &str,
    subject_type_id: Uuid,
    conclusion: &str,
    conclude_type_id: Option<Uuid>,
    conclude_predicate_id: Option<Uuid>,
    conclude_value: Option<serde_json::Value>,
    // 算出来的结论那棵树（0032）
    conclude_expr: Option<serde_json::Value>,
    conditions: &[ConditionInput],
) -> AppResult<Uuid> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::invalid(
            "bad_rule_name",
            "A rule needs a name of 1-80 characters.",
        ));
    }
    if conditions.is_empty() {
        // 空合取恒真，会把整个类归进去——挡在入口比在求值器里默默不推更早
        return Err(AppError::invalid(
            "no_conditions",
            "A rule needs at least one condition; without one it would conclude for every entity of the class.",
        ));
    }
    validate_conditions(pool, kb_id, conditions).await?;

    validate_conclusion(
        pool,
        kb_id,
        &ConclusionInput {
            kind: conclusion.to_string(),
            type_id: conclude_type_id,
            predicate_id: conclude_predicate_id,
            value: conclude_value.clone(),
            expr: conclude_expr.clone(),
        },
    )
    .await?;
    exists(
        pool,
        kb_id,
        "entity_types",
        subject_type_id,
        "unknown_class",
        "That class is not in this base.",
    )
    .await?;

    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO attribute_rules
             (id, kb_id, name, description, subject_type_id, conclusion,
              conclude_type_id, conclude_predicate_id, conclude_value, conclude_expr)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(name)
    .bind(description.trim())
    .bind(subject_type_id)
    .bind(conclusion)
    .bind(conclude_type_id)
    .bind(conclude_predicate_id)
    .bind(&conclude_value)
    .bind(&conclude_expr)
    .execute(&mut *tx)
    .await
    .map_err(|e| match e {
        sqlx::Error::Database(ref d) if d.is_unique_violation() => AppError::invalid(
            "duplicate_rule",
            "A rule with that name already exists here.",
        ),
        other => AppError::Db(other),
    })?;
    insert_conditions(&mut tx, id, conditions).await?;
    tx.commit().await?;
    Ok(id)
}

/// 改一条规则：条件整组替换。
///
/// **不做逐条的增删改**——条件是一个合取整体，替换比对着 seq 打补丁好读，
/// 也没有「改到一半」的中间状态。
/// 结论的一次改写。**两种形状各自完整地给**，不做「只改类、别的不动」——
/// 结论的三格是互相定义的，部分更新会留下 typing 却带着值这种半截状态。
pub struct ConclusionInput {
    /// typing | attribute | computed
    pub kind: String,
    pub type_id: Option<Uuid>,
    pub predicate_id: Option<Uuid>,
    pub value: Option<serde_json::Value>,
    /// 算出来的结论那棵树（0032）。`computed` 时必给，别的两支必空
    pub expr: Option<serde_json::Value>,
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    pool: &PgPool,
    kb_id: Uuid,
    rule_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    enabled: Option<bool>,
    conditions: Option<&[ConditionInput]>,
    conclusion: Option<&ConclusionInput>,
) -> AppResult<()> {
    if let Some(cs) = conditions {
        if cs.is_empty() {
            return Err(AppError::invalid(
                "no_conditions",
                "A rule needs at least one condition.",
            ));
        }
        validate_conditions(pool, kb_id, cs).await?;
    }
    if let Some(c) = conclusion {
        validate_conclusion(pool, kb_id, c).await?;
    }
    let mut tx = pool.begin().await?;
    let res = sqlx::query(
        // 结论给了就整组换（三格一起），没给就一格不动——COALESCE 在这里
        // 不够用：把 typing 改成 attribute 要把 conclude_type_id 置回 NULL，
        // 而 COALESCE 恰恰做不到「显式清空」
        "UPDATE attribute_rules
            SET name = COALESCE($3, name),
                description = COALESCE($4, description),
                enabled = COALESCE($5, enabled),
                conclusion = COALESCE($6, conclusion),
                conclude_type_id      = CASE WHEN $6 IS NULL THEN conclude_type_id      ELSE $7 END,
                conclude_predicate_id = CASE WHEN $6 IS NULL THEN conclude_predicate_id ELSE $8 END,
                conclude_value        = CASE WHEN $6 IS NULL THEN conclude_value        ELSE $9 END,
                conclude_expr         = CASE WHEN $6 IS NULL THEN conclude_expr         ELSE $10 END,
                updated_at = now()
          WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(rule_id)
    .bind(name.map(str::trim))
    .bind(description.map(str::trim))
    .bind(enabled)
    .bind(conclusion.map(|c| c.kind.as_str()))
    .bind(conclusion.and_then(|c| c.type_id))
    .bind(conclusion.and_then(|c| c.predicate_id))
    .bind(conclusion.and_then(|c| c.value.clone()))
    .bind(conclusion.and_then(|c| c.expr.clone()))
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    if let Some(cs) = conditions {
        sqlx::query("DELETE FROM attribute_rule_conditions WHERE rule_id = $1")
            .bind(rule_id)
            .execute(&mut *tx)
            .await?;
        insert_conditions(&mut tx, rule_id, cs).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 删一条规则。它推出来的派生行随 `ON DELETE CASCADE` 一起走——**规则没了，
/// 凭它得出的结论就没有依据了**，留着无从解释。
pub async fn delete(pool: &PgPool, kb_id: Uuid, rule_id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM attribute_rules WHERE id = $2 AND kb_id = $1")
        .bind(kb_id)
        .bind(rule_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 列出规则，连同条件与「现在推出了多少条」。
pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<serde_json::Value>> {
    let rules: Vec<RuleRow> = sqlx::query_as(
        "SELECT r.id, r.name, r.description, r.subject_type_id, st.label,
                r.conclusion, r.conclude_type_id, ct.label,
                r.conclude_predicate_id, cp.label, r.conclude_value, r.conclude_expr, r.enabled,
                (SELECT count(*) FROM derived_facts d
                  WHERE d.attribute_rule_id = r.id AND d.invalidated_at IS NULL),
                r.capped_at_last_run
           FROM attribute_rules r
           JOIN entity_types st ON st.id = r.subject_type_id
           LEFT JOIN entity_types ct ON ct.id = r.conclude_type_id
           LEFT JOIN relation_types cp ON cp.id = r.conclude_predicate_id
          WHERE r.kb_id = $1
          ORDER BY r.created_at",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<Uuid> = rules.iter().map(|r| r.0).collect();
    let conds: Vec<ConditionRow> = sqlx::query_as(
        "SELECT c.rule_id, c.group_seq, c.predicate_id, p.label, c.op, c.operand
               FROM attribute_rule_conditions c
               JOIN relation_types p ON p.id = c.predicate_id
              WHERE c.rule_id = ANY($1)
              ORDER BY c.rule_id, c.group_seq, c.seq",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;

    Ok(rules
        .into_iter()
        .map(
            |(
                id,
                name,
                description,
                subject_type_id,
                subject_label,
                conclusion,
                ct,
                ct_label,
                cp,
                cp_label,
                cv,
                cx,
                enabled,
                derived,
                capped,
            )| {
                let conditions: Vec<serde_json::Value> = conds
                    .iter()
                    .filter(|c| c.0 == id)
                    .map(|(_, group, pid, plabel, op, operand)| {
                        json!({
                            "group": group,
                            "predicate_id": pid,
                            "predicate_label": plabel,
                            "op": op,
                            "operand": operand,
                        })
                    })
                    .collect();
                json!({
                    "id": id,
                    "name": name,
                    "description": description,
                    "subject_type_id": subject_type_id,
                    "subject_label": subject_label,
                    "conclusion": conclusion,
                    "conclude_type_id": ct,
                    "conclude_type_label": ct_label,
                    "conclude_predicate_id": cp,
                    "conclude_predicate_label": cp_label,
                    "conclude_value": cv,
                    "conclude_expr": cx,
                    "enabled": enabled,
                    "derived_count": derived,
                    "capped": capped,
                    "conditions": conditions,
                })
            },
        )
        .collect())
}

/// 命中查询回来的一行：派生 id、实体 id 与名字、结论、区间两端、前提的可读形态
type MatchRow = (
    Uuid,
    Uuid,
    String,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Vec<String>,
);

/// 一条规则此刻标了哪些实体。
///
/// **规则卡片上那个数字要点得动**：二十个实体还能一个个点开看，两百个就只能
/// 靠这份列表。前提也带回来——「凭哪几条读数」与结论本身同样是答案的一半。
pub async fn matches(
    pool: &PgPool,
    kb_id: Uuid,
    rule_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<serde_json::Value>, i64)> {
    let total: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM derived_facts d
          WHERE d.kb_id = $1 AND d.attribute_rule_id = $2 AND d.invalidated_at IS NULL",
    )
    .bind(kb_id)
    .bind(rule_id)
    .fetch_one(pool)
    .await?;

    let rows: Vec<MatchRow> = sqlx::query_as(
        // 结论读出来要是人看的那个名字。库里存的是类的 key/IRI（归类）或
        // 字面值（属性），两者都不该原样端上来
        "SELECT d.id, e.id, e.canonical_name,
                COALESCE(ct.label,
                         d.object_value #>> '{value}',
                         d.object_value ->> 'class'),
                d.valid_from, d.valid_to,
                COALESCE(
                    (SELECT array_agg(
                                COALESCE(pr.label, '?') || ' = '
                                || COALESCE(fd.object_value #>> '{value}',
                                            fd.object_value #>> '{class}', '?')
                                ORDER BY fd.seq)
                       FROM derivation_premises fd
                       LEFT JOIN relation_types pr ON pr.id = fd.predicate_id
                      WHERE fd.derived_fact_id = d.id),
                    ARRAY[]::text[]
                )
           FROM derived_facts d
           JOIN entities e ON e.id = d.subject_id
           JOIN attribute_rules ar ON ar.id = d.attribute_rule_id
           LEFT JOIN entity_types ct ON ct.id = ar.conclude_type_id
          WHERE d.kb_id = $1 AND d.attribute_rule_id = $2 AND d.invalidated_at IS NULL
          ORDER BY e.canonical_name, d.valid_from, d.id
          LIMIT $3 OFFSET $4",
    )
    .bind(kb_id)
    .bind(rule_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok((
        rows.into_iter()
            .map(|(id, entity_id, name, concluded, from, to, premises)| {
                json!({
                    "derived_id": id,
                    "entity_id": entity_id,
                    "entity": name,
                    "concluded": concluded,
                    "valid_from": from,
                    "valid_to": to,
                    "premises": premises,
                })
            })
            .collect(),
        total.0,
    ))
}

async fn insert_conditions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    rule_id: Uuid,
    conditions: &[ConditionInput],
) -> AppResult<()> {
    // seq 在**组内**编号：(rule, group, seq) 才是一条条件的位置，两组各自从 0 起
    let mut next: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    for c in conditions {
        let seq = next.entry(c.group).or_insert(0);
        sqlx::query(
            "INSERT INTO attribute_rule_conditions
                (id, rule_id, group_seq, seq, predicate_id, op, operand)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(rule_id)
        .bind(c.group)
        .bind(*seq)
        .bind(c.predicate_id)
        .bind(&c.op)
        .bind(&c.operand)
        .execute(&mut **tx)
        .await?;
        *seq += 1;
    }
    Ok(())
}

/// 结论的校验。**建与改共用这一份**——分成两处写，改一次就能绕过建时的判据，
/// 而库里的 CHECK 只会回一个约束名，读的人无从知道错在哪一格。
async fn validate_conclusion(pool: &PgPool, kb_id: Uuid, c: &ConclusionInput) -> AppResult<()> {
    match c.kind.as_str() {
        "typing" => {
            let t = c.type_id.ok_or_else(|| {
                AppError::invalid("no_class", "A typing rule needs the class it concludes.")
            })?;
            exists(
                pool,
                kb_id,
                "entity_types",
                t,
                "unknown_class",
                "That class is not in this base.",
            )
            .await?;
            // 归类结论落在内建 is_a 上，第一次写这种规则时把它建出来
            ensure_is_a(pool, kb_id).await?;
            Ok(())
        }
        "attribute" => {
            let p = c.predicate_id.ok_or_else(|| {
                AppError::invalid(
                    "no_predicate",
                    "An attribute rule needs the attribute it sets.",
                )
            })?;
            if c.value.is_none() {
                return Err(AppError::invalid(
                    "no_value",
                    "An attribute rule needs the value it sets.",
                ));
            }
            attribute_predicate(pool, kb_id, p).await
        }
        // 算出来的属性（0032）：谓词照旧要在，值换成一棵算式树
        "computed" => {
            let p = c.predicate_id.ok_or_else(|| {
                AppError::invalid(
                    "no_predicate",
                    "A computed rule needs the attribute it sets.",
                )
            })?;
            let Some(expr) = c.expr.as_ref() else {
                return Err(AppError::invalid(
                    "no_expression",
                    "A computed rule needs the expression it computes.",
                ));
            };
            // **这几条挡在入口而不是留给求值器。** 读不懂的树在求值时只会
            // 「这条规则什么都不推」，而写它的人看不见任何理由（`not_in`
            // 那次就是这么丢的，见 #494）
            let reads = validate_expr(expr, 0)?;
            if reads.is_empty() {
                return Err(AppError::invalid(
                    "constant_expression",
                    "A computed rule that reads no attribute is a constant; set the value directly.",
                ));
            }
            for r in reads {
                attribute_predicate(pool, kb_id, r).await?;
            }
            attribute_predicate(pool, kb_id, p).await
        }
        _ => Err(AppError::invalid(
            "bad_conclusion",
            "A conclusion is a typing, an attribute or a computed attribute.",
        )),
    }
}

/// 一棵算式树的校验，返回它读到的谓词。
///
/// **形状不对就说清哪里不对。** 库里的 CHECK 只管「computed 得有一棵树」，
/// 树自己长得对不对得在这里判——否则一棵写坏的树要等到下一次物化才表现为
/// 「这条规则不推东西」，而那时候没有任何地方说得出为什么。
fn validate_expr(raw: &serde_json::Value, depth: usize) -> AppResult<Vec<Uuid>> {
    use utopia_reason::rules::{Arith, MAX_EXPR_DEPTH};
    if depth > MAX_EXPR_DEPTH {
        return Err(AppError::invalid(
            "expression_too_deep",
            "That expression nests too deeply; a rule computes with a few operations, not a program.",
        ));
    }
    let bad = || {
        AppError::invalid(
            "bad_expression",
            "An expression is an attribute, a number, or two of those combined with + − × ÷.",
        )
    };
    let obj = raw.as_object().ok_or_else(bad)?;
    if let Some(a) = obj.get("attr") {
        let id: Uuid = a.as_str().ok_or_else(bad)?.parse().map_err(|_| bad())?;
        return Ok(vec![id]);
    }
    if let Some(c) = obj.get("const") {
        let n = c
            .as_f64()
            .or_else(|| c.as_str()?.trim().parse().ok())
            .ok_or_else(bad)?;
        if !n.is_finite() {
            return Err(bad());
        }
        return Ok(Vec::new());
    }
    Arith::parse(obj.get("op").and_then(|v| v.as_str()).ok_or_else(bad)?).ok_or_else(bad)?;
    let mut reads = validate_expr(obj.get("l").ok_or_else(bad)?, depth + 1)?;
    reads.extend(validate_expr(obj.get("r").ok_or_else(bad)?, depth + 1)?);
    reads.sort();
    reads.dedup();
    Ok(reads)
}

/// 条件的校验。**报错要说人话**：库里的 CHECK 只会回一个约束名。
async fn validate_conditions(
    pool: &PgPool,
    kb_id: Uuid,
    conditions: &[ConditionInput],
) -> AppResult<()> {
    for c in conditions {
        let op = utopia_reason::rules::Op::parse(&c.op).ok_or_else(|| {
            AppError::invalid(
                "bad_op",
                "A condition compares with >, >=, <, <=, a range, a set (in or not in), or presence.",
            )
        })?;
        attribute_predicate(pool, kb_id, c.predicate_id).await?;
        let shaped = match op {
            utopia_reason::rules::Op::Present => c.operand.is_none(),
            utopia_reason::rules::Op::NotIn => c
                .operand
                .as_ref()
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty()),
            utopia_reason::rules::Op::Between => c
                .operand
                .as_ref()
                .and_then(|v| v.as_array())
                .is_some_and(|a| a.len() == 2 && a.iter().all(|x| x.as_f64().is_some())),
            utopia_reason::rules::Op::In => c
                .operand
                .as_ref()
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty()),
            _ => c.operand.as_ref().is_some_and(|v| {
                v.as_f64().is_some()
                    || v.as_str()
                        .and_then(|s| s.trim().parse::<f64>().ok())
                        .is_some()
            }),
        };
        if !shaped {
            return Err(AppError::invalid(
                "bad_operand",
                "That comparison does not match the value it was given: a threshold needs a number, a range needs two, a set needs at least one entry, and presence takes none.",
            ));
        }
    }
    Ok(())
}

/// 条件与属性结论只能落在 kind='attribute' 的谓词上——规则读的是这个实体
/// 自己的字面值，关系（实体到实体）不参与（0021 决策 3）。
async fn attribute_predicate(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<()> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT kind FROM relation_types WHERE id = $2 AND kb_id = $1")
            .bind(kb_id)
            .bind(id)
            .fetch_optional(pool)
            .await?;
    match row.as_ref().map(|(k,)| k.as_str()) {
        Some("attribute") => Ok(()),
        Some(_) => Err(AppError::invalid(
            "not_an_attribute",
            "A rule reads this entity's own attributes; a relation to another entity cannot be a condition.",
        )),
        None => Err(AppError::invalid(
            "unknown_predicate",
            "That attribute is not in this base.",
        )),
    }
}

async fn exists(
    pool: &PgPool,
    kb_id: Uuid,
    table: &str,
    id: Uuid,
    code: &'static str,
    message: &'static str,
) -> AppResult<()> {
    let sql = format!("SELECT 1 FROM {table} WHERE id = $2 AND kb_id = $1");
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(kb_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ())
        .ok_or_else(|| AppError::invalid(code, message))
}
