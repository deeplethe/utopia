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
    new_valid_from: Option<DateTime<Utc>>,
    new_valid_to: Option<DateTime<Utc>>,
    new_confidence: f32,
) -> AppResult<ReconcileReport> {
    // 已闭区间的新事实是历史陈述，不威胁"开放期唯一"不变量——不触发任何改写
    // （闭区间之间的重叠矛盾是更细的区间代数，暂不自动裁，留给 Review 的人眼）
    if new_valid_to.is_some() {
        return Ok(ReconcileReport::default());
    }
    // 宾语侧唯一性只对实体宾语有意义（字面值不"被占用"）
    if matches!(direction, Uniqueness::ObjectSide) && object_id.is_none() {
        return Ok(ReconcileReport::default());
    }
    // 不变量点查：主语侧 = 同 (kb, S, P) 宾语不同；宾语侧 = 同 (kb, P, O) 主语不同
    let sql = match direction {
        Uniqueness::SubjectSide => {
            "SELECT id, valid_from FROM facts
             WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3
               AND valid_to IS NULL AND invalidated_at IS NULL
               AND id <> $4
               AND (object_id IS DISTINCT FROM $5 OR object_value IS DISTINCT FROM $6)"
        }
        Uniqueness::ObjectSide => {
            "SELECT id, valid_from FROM facts
             WHERE kb_id = $1 AND object_id = $5 AND predicate_id = $3
               AND valid_to IS NULL AND invalidated_at IS NULL
               AND id <> $4 AND subject_id IS DISTINCT FROM $2"
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
        match (old.valid_from, new_valid_from) {
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
                    report
                        .corrected
                        .push(close_superseded(pool, new_fact_id, of).await?);
                }
            }
            // 常规接替：旧事实闭合在新事实的开始（旧事实无起点也适用——起点未知但已结束）
            (_, Some(nf)) => {
                if new_confidence < AUTO_CLOSE_MIN_CONFIDENCE {
                    record_conflict(pool, kb_id, old.id, new_fact_id, "low_confidence").await?;
                    report.conflicts += 1;
                } else {
                    report
                        .corrected
                        .push(close_superseded(pool, old.id, nf).await?);
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
    #[derive(sqlx::FromRow)]
    struct MovedFact {
        id: Uuid,
        subject_id: Uuid,
        predicate_id: Uuid,
        object_id: Option<Uuid>,
        object_value: Option<serde_json::Value>,
        valid_from: Option<DateTime<Utc>>,
        confidence: f32,
        functional: bool,
        inverse_functional: bool,
    }
    let rows: Vec<MovedFact> = sqlx::query_as(
        "SELECT f.id, f.subject_id, f.predicate_id, f.object_id, f.object_value, f.valid_from,
                f.confidence, r.functional, r.inverse_functional
         FROM facts f JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND f.id = ANY($2)
           AND f.invalidated_at IS NULL AND f.valid_to IS NULL
           AND r.temporal = 'state' AND (r.functional OR r.inverse_functional)
         ORDER BY f.recorded_at",
    )
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
                f.valid_from,
                None,
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
/// （世界轴），证据引用随行复制。返回修正行 id。
pub async fn close_superseded(
    pool: &PgPool,
    fact_id: Uuid,
    valid_to: DateTime<Utc>,
) -> AppResult<Uuid> {
    let mut tx = pool.begin().await?;
    let corrected = Uuid::now_v7();
    let inserted: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_to, valid_precision, confidence, supersedes)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                valid_from, $3, valid_precision, confidence, id
         FROM facts WHERE id = $2 AND invalidated_at IS NULL
         RETURNING id",
    )
    .bind(corrected)
    .bind(fact_id)
    .bind(valid_to)
    .fetch_optional(&mut *tx)
    .await?;
    // 已被并发修正过：不重复动手
    if inserted.is_none() {
        tx.rollback().await?;
        return Ok(fact_id);
    }
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        // 表层谓词随证据一起搬：纠正的是时间区间，不是原文说了什么
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, surface_predicate, document_id, doc_version)
         SELECT $1, chunk_id, quote, surface_predicate, document_id, doc_version
         FROM fact_evidence WHERE fact_id = $2
         ON CONFLICT DO NOTHING",
    )
    .bind(corrected)
    .bind(fact_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(corrected)
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
pub async fn list_conflicts(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ConflictView>> {
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
         LIMIT 200",
    )
    .bind(kb_id)
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
                utopia_core::AppError::Validation(
                    "close_at is required when the new fact has no start time".into(),
                )
            })?;
            close_superseded(pool, old_fact_id, at).await?;
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
