//! 时态引擎（S3）：functional 状态关系的矛盾检测与自动闭合。
//!
//! 原则：
//! - 纯规则判定，零 LLM——模糊性已在上游（消解归并实体、本体标 functional）消化
//! - 闭合走"作废 + 改写"而非原地改：旧断言 invalidated_at 记下"何时被修正"，
//!   修正行闭合区间并以 supersedes 链回旧行——"以当时的认知回放当时"得以成立
//! - 闭合点只用世界时间（新事实的 valid_from），绝不用摄取时刻顶替
//! - 拿不准（缺时间/同时开始/低置信）绝不硬闭合，进 fact_conflicts 由人裁决

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::models::ConflictView;
use utopia_core::AppResult;
use uuid::Uuid;

/// 低于此置信度的新事实不允许自动改写历史（进审）。
const AUTO_CLOSE_MIN_CONFIDENCE: f32 = 0.75;

/// 命中的开放期事实（不变量下至多一条，引擎上线前的历史脏数据可能多条）。
#[derive(Debug, sqlx::FromRow)]
struct OpenFact {
    id: Uuid,
    valid_from: Option<DateTime<Utc>>,
    /// 闭合别人的区间时要用它当那个时刻的粒度
    valid_from_precision: Option<String>,
}

/// 唯一性方向：functional = 主语侧（张三同时只 reports_to 一人）；
/// inverse functional = 宾语侧（一个项目同时只有一个 leads 它的人）。
#[derive(Debug, Clone, Copy)]
pub enum Uniqueness {
    SubjectSide,
    ObjectSide,
}

/// 对账结果：自动闭合产生的修正行 id（调用方按需记账，如合并回滚要撤销它们）
/// 与进入人审的冲突数。
#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub corrected: Vec<Uuid>,
    pub conflicts: u32,
}

