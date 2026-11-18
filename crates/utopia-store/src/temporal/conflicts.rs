use super::*;
use serde_json::json;
use utopia_core::AppError;

pub struct ConflictDecision<'a> {
    pub action: &'a str,
    pub close_at: Option<DateTime<Utc>>,
    pub close_precision: &'a str,
    pub new_from: Option<DateTime<Utc>>,
    pub new_precision: &'a str,
}

/// The ordinary card and subsequent proposal answers share this human writer.
pub async fn resolve_review_conflict(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    decision: ConflictDecision<'_>,
    actor_id: Uuid,
) -> AppResult<()> {
    resolve(pool, kb_id, conflict_id, decision, Some(actor_id)).await
}

/// 不记操作者的入口：只有 store 的测试在用，人工审核走 [`resolve_review_conflict`]。
pub async fn resolve_conflict(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    action: &str,
    close_at: Option<DateTime<Utc>>,
    close_precision: &str,
) -> AppResult<()> {
    resolve(
        pool,
        kb_id,
        conflict_id,
        ConflictDecision {
            action,
            close_at,
            close_precision,
            new_from: None,
            new_precision: "day",
        },
        None,
    )
    .await
}

async fn resolve(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    decision: ConflictDecision<'_>,
    actor_id: Option<Uuid>,
) -> AppResult<()> {
    let (action, event) = match decision.action {
        "close" => ("close_old", "conflict.close_old"),
        "retime_new" => ("retime_new", "conflict.retime_new"),
        "keep" => ("keep", "conflict.keep_both"),
        "reject_new" => ("reject_new", "conflict.reject_new"),
        other => return Err(AppError::Validation(format!("Unknown resolution: {other}"))),
    };
    for precision in [decision.close_precision, decision.new_precision] {
        if !crate::graph::WORLD_PRECISIONS.contains(&precision) {
            return Err(AppError::invalid(
                "bad_precision",
                "Unknown date precision.",
            ));
        }
    }
    let mut tx = pool.begin().await?;
    let pair: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT old_fact_id, new_fact_id FROM fact_conflicts
         WHERE kb_id = $1 AND id = $2 AND status = 'open'",
    )
    .bind(kb_id)
    .bind(conflict_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (old, new) = pair.ok_or(AppError::NotFound)?;
    // Reconciliation takes timeline locks before fact/conflict rows. Re-read the
    // pair afterwards: a rewrite may have carried this conflict to different rows.
    let timelines = timelines_of(&mut *tx, kb_id, &[old, new], None).await?;
    lock_timelines(&mut tx, kb_id, &timelines).await?;
    sqlx::query("SELECT id FROM facts WHERE kb_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE")
        .bind(kb_id)
        .bind([old, new])
        .fetch_all(&mut *tx)
        .await?;
    let current: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT c.old_fact_id, c.new_fact_id FROM fact_conflicts c
         JOIN facts a ON a.id = c.old_fact_id JOIN facts b ON b.id = c.new_fact_id
         WHERE c.kb_id = $1 AND c.id = $2 AND c.status = 'open'
           AND a.invalidated_at IS NULL AND b.invalidated_at IS NULL FOR UPDATE OF c",
    )
    .bind(kb_id)
    .bind(conflict_id)
    .fetch_optional(&mut *tx)
    .await?;
    if current != pair {
        return Err(AppError::Conflict(
            "the conflict changed before it could be decided".into(),
        ));
    }
    let (new_from, new_precision): (Option<DateTime<Utc>>, Option<String>) =
        sqlx::query_as("SELECT valid_from, valid_from_precision FROM facts WHERE id = $1")
            .bind(new)
            .fetch_one(&mut *tx)
            .await?;
    let mut detail: serde_json::Value = sqlx::query_scalar(
        "SELECT jsonb_build_object('predicate', r.label,
                   'old_subject', os.canonical_name, 'old_object', COALESCE(to_jsonb(oo.canonical_name), fo.object_value),
                   'new_subject', ns.canonical_name, 'new_object', COALESCE(to_jsonb(no_.canonical_name), fn_.object_value),
                   'old_fact_id', fo.id, 'new_fact_id', fn_.id)
         FROM facts fo JOIN facts fn_ ON fn_.id = $2
         JOIN entities os ON os.id = fo.subject_id JOIN entities ns ON ns.id = fn_.subject_id
         JOIN relation_types r ON r.id = fo.predicate_id
         LEFT JOIN entities oo ON oo.id = fo.object_id LEFT JOIN entities no_ ON no_.id = fn_.object_id
         WHERE fo.id = $1",
    ).bind(old).bind(new).fetch_one(&mut *tx).await?;
    let stored = match action {
        "close_old" => {
            // 闭合点带着它的精度走：人给了日期就用人给的精度，没给就闭合在新事实的
            // 起点——那个起点是几月还是几号，闭合点就是几月还是几号。从前一律写 day，
            // 「2023 年 6 月接任」把前任闭合成了 6 月 1 日
            let (at, precision) = match decision.close_at {
                Some(at) => (at, decision.close_precision),
                None => (
                    new_from.ok_or_else(|| {
                        AppError::invalid(
                            "close_at_required",
                            "close_at is required when the new fact has no start time",
                        )
                    })?,
                    new_precision.as_deref().unwrap_or("day"),
                ),
            };
            let corrected = close_superseded(&mut *tx, old, at, precision)
                .await?
                .ok_or(AppError::NotFound)?;
            detail["close_at"] = json!(crate::graph::truncate_to(at, Some(precision)));
            detail["close_at_precision"] = json!(precision);
            detail["corrected_fact_id"] = json!(corrected);
            "closed"
        }
        "retime_new" => {
            let at = decision.new_from.ok_or_else(|| {
                AppError::invalid(
                    "new_from_required",
                    "A new start is required when retiming a fact.",
                )
            })?;
            let corrected = rewrite_start(&mut tx, new, at, decision.new_precision).await?;
            detail["new_from"] = json!(crate::graph::truncate_to(at, Some(decision.new_precision)));
            detail["new_from_precision"] = json!(decision.new_precision);
            detail["corrected_fact_id"] = json!(corrected);
            "retimed_new"
        }
        "keep" => "kept_both",
        _ => {
            // Invalidation's trigger withdraws these rows. Settle the siblings
            // first so they retain the human rejection rather than a withdrawal.
            sqlx::query("UPDATE fact_conflicts SET status = 'resolved', resolution = 'rejected_new', resolved_at = now()
                         WHERE kb_id = $1 AND new_fact_id = $2 AND status = 'open'")
                .bind(kb_id).bind(new).execute(&mut *tx).await?;
            retract(&mut *tx, kb_id, new).await?;
            "rejected_new"
        }
    };
    sqlx::query(
        "UPDATE fact_conflicts SET status = 'resolved', resolution = $3, resolved_at = now()
                 WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(conflict_id)
    .bind(stored)
    .execute(&mut *tx)
    .await?;
    // The settlement is visible to reconciliation in this same transaction.
    // Explicit ends stay fixed; only the engine's ends can be redrawn.
    if matches!(action, "close_old" | "retime_new") {
        tidy_timelines_tx(&mut tx, kb_id, &timelines).await?;
    }
    crate::audit::record_on(
        &mut tx,
        Some(kb_id),
        actor_id,
        event,
        "conflict",
        Some(conflict_id),
        detail,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// A start edit must not turn an engine-derived end into a person's fixed end.
/// Preserve the full row and its provenance, unlike an edit of the entire span.
async fn rewrite_start(
    tx: &mut Transaction<'_, Postgres>,
    fact_id: Uuid,
    at: DateTime<Utc>,
    precision: &str,
) -> AppResult<Uuid> {
    let (predicate, to, to_precision, derived): (
        Option<Uuid>,
        Option<DateTime<Utc>>,
        Option<String>,
        bool,
    ) = sqlx::query_as(
        "SELECT predicate_id, valid_to, valid_to_precision, end_derived FROM facts WHERE id = $1",
    )
    .bind(fact_id)
    .fetch_one(&mut **tx)
    .await?;
    let from = crate::graph::truncate_to(at, Some(precision));
    let temporal = crate::graph::predicate_temporal(&mut **tx, predicate).await?;
    if temporal != crate::graph::Temporal::State {
        return Err(AppError::invalid(
            "not_a_state",
            "Only a state has a start to retime.",
        ));
    }
    if !derived {
        if to.is_some_and(|end| from > crate::graph::truncate_to(end, to_precision.as_deref())) {
            return Err(AppError::invalid(
                "interval_inverted",
                "The start of the interval is after its end.",
            ));
        }
        crate::graph::Validity {
            from: Some(from),
            from_precision: Some(precision),
            to,
            to_precision: to_precision.as_deref(),
            ..Default::default()
        }
        .under(temporal)?;
    }
    let corrected = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value,
                            valid_from, valid_from_precision, valid_from_grade, valid_to, valid_to_precision,
                            layer, confidence, derived_by_rule, supersedes, attested_from, attested_by,
                            attested_to, end_derived, from_statement_id, implied, corrected_ends, end_after_period)
         SELECT $1, kb_id, subject_id, predicate_id, object_id, object_value,
                $3, $4, NULL, valid_to, valid_to_precision,
                layer, confidence, derived_by_rule, id, attested_from, attested_by,
                attested_to, end_derived, from_statement_id, implied,
                CASE WHEN corrected_ends IN ('end', 'both') THEN 'both' ELSE 'start' END, end_after_period
         FROM facts WHERE id = $2",
    ).bind(corrected).bind(fact_id).bind(from).bind(precision).execute(&mut **tx).await?;
    carry_open_conflicts(tx, fact_id, corrected).await?;
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(fact_id)
        .execute(&mut **tx)
        .await?;
    copy_evidence(tx, fact_id, corrected).await?;
    copy_qualifiers(tx, fact_id, corrected).await?;
    copy_materialization_links(tx, fact_id, corrected).await?;
    Ok(corrected)
}
