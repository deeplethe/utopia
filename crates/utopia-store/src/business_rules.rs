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
    bool,
    i64,
);

/// 条件查询回来的一行：规则、属性谓词及其标签、比较方式、操作数
type ConditionRow = (
    Uuid,
    Uuid,
    Option<String>,
    String,
    Option<serde_json::Value>,
);

/// 一条条件，界面与 API 共用的形状。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConditionInput {
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

    match conclusion {
        "typing" => {
            let t = conclude_type_id.ok_or_else(|| {
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
            ensure_is_a(pool, kb_id).await?;
        }
        "attribute" => {
            let p = conclude_predicate_id.ok_or_else(|| {
                AppError::invalid(
                    "no_predicate",
                    "An attribute rule needs the attribute it sets.",
                )
            })?;
            if conclude_value.is_none() {
                return Err(AppError::invalid(
                    "no_value",
                    "An attribute rule needs the value it sets.",
                ));
            }
            attribute_predicate(pool, kb_id, p).await?;
        }
        _ => {
            return Err(AppError::invalid(
                "bad_conclusion",
                "A conclusion is either a typing or an attribute.",
            ))
        }
    }
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
              conclude_type_id, conclude_predicate_id, conclude_value)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
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
pub async fn update(
    pool: &PgPool,
    kb_id: Uuid,
    rule_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    enabled: Option<bool>,
    conditions: Option<&[ConditionInput]>,
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
    let mut tx = pool.begin().await?;
    let res = sqlx::query(
        "UPDATE attribute_rules
            SET name = COALESCE($3, name),
                description = COALESCE($4, description),
                enabled = COALESCE($5, enabled),
                updated_at = now()
          WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(rule_id)
    .bind(name.map(str::trim))
    .bind(description.map(str::trim))
    .bind(enabled)
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
                r.conclude_predicate_id, cp.label, r.conclude_value, r.enabled,
                (SELECT count(*) FROM derived_facts d
                  WHERE d.attribute_rule_id = r.id AND d.invalidated_at IS NULL)
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
        "SELECT c.rule_id, c.predicate_id, p.label, c.op, c.operand
               FROM attribute_rule_conditions c
               JOIN relation_types p ON p.id = c.predicate_id
              WHERE c.rule_id = ANY($1)
              ORDER BY c.rule_id, c.seq",
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
                enabled,
                derived,
            )| {
                let conditions: Vec<serde_json::Value> = conds
                    .iter()
                    .filter(|c| c.0 == id)
                    .map(|(_, pid, plabel, op, operand)| {
                        json!({
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
                    "enabled": enabled,
                    "derived_count": derived,
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
    Option<serde_json::Value>,
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
        "SELECT d.id, e.id, e.canonical_name, d.object_value, d.valid_from, d.valid_to,
                COALESCE(
                    (SELECT array_agg(
                                COALESCE(pr.label, '?') || ' = '
                                || COALESCE(pf.object_value #>> '{value}', '?')
                                ORDER BY fd.seq)
                       FROM fact_derivations fd
                       JOIN facts pf ON pf.id = fd.premise_fact_id
                       LEFT JOIN relation_types pr ON pr.id = pf.predicate_id
                      WHERE fd.derived_fact_id = d.id),
                    ARRAY[]::text[]
                )
           FROM derived_facts d
           JOIN entities e ON e.id = d.subject_id
          WHERE d.kb_id = $1 AND d.attribute_rule_id = $2 AND d.invalidated_at IS NULL
          ORDER BY e.canonical_name
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
            .map(|(id, entity_id, name, value, from, to, premises)| {
                json!({
                    "derived_id": id,
                    "entity_id": entity_id,
                    "entity": name,
                    "concluded": value
                        .as_ref()
                        .and_then(|v| v.get("class").or_else(|| v.get("value")))
                        .cloned(),
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
    for (seq, c) in conditions.iter().enumerate() {
        sqlx::query(
            "INSERT INTO attribute_rule_conditions (id, rule_id, seq, predicate_id, op, operand)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(rule_id)
        .bind(seq as i32)
        .bind(c.predicate_id)
        .bind(&c.op)
        .bind(&c.operand)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
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
                "A condition compares with >, >=, <, <=, a range, a set, or presence.",
            )
        })?;
        attribute_predicate(pool, kb_id, c.predicate_id).await?;
        let shaped = match op {
            utopia_reason::rules::Op::Present => c.operand.is_none(),
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