/// 一条新 state 事实落库后、沿指定唯一性方向的对账。调用方负责判断关系确实
/// 带该方向的唯一性且 temporal = state（本体元数据在抽取任务里已加载）。
///
/// 宾语可以是实体（object_id）或字面值（object_value，属性事实）——
/// "宾语不同"的判定是 (object_id, object_value) 组合比较：工资从 3 万变 3.5 万
/// 与"从张三换成李四"走同一条闭合路径。
#[allow(clippy::too_many_arguments)]
pub async fn reconcile_new_fact(
    pool: &PgPool,
    kb_id: Uuid,
    new_fact_id: Uuid,
    subject_id: Uuid,
    predicate_id: Uuid,
    object_id: Option<Uuid>,
    object_value: Option<&serde_json::Value>,
    direction: Uniqueness,
    new_validity: crate::graph::Validity<'_>,
    new_confidence: f32,
) -> AppResult<ReconcileReport> {
    // 已闭区间的新事实是历史陈述，不威胁"开放期唯一"不变量——不触发任何改写
    // （闭区间之间的重叠矛盾是更细的区间代数，暂不自动裁，留给 Review 的人眼）
    if new_validity.has_ended() {
        return Ok(ReconcileReport::default());
    }
    // 宾语侧唯一性只对实体宾语有意义（字面值不"被占用"）
    if matches!(direction, Uniqueness::ObjectSide) && object_id.is_none() {
        return Ok(ReconcileReport::default());
    }
    // 不变量点查：主语侧 = 同 (kb, S, P) 宾语不同；宾语侧 = 同 (kb, P, O) 主语不同
    let sql = match direction {
        Uniqueness::SubjectSide => {
            "SELECT id, valid_from, valid_from_precision FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3
               AND valid_to IS NULL AND valid_to_precision IS NULL
               AND invalidated_at IS NULL
               AND id <> $4
               AND (object_id IS DISTINCT FROM $5 OR object_value IS DISTINCT FROM $6)
             ORDER BY valid_from ASC NULLS LAST, recorded_at ASC"
        }
        Uniqueness::ObjectSide => {
            "SELECT id, valid_from, valid_from_precision FROM facts
             WHERE kb_id = $1 AND object_id = $5 AND predicate_id = $3
               AND valid_to IS NULL AND valid_to_precision IS NULL
               AND invalidated_at IS NULL
               AND id <> $4 AND subject_id IS DISTINCT FROM $2
             ORDER BY valid_from ASC NULLS LAST, recorded_at ASC"
        }
    };
    let q = sqlx::query_as(sql)
        .bind(kb_id)
        .bind(subject_id)
        .bind(predicate_id)
        .bind(new_fact_id)
        .bind(object_id);
    let open: Vec<OpenFact> = match direction {
        Uniqueness::SubjectSide => q.bind(object_value).fetch_all(pool).await?,
        Uniqueness::ObjectSide => q.fetch_all(pool).await?,
    };
    if open.is_empty() {
        return Ok(ReconcileReport::default());
    }

    let mut report = ReconcileReport::default();
    for old in open {
        match (old.valid_from, new_validity.from) {
            // 新事实没有世界时间：闭合点无从谈起 → 人裁
            (_, None) => {
                record_conflict(pool, kb_id, old.id, new_fact_id, "no_time").await?;
                report.conflicts += 1;
            }
            // 同一时刻开始：谁接替谁说不清 → 人裁
            (Some(of), Some(nf)) if of == nf => {
                record_conflict(pool, kb_id, old.id, new_fact_id, "simultaneous").await?;
                report.conflicts += 1;
            }
            // 新事实开始得更早：它是历史前任，闭合在旧事实的开始
            (Some(of), Some(nf)) if nf < of => {
                if new_confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                    record_conflict(pool, kb_id, old.id, new_fact_id, "low_confidence").await?;
                    report.conflicts += 1;
                } else {
                    if let Some(id) = close_superseded(
                        pool,
                        new_fact_id,
                        of,
                        old.valid_from_precision.as_deref().unwrap_or("day"),
                    )
                    .await?
                    {
                        report.corrected.push(id);
                    }
                }
            }
            // 常规接替：旧事实闭合在新事实的开始（旧事实无起点也适用——起点未知但已结束）
            (_, Some(nf)) => {
                if new_confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                    record_conflict(pool, kb_id, old.id, new_fact_id, "low_confidence").await?;
                    report.conflicts += 1;
                } else {
                    if let Some(id) = close_superseded(
                        pool,
                        old.id,
                        nf,
                        new_validity.from_precision.unwrap_or("day"),
                    )
                    .await?
                    {
                        report.corrected.push(id);
                    }
                }
            }
        }
    }
    Ok(report)
}

/// 实体合并搬移事实后的对账：换了主/宾的事实等价于"新落库的观察"——
/// 两个对象折成一个之后，唯一性不变量才第一次看得到它们相撞。
/// 按 recorded_at 顺序逐条重跑插入时对账；非唯一性关系、已闭合区间、
/// 已被前一条改写作废的事实自动跳过。
/// 返回的修正行 id 由调用方记入合并账本——这些修正的唯一成因是合并本身，
/// 回滚合并时必须随之撤销（修正行作废、被取代的原行恢复），否则修正行会
/// 错挂在 target 上而其依据已随回滚离开。
pub async fn reconcile_moved_facts(
    pool: &PgPool,
    kb_id: Uuid,
    fact_ids: &[Uuid],
) -> AppResult<ReconcileReport> {
    if fact_ids.is_empty() {
        return Ok(ReconcileReport::default());
    }
    reconcile_open(pool, kb_id, fact_ids, "f.recorded_at").await
}

