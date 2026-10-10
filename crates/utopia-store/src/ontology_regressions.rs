//! 0061 cut 2 的第一片：人的例子比较现有对齐账本，不额外调用模型。
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, PgPool};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

use crate::phrase_bindings::PhraseSignature;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Case {
    pub id: Uuid,
    pub kb_id: Uuid,
    pub statement_id: Uuid,
    pub expected_property_id: Uuid,
    pub expected_direction: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub origin: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_result: Option<serde_json::Value>,
}

pub struct NewCase<'a> {
    pub statement_id: Uuid,
    pub expected_property_id: Uuid,
    pub expected_direction: &'a str,
    pub created_by: Uuid,
    pub origin: &'a str,
}

pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Case>> {
    Ok(sqlx::query_as(
        "SELECT c.* FROM ontology_regression_cases c WHERE c.kb_id=$1
        AND EXISTS(SELECT 1 FROM facts f WHERE f.id=c.statement_id AND f.kb_id=c.kb_id
          AND f.layer='open' AND f.invalidated_at IS NULL) ORDER BY c.created_at DESC,c.id",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<Option<Case>> {
    Ok(
        sqlx::query_as("SELECT * FROM ontology_regression_cases WHERE kb_id=$1 AND id=$2")
            .bind(kb_id)
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn add(pool: &PgPool, kb_id: Uuid, case: NewCase<'_>) -> AppResult<Uuid> {
    let mut tx = pool.begin().await?;
    let id = add_on(&mut tx, kb_id, case).await?;
    tx.commit().await?;
    Ok(id)
}

/// Adoption calls this in its own transaction so marking a proposal adopted and keeping
/// its examples cannot commit separately. Duplicate requests retain the first actor/origin.
pub async fn add_on(conn: &mut PgConnection, kb_id: Uuid, case: NewCase<'_>) -> AppResult<Uuid> {
    if !matches!(case.expected_direction, "forward" | "reverse") {
        return Err(AppError::invalid(
            "bad_direction",
            "Direction is forward or reverse.",
        ));
    }
    if !matches!(case.origin, "adoption" | "person") {
        return Err(AppError::invalid(
            "bad_origin",
            "Origin is adoption or person.",
        ));
    }
    let value: Option<bool> = sqlx::query_scalar(
        "SELECT object_id IS NULL FROM facts
        WHERE kb_id=$1 AND id=$2 AND layer='open' AND invalidated_at IS NULL
          AND nullif(btrim(phrase),'') IS NOT NULL FOR SHARE",
    )
    .bind(kb_id)
    .bind(case.statement_id)
    .fetch_optional(&mut *conn)
    .await?;
    let value = value.ok_or(AppError::NotFound)?;
    let kind: Option<String> =
        sqlx::query_scalar("SELECT kind FROM relation_types WHERE kb_id=$1 AND id=$2 FOR SHARE")
            .bind(kb_id)
            .bind(case.expected_property_id)
            .fetch_optional(&mut *conn)
            .await?;
    let kind = kind.ok_or(AppError::NotFound)?;
    if value != (kind == "attribute") || (value && case.expected_direction != "forward") {
        return Err(AppError::invalid(
            "bad_property_shape",
            "The property's kind and direction must fit the statement's object.",
        ));
    }
    // DO NOTHING followed by SELECT observes a concurrent inserter after its commit,
    // without turning an idempotent retry into an edit of the original confirmation.
    let inserted:Option<Uuid>=sqlx::query_scalar("INSERT INTO ontology_regression_cases
        (id,kb_id,statement_id,expected_property_id,expected_direction,created_by,origin)
        VALUES($1,$2,$3,$4,$5,$6,$7)
        ON CONFLICT(kb_id,statement_id,expected_property_id,expected_direction) DO NOTHING RETURNING id")
        .bind(Uuid::now_v7()).bind(kb_id).bind(case.statement_id).bind(case.expected_property_id)
        .bind(case.expected_direction).bind(case.created_by).bind(case.origin).fetch_optional(&mut *conn).await?;
    let id = match inserted {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                "SELECT id FROM ontology_regression_cases WHERE kb_id=$1 AND statement_id=$2
            AND expected_property_id=$3 AND expected_direction=$4",
            )
            .bind(kb_id)
            .bind(case.statement_id)
            .bind(case.expected_property_id)
            .bind(case.expected_direction)
            .fetch_one(&mut *conn)
            .await?
        }
    };
    // A pre-existing decision is reported at its original time, never as a new model check.
    sqlx::query(&format!(
        "{} AND c.id=$2 AND c.last_checked_at IS NULL",
        record_sql(false)
    ))
    .bind(kb_id)
    .bind(id)
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

// The live statement determines the signature, exactly as production alignment does.
// A person's binding remains authoritative even when it differs from the example; the
// human_bound flag distinguishes that protection from an independent machine success.
fn record_sql(fresh: bool) -> String {
    let checked_at = if fresh { "now()" } else { "b.decided_at" };
    format!("UPDATE ontology_regression_cases c
    SET last_checked_at={checked_at},
        last_result=jsonb_build_object('passed',coalesce((b.decided_by='person' AND b.status='bound') OR
          (b.status='bound' AND b.relation_type_id=c.expected_property_id AND b.direction=c.expected_direction),false),
          'human_bound',b.decided_by='person' AND b.status='bound','actual_property_id',b.relation_type_id,
          'actual_direction',b.direction,'status',b.status,'decided_at',b.decided_at)
    FROM facts f JOIN entities s ON s.id=f.subject_id AND s.kb_id=f.kb_id
    LEFT JOIN entities o ON o.id=f.object_id AND o.kb_id=f.kb_id
    JOIN phrase_bindings b ON b.kb_id=f.kb_id
      AND b.phrase=lower(btrim(regexp_replace(f.phrase,'\\s+',' ','g')))
      AND b.subject_type_id IS NOT DISTINCT FROM s.type_id
      AND b.object_type_id IS NOT DISTINCT FROM o.type_id
      AND b.object_is_value=(f.object_id IS NULL)
    WHERE c.kb_id=$1 AND c.statement_id=f.id AND f.kb_id=c.kb_id
      AND f.layer='open' AND f.invalidated_at IS NULL")
}

pub async fn record_for_signature(
    conn: &mut PgConnection,
    kb_id: Uuid,
    sig: &PhraseSignature,
) -> AppResult<()> {
    sqlx::query(&format!(
        "{} AND b.phrase=$2
        AND b.subject_type_id IS NOT DISTINCT FROM $3 AND b.object_type_id IS NOT DISTINCT FROM $4
        AND b.object_is_value=$5",
        record_sql(true)
    ))
    .bind(kb_id)
    .bind(crate::phrase_bindings::normalize(&sig.phrase))
    .bind(sig.subject_type_id)
    .bind(if sig.object_is_value {
        None
    } else {
        sig.object_type_id
    })
    .bind(sig.object_is_value)
    .execute(conn)
    .await?;
    Ok(())
}

/// Capture protected human choices when production skips them, without re-reading cached
/// agent outputs as though they had just been evaluated after an unsuccessful model call.
pub async fn record_human(pool: &PgPool, kb_id: Uuid) -> AppResult<()> {
    sqlx::query(&format!("{} AND b.decided_by='person'", record_sql(true)))
        .bind(kb_id)
        .execute(pool)
        .await?;
    Ok(())
}