/// 声明来晚了：一条谓词上所有开放事实按**年表**重跑一遍落库时的对账（#341）。
///
/// 本体自己长出来的库里没人声明过唯一性，接任不会闭合前任——三个人同时在管一个
/// 项目。人补上声明之后，这里把已经躺在账上的行对一遍。走的是与落库时同一条
/// 路（作废 + 改写，supersedes 链回旧行），所以记录轴倒回声明之前仍看得见三条
/// 开放的行；拿不准的（缺时间 / 同时开始 / 低置信）照旧进人审，不硬闭合。
///
/// 按 `valid_from` 而不是 `recorded_at` 走：合并搬移按摄入顺序回放是对的，
/// 因为那是在重演落库；这里三条早就都在账上，谁先谁后只有年表说了算——
/// 前任必须闭合在**最早的**后任起点上，而周七的文档完全可能比李四的先到。
///
/// 没有声明的谓词拒绝：引擎不替人推断（bootstrap_ontology.rs 写了为什么）。
pub async fn reconcile_predicate(
    pool: &PgPool,
    kb_id: Uuid,
    predicate_id: Uuid,
) -> AppResult<ReconcileReport> {
    let declared: Option<(bool, bool, String)> = sqlx::query_as(
        "SELECT functional, inverse_functional, temporal FROM relation_types
         WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(predicate_id)
    .fetch_optional(pool)
    .await?;
    let Some((functional, inverse_functional, temporal)) = declared else {
        return Err(utopia_core::AppError::NotFound);
    };
    if temporal != "state" {
        return Err(utopia_core::AppError::invalid(
            "not_a_state",
            "only a state relation has intervals to close",
        ));
    }
    if !functional && !inverse_functional {
        return Err(utopia_core::AppError::invalid(
            "not_unique",
            "declare the relation functional or inverse-functional first; the engine does not infer it",
        ));
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM facts
         WHERE kb_id = $1 AND predicate_id = $2
           AND invalidated_at IS NULL AND valid_to IS NULL AND valid_to_precision IS NULL
         ORDER BY valid_from ASC NULLS LAST, recorded_at ASC",
    )
    .bind(kb_id)
    .bind(predicate_id)
    .fetch_all(pool)
    .await?;
    if ids.is_empty() {
        return Ok(ReconcileReport::default());
    }
    reconcile_open(
        pool,
        kb_id,
        &ids,
        "f.valid_from ASC NULLS LAST, f.recorded_at ASC",
    )
    .await
}

/// 一批开放期事实按 `order`（SQL 的 ORDER BY 表达式）逐条当作"新落库的观察"对账。
async fn reconcile_open(
    pool: &PgPool,
    kb_id: Uuid,
    fact_ids: &[Uuid],
    order: &str,
) -> AppResult<ReconcileReport> {
    #[derive(sqlx::FromRow)]
    struct OpenRow {
        id: Uuid,
        subject_id: Uuid,
        predicate_id: Uuid,
        object_id: Option<Uuid>,
        object_value: Option<serde_json::Value>,
        valid_from: Option<DateTime<Utc>>,
        valid_from_precision: Option<String>,
        confidence: f32,
        functional: bool,
        inverse_functional: bool,
    }
    let rows: Vec<OpenRow> = sqlx::query_as(&format!(
        "SELECT f.id, f.subject_id, f.predicate_id, f.object_id, f.object_value,
                f.valid_from, f.valid_from_precision,
                f.confidence, r.functional, r.inverse_functional
         FROM facts f JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND f.id = ANY($2)
           AND f.invalidated_at IS NULL AND f.valid_to IS NULL
           AND r.temporal = 'state' AND (r.functional OR r.inverse_functional)
         ORDER BY {order}"
    ))
    .bind(kb_id)
    .bind(fact_ids)
    .fetch_all(pool)
    .await?;

    let mut report = ReconcileReport::default();
    for f in rows {
        // 字面值事实（属性）合并后同样对账：两个"张三"折成一个，工资撞车也要闭合
        if f.object_id.is_none() && f.object_value.is_none() {
            continue;
        }
        // 前面的闭合可能已把本条改写作废——逐条复核存活性再当"新事实"用
        let (alive,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM facts
                            WHERE id = $1 AND invalidated_at IS NULL AND valid_to IS NULL)",
        )
        .bind(f.id)
        .fetch_one(pool)
        .await?;
        if !alive {
            continue;
        }
        let mut directions = Vec::new();
        if f.functional {
            directions.push(Uniqueness::SubjectSide);
        }
        if f.inverse_functional {
            directions.push(Uniqueness::ObjectSide);
        }
        for dir in directions {
            let r = reconcile_new_fact(
                pool,
                kb_id,
                f.id,
                f.subject_id,
                f.predicate_id,
                f.object_id,
                f.object_value.as_ref(),
                dir,
                crate::graph::Validity::starting(f.valid_from, f.valid_from_precision.as_deref()),
                f.confidence,
            )
            .await?;
            report.corrected.extend(r.corrected);
            report.conflicts += r.conflicts;
        }
    }
    Ok(report)
}

/// 作废 + 改写：旧行记 invalidated_at（认知轴），插入闭合区间的修正行
/// （世界轴），证据引用随行复制。返回修正行 id；`None` = 这条已不是开放行，没动。
pub async fn close_superseded(
    pool: &PgPool,
    fact_id: Uuid,
    valid_to: DateTime<Utc>,
    valid_to_precision: &str,
) -> AppResult<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let corrected = Uuid::now_v7();
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence, supersedes, attested_at)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                valid_from, valid_from_precision, $3, $4, confidence, id, attested_at
         FROM facts WHERE id = $2 AND invalidated_at IS NULL
         RETURNING id",
    )
    .bind(corrected)
    .bind(fact_id)
    .bind(valid_to)
    .bind(valid_to_precision)
    .fetch_optional(&mut *tx)
    .await?;
    // 已被改写过（并发，或同一批对账里前一步已经闭合了它）：不重复动手，也不算一次修正
    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;
    copy_evidence(&mut tx, fact_id, corrected).await?;
    tx.commit().await?;
    Ok(Some(corrected))
}

/// 作废 + 改写成「结束了，不知哪天」（0022）：旧行记 invalidated_at，修正行终点仍是
/// NULL、精度 'unknown'，锚点换成**说出结束的那份文档**——读出来就是「到它为止」
/// （起点是原文给的，锚点只管结束端）。证据引用随行复制。
/// 返回修正行 id；`None` = 这条已不是带起点的开放行，没动。
pub async fn close_with_unknown_end(
    pool: &PgPool,
    fact_id: Uuid,
    attested_at: Option<DateTime<Utc>>,
) -> AppResult<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let corrected = Uuid::now_v7();
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence, supersedes, attested_at)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                valid_from, valid_from_precision, NULL, $3, confidence, id, COALESCE($4, now())
         FROM facts
         WHERE id = $2 AND invalidated_at IS NULL
           AND valid_from IS NOT NULL AND valid_to IS NULL AND valid_to_precision IS NULL
         RETURNING id",
    )
    .bind(corrected)
    .bind(fact_id)
    .bind(crate::graph::ENDED_UNKNOWN)
    .bind(attested_at)
    .fetch_optional(&mut *tx)
    .await?;
    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;
    copy_evidence(&mut tx, fact_id, corrected).await?;
    tx.commit().await?;
    Ok(Some(corrected))
}

/// 证据引用随修正行复制。表层谓词一起搬：纠正的是时间区间，不是原文说了什么。
async fn copy_evidence(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    from: Uuid,
    to: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version)
         SELECT $1, chunk_id, quote, proposed_predicate, document_id, doc_version
         FROM fact_evidence WHERE fact_id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(to)
    .bind(from)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// 人工修正一条事实的有效区间。与自动闭合同一机制（作废 + 改写），区别只在
/// 两端都来自参数，而不是继承旧行的起点。
///
/// 抽取把「2023 年上半年」读成 1 月 1 日，在此之前只能删掉文档重抽一遍：
/// 名字判错有 Review 可改，时间判错没有入口，而整条时间线都歪在那一个值上。
///
/// **不原地 UPDATE。** 原地改会把修正本身抹掉，而那正是记录轴要回放的东西
/// （0019）——改过之后，问三月与问九月应当得到不同的区间。这也是这个函数
/// 与一条 `UPDATE facts SET valid_from = …` 的全部差别。
///
/// 返回修正行 id；`None` 表示这条已被并发改写或作废，本次没有动手。
pub async fn correct_interval(
    pool: &PgPool,
    fact_id: Uuid,
    validity: crate::graph::Validity<'_>,
) -> AppResult<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    let corrected = Uuid::now_v7();
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence, supersedes, attested_at)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                $3, $4, $5, $6, confidence, id, attested_at
         FROM facts WHERE id = $2 AND invalidated_at IS NULL
         RETURNING id",
    )
    .bind(corrected)
    .bind(fact_id)
    .bind(validity.from)
    .bind(validity.from_precision)
    .bind(validity.to)
    .bind(validity.to_precision)
    .fetch_optional(&mut *tx)
    .await?;
    // 已被并发修正过：不重复动手（与 close_superseded 同一防线）
    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;
    copy_evidence(&mut tx, fact_id, corrected).await?;
    tx.commit().await?;
    Ok(Some(corrected))
}

async fn record_conflict(
    pool: &PgPool,
    kb_id: Uuid,
    old_fact_id: Uuid,
    new_fact_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO fact_conflicts (id, kb_id, old_fact_id, new_fact_id, reason)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (old_fact_id, new_fact_id) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(old_fact_id)
    .bind(new_fact_id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

/// Review 页的冲突列表（双方事实带名字与区间）。
/// 惰性清理：任一方已被作废（被驳回/被别的闭合改写）的冲突已无意义，
/// 自动出队标 stale——防止在僵尸冲突上误裁（如把 Eve 闭合在已驳回的 Ivan 上）。
pub async fn list_conflicts(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ConflictView>> {
    sqlx::query(
        "UPDATE fact_conflicts c
         SET status = 'resolved', resolution = 'stale', resolved_at = now()
         WHERE c.kb_id = $1 AND c.status = 'open'
           AND EXISTS (SELECT 1 FROM facts f
                       WHERE f.id IN (c.old_fact_id, c.new_fact_id)
                         AND f.invalidated_at IS NOT NULL)",
    )
    .bind(kb_id)
    .execute(pool)
    .await?;
    let rows: Vec<ConflictView> = sqlx::query_as(
        "SELECT c.id, c.reason, c.created_at, r.label AS predicate_label,
                c.old_fact_id, os.canonical_name AS old_subject,
                oo.canonical_name AS old_object, fo.valid_from AS old_valid_from,
                c.new_fact_id, ns.canonical_name AS new_subject,
                no_.canonical_name AS new_object, fn_.valid_from AS new_valid_from,
                fn_.confidence AS new_confidence
         FROM fact_conflicts c
         JOIN facts fo ON fo.id = c.old_fact_id
         JOIN facts fn_ ON fn_.id = c.new_fact_id
         JOIN entities os ON os.id = fo.subject_id
         JOIN entities ns ON ns.id = fn_.subject_id
         JOIN relation_types r ON r.id = fo.predicate_id
         LEFT JOIN entities oo ON oo.id = fo.object_id
         LEFT JOIN entities no_ ON no_.id = fn_.object_id
         WHERE c.kb_id = $1 AND c.status = 'open'
         ORDER BY c.created_at DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 人工裁决：close（旧事实闭合于 close_at 或新事实起点）/ keep（并存不矛盾）/
/// reject_new（新事实是抽取错误，作废）。
pub async fn resolve_conflict(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    resolution: &str,
    close_at: Option<DateTime<Utc>>,
) -> AppResult<()> {
    let row: Option<(Uuid, Uuid, Option<DateTime<Utc>>)> = sqlx::query_as(
        "SELECT c.old_fact_id, c.new_fact_id, fn_.valid_from
         FROM fact_conflicts c JOIN facts fn_ ON fn_.id = c.new_fact_id
         WHERE c.id = $1 AND c.kb_id = $2 AND c.status = 'open'",
    )
    .bind(conflict_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    let Some((old_fact_id, new_fact_id, new_from)) = row else {
        return Err(utopia_core::AppError::NotFound);
    };

    let stored = match resolution {
        "close" => {
            let at = close_at.or(new_from).ok_or_else(|| {
                utopia_core::AppError::invalid(
                    "close_at_required",
                    "close_at is required when the new fact has no start time",
                )
            })?;
            close_superseded(pool, old_fact_id, at, "day").await?;
            "closed"
        }
        "keep" => "kept_both",
        "reject_new" => {
            sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
                .bind(new_fact_id)
                .execute(pool)
                .await?;
            // 波及：同一新事实撞出的其他 open 冲突一并出队（新事实已死，无从裁起）
            sqlx::query(
                "UPDATE fact_conflicts
                 SET status = 'resolved', resolution = 'rejected_new', resolved_at = now()
                 WHERE new_fact_id = $1 AND status = 'open' AND id <> $2",
            )
            .bind(new_fact_id)
            .bind(conflict_id)
            .execute(pool)
            .await?;
            "rejected_new"
        }
        other => {
            return Err(utopia_core::AppError::Validation(format!(
                "Unknown resolution: {other}"
            )))
        }
    };
    sqlx::query(
        "UPDATE fact_conflicts SET status = 'resolved', resolution = $3, resolved_at = now()
         WHERE id = $1 AND kb_id = $2",
    )
    .bind(conflict_id)
    .bind(kb_id)
    .bind(stored)
    .execute(pool)
    .await?;
    Ok(())
}

/// 一条谓词的一端挂着**两个以上开放值**的持有者——唯一性没声明（或声明来晚了）
/// 时账本的样子（#341）。这是给人看的提议依据，不是判决：`declared` 为假时它说
/// "这里像是该声明的"，为真时它说"声明了，但这些行还没对过账"。
#[derive(Debug, Clone)]
pub struct UniquenessCandidate {
    pub predicate_id: Uuid,
    pub key: String,
    pub label: String,
    /// relation | attribute
    pub kind: String,
    /// "subject"（→ functional）或 "object"（→ inverse_functional）
    pub side: &'static str,
    /// 这一端的唯一性是否已经声明
    pub declared: bool,
    /// 挂着两个以上开放值的持有者数
    pub holders: usize,
    /// 那些持有者身上的开放事实数
    pub open_facts: usize,
    /// 对账会闭合的区间数（估算，按落库时的规则：起点更早者止于后任起点）
    pub would_close: usize,
    /// 对账会送进人审的对数（缺时间 / 同时开始 / 低置信）
    pub would_review: usize,
    /// 头几个持有者与它们的开放值，按年表排
    pub examples: Vec<HolderExample>,
}

#[derive(Debug, Clone)]
pub struct HolderExample {
    pub holder: String,
    pub values: Vec<OpenValue>,
}

#[derive(Debug, Clone)]
pub struct OpenValue {
    pub fact_id: Uuid,
    pub name: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub confidence: f32,
}

/// 每个候选带几个例子。
const EXAMPLE_HOLDERS: usize = 3;

pub async fn uniqueness_candidates(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<Vec<UniquenessCandidate>> {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: Uuid,
        predicate_id: Uuid,
        key: String,
        label: String,
        kind: String,
        declared: bool,
        holder_name: String,
        other_name: Option<String>,
        object_value: Option<serde_json::Value>,
        valid_from: Option<DateTime<Utc>>,
        confidence: f32,
    }
    // 两端各查一遍。`crowded` 先按 (谓词, 持有者) 数不同的值，再把那些持有者
    // 身上的开放事实整批取回来——估算要看每一对相邻值的起点与置信度，
    // 光有计数不够。开放 = 世界轴没有终点，与落库时对账用的是同一个不变量
    let subject_side: Vec<Row> = sqlx::query_as(
        "WITH open AS (
             SELECT f.id, f.predicate_id, f.subject_id AS holder, f.object_id, f.object_value,
                    COALESCE(f.object_id::text, f.object_value::text) AS value_key,
                    f.valid_from, f.confidence, f.recorded_at
             FROM facts f JOIN relation_types r ON r.id = f.predicate_id
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
               AND f.valid_to IS NULL AND f.valid_to_precision IS NULL
               AND r.temporal = 'state'
               AND (f.object_id IS NOT NULL OR f.object_value IS NOT NULL)
         ),
         crowded AS (
             SELECT predicate_id, holder FROM open
             GROUP BY predicate_id, holder HAVING count(DISTINCT value_key) >= 2
         )
         SELECT o.id, o.predicate_id, r.key, r.label, r.kind, r.functional AS declared,
                h.canonical_name AS holder_name, e.canonical_name AS other_name,
                o.object_value, o.valid_from, o.confidence
         FROM open o
         JOIN crowded c ON c.predicate_id = o.predicate_id AND c.holder = o.holder
         JOIN relation_types r ON r.id = o.predicate_id
         JOIN entities h ON h.id = o.holder
         LEFT JOIN entities e ON e.id = o.object_id
         ORDER BY r.key, h.canonical_name, o.valid_from ASC NULLS LAST, o.recorded_at ASC",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let object_side: Vec<Row> = sqlx::query_as(
        "WITH open AS (
             SELECT f.id, f.predicate_id, f.object_id AS holder, f.subject_id,
                    f.valid_from, f.confidence, f.recorded_at
             FROM facts f JOIN relation_types r ON r.id = f.predicate_id
             WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
               AND f.valid_to IS NULL AND f.valid_to_precision IS NULL
               AND r.temporal = 'state' AND r.kind = 'relation'
               AND f.object_id IS NOT NULL
         ),
         crowded AS (
             SELECT predicate_id, holder FROM open
             GROUP BY predicate_id, holder HAVING count(DISTINCT subject_id) >= 2
         )
         SELECT o.id, o.predicate_id, r.key, r.label, r.kind, r.inverse_functional AS declared,
                h.canonical_name AS holder_name, e.canonical_name AS other_name,
                NULL::jsonb AS object_value, o.valid_from, o.confidence
         FROM open o
         JOIN crowded c ON c.predicate_id = o.predicate_id AND c.holder = o.holder
         JOIN relation_types r ON r.id = o.predicate_id
         JOIN entities h ON h.id = o.holder
         JOIN entities e ON e.id = o.subject_id
         ORDER BY r.key, h.canonical_name, o.valid_from ASC NULLS LAST, o.recorded_at ASC",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::new();
    for (side, rows) in [("subject", subject_side), ("object", object_side)] {
        // 行已按 (谓词, 持有者, 年表) 排好，顺着切段就是分组
        let mut i = 0;
        while i < rows.len() {
            let pred = rows[i].predicate_id;
            let mut cand = UniquenessCandidate {
                predicate_id: pred,
                key: rows[i].key.clone(),
                label: rows[i].label.clone(),
                kind: rows[i].kind.clone(),
                side,
                declared: rows[i].declared,
                holders: 0,
                open_facts: 0,
                would_close: 0,
                would_review: 0,
                examples: Vec::new(),
            };
            while i < rows.len() && rows[i].predicate_id == pred {
                let holder = rows[i].holder_name.clone();
                let mut values = Vec::new();
                while i < rows.len()
                    && rows[i].predicate_id == pred
                    && rows[i].holder_name == holder
                {
                    let r = &rows[i];
                    values.push(OpenValue {
                        fact_id: r.id,
                        name: r
                            .other_name
                            .clone()
                            .or_else(|| r.object_value.as_ref().map(literal_name))
                            .unwrap_or_else(|| "?".to_string()),
                        valid_from: r.valid_from,
                        confidence: r.confidence,
                    });
                    i += 1;
                }
                let (close, review) = plan_closures(&values);
                cand.holders += 1;
                cand.open_facts += values.len();
                cand.would_close += close;
                cand.would_review += review;
                if cand.examples.len() < EXAMPLE_HOLDERS {
                    cand.examples.push(HolderExample { holder, values });
                }
            }
            out.push(cand);
        }
    }
    Ok(out)
}

/// 一个持有者的开放值按年表排好后，对账会怎么处置：(闭合数, 进人审数)。
///
/// 把 `reconcile_predicate` 的过程干跑一遍，不落库：逐条当"新落库的观察"，与
/// 后面还开着的每一条比——起点更早者止于后任起点（自己置信度够才许改写历史）；
/// 两条都没起点、或同一天开始，说不清谁接替谁，进人审，两条都还开着；后任没起点
/// 的，它止于前任的起点。三条都没起点的持有者会报三对冲突，与引擎一致。
/// 这是估算——真跑一遍的结果才作数
fn plan_closures(values: &[OpenValue]) -> (usize, usize) {
    let mut open = vec![true; values.len()];
    let mut close = 0;
    let mut review = 0;
    for i in 0..values.len() {
        if !open[i] {
            continue;
        }
        let new = &values[i];
        for j in (i + 1)..values.len() {
            if !open[j] {
                continue;
            }
            let old = &values[j];
            match (old.valid_from, new.valid_from) {
                (_, None) => review += 1,
                (Some(of), Some(nf)) if of == nf => review += 1,
                (Some(of), Some(nf)) if nf < of => {
                    if new.confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                        review += 1;
                    } else {
                        close += 1;
                        open[i] = false;
                        break;
                    }
                }
                (_, Some(_)) => {
                    if new.confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                        review += 1;
                    } else {
                        close += 1;
                        open[j] = false;
                    }
                }
            }
        }
    }
    (close, review)
}

/// 字面值给人看的样子：`{value, unit}` → "28000 CNY"，裸标量读成它自己。
fn literal_name(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) => {
            let value = match o.get("value") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => return v.to_string(),
            };
            match o
                .get("unit")
                .and_then(|u| u.as_str())
                .filter(|u| !u.is_empty())
            {
                Some(unit) => format!("{value} {unit}"),
                None => value,
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn v(from: Option<(i32, u32, u32)>, confidence: f32) -> OpenValue {
        OpenValue {
            fact_id: Uuid::now_v7(),
            name: String::new(),
            valid_from: from.map(|(y, m, d)| Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()),
            confidence,
        }
    }

    #[test]
    fn a_chain_of_three_closes_twice() {
        let values = [
            v(Some((2023, 2, 1)), 0.9),
            v(Some((2024, 7, 5)), 0.9),
            v(Some((2025, 9, 1)), 0.9),
        ];
        assert_eq!(plan_closures(&values), (2, 0));
    }

    #[test]
    fn what_the_engine_would_not_close_goes_to_review() {
        // 同一天开始
        assert_eq!(
            plan_closures(&[v(Some((2024, 1, 1)), 0.9), v(Some((2024, 1, 1)), 0.9)]),
            (0, 1)
        );
        // 两条都没起点
        assert_eq!(plan_closures(&[v(None, 0.9), v(None, 0.9)]), (0, 1));
        // 起点更早的那条置信度不够，不许它改写历史
        assert_eq!(
            plan_closures(&[v(Some((2023, 1, 1)), 0.5), v(Some((2024, 1, 1)), 0.9)]),
            (0, 1)
        );
        // 后任没起点：它止于前任的起点（落库时的"旧事实无起点也适用"）
        assert_eq!(
            plan_closures(&[v(Some((2023, 1, 1)), 0.9), v(None, 0.9)]),
            (1, 0)
        );
        // 一条不成对
        assert_eq!(plan_closures(&[v(Some((2023, 1, 1)), 0.9)]), (0, 0));
        // 三条都没起点：每一对都说不清，三对冲突，与引擎一致
        assert_eq!(
            plan_closures(&[v(None, 0.9), v(None, 0.9), v(None, 0.9)]),
            (0, 3)
        );
        // 有起点的一条闭合两条没起点的：它们都止于它的起点
        assert_eq!(
            plan_closures(&[v(Some((2023, 1, 1)), 0.9), v(None, 0.9), v(None, 0.9)]),
            (2, 0)
        );
    }

    #[test]
    fn a_literal_reads_as_a_value_with_its_unit() {
        assert_eq!(
            literal_name(&serde_json::json!({ "value": 28000, "unit": "CNY" })),
            "28000 CNY"
        );
        assert_eq!(
            literal_name(&serde_json::json!({ "value": "Staff Engineer" })),
            "Staff Engineer"
        );
        assert_eq!(literal_name(&serde_json::json!(32000)), "32000");
        assert_eq!(literal_name(&serde_json::json!("plain")), "plain");
    }
}
