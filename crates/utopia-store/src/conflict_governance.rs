//! The conflict part of 0043, on the typed graph. Model work happens outside a
//! transaction. Applying or answering a verdict re-reads its inputs under the
//! timeline locks; the graph change and its decision commit together.

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

use crate::temporal;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
pub struct Fact {
    pub id: Uuid,
    pub subject_id: Uuid,
    pub predicate_id: Option<Uuid>,
    pub object_id: Option<Uuid>,
    pub subject: String,
    pub predicate: String,
    pub property: Value,
    pub object: String,
    pub qualifiers: Value,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_from_precision: Option<String>,
    pub valid_from_grade: Option<String>,
    pub valid_to: Option<DateTime<Utc>>,
    pub valid_to_precision: Option<String>,
    pub attested_from: Option<DateTime<Utc>>,
    pub attested_to: Option<DateTime<Utc>>,
    pub end_derived: bool,
    pub corrected_ends: Option<String>,
    pub confidence: f32,
    pub invalidated_at: Option<DateTime<Utc>>,
    pub implied: bool,
    pub derived_by_rule: Option<Uuid>,
    pub memory: bool,
    pub described: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
pub struct Evidence {
    pub chunk_id: Uuid,
    pub document_id: Uuid,
    pub document: String,
    pub dated: Option<DateTime<Utc>>,
    pub quote: Option<String>,
    pub described: bool,
    pub memory: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Case {
    pub id: Uuid,
    pub reason: String,
    pub old: Fact,
    pub new: Fact,
    pub old_evidence: Vec<Evidence>,
    pub new_evidence: Vec<Evidence>,
    pub neighbours: Vec<Fact>,
    pub pair_key: String,
}

impl Case {
    pub fn summary(&self) -> String {
        format!(
            "{} — {} → {} / {} — {} → {}",
            self.old.subject,
            self.old.predicate,
            self.old.object,
            self.new.subject,
            self.new.predicate,
            self.new.object
        )
    }
}

/// Fixed aliases c / old / new. Both roots are followed through supersedes, so a
/// rewritten or restored pair cannot undo the person's revert by entering anew.
const PAIR_KEY: &str = "(
    WITH RECURSIVE a(id, supersedes) AS (
        SELECT id, supersedes FROM facts WHERE id = c.old_fact_id
        UNION SELECT f.id, f.supersedes FROM facts f JOIN a ON f.id = a.supersedes),
      b(id, supersedes) AS (
        SELECT id, supersedes FROM facts WHERE id = c.new_fact_id
        UNION SELECT f.id, f.supersedes FROM facts f JOIN b ON f.id = b.supersedes),
      roots AS (SELECT
        COALESCE((SELECT id FROM a WHERE supersedes IS NULL LIMIT 1), c.old_fact_id) AS a,
        COALESCE((SELECT id FROM b WHERE supersedes IS NULL LIMIT 1), c.new_fact_id) AS b)
    SELECT least(a,b)::text || ':' || greatest(a,b)::text FROM roots)";

pub(crate) fn waiting_sql() -> String {
    format!(
        "c.status = 'open'
        AND EXISTS (SELECT 1 FROM facts f WHERE f.id = c.old_fact_id
                    AND f.kb_id = c.kb_id AND f.layer = 'typed' AND f.invalidated_at IS NULL)
        AND EXISTS (SELECT 1 FROM facts f WHERE f.id = c.new_fact_id
                    AND f.kb_id = c.kb_id AND f.layer = 'typed' AND f.invalidated_at IS NULL)
        AND NOT EXISTS (SELECT 1 FROM agent_decisions d
            WHERE d.kb_id = c.kb_id AND d.target_kind = 'conflict'
              AND d.status <> 'superseded'
              AND (d.target_id = c.id OR d.detail->>'pair_key' = {PAIR_KEY}
                OR (d.status = 'reverted' AND d.detail->'undo'->'affected_pairs' ? {PAIR_KEY})))"
    )
}

pub async fn queue(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(&format!(
        "SELECT c.id FROM fact_conflicts c WHERE c.kb_id = $1 AND {}
         ORDER BY c.created_at, c.id LIMIT $2",
        waiting_sql()
    ))
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

const FACT: &str = "SELECT f.id, f.subject_id, f.predicate_id, f.object_id,
    s.canonical_name AS subject, COALESCE(r.label, f.phrase, '?') AS predicate,
    jsonb_build_object('description',r.description,'temporal',r.temporal,
        'functional',r.functional,'inverse_functional',r.inverse_functional) AS property,
    COALESCE(o.canonical_name, f.object_value::text, '?') AS object,
    COALESCE((SELECT jsonb_agg(jsonb_build_object('property',qr.label,'value',q.value,
                'entity',qe.canonical_name) ORDER BY q.qualifier_type_id)
              FROM fact_qualifiers q JOIN relation_types qr ON qr.id = q.qualifier_type_id
              LEFT JOIN entities qe ON qe.id = q.entity_id WHERE q.fact_id = f.id), '[]') AS qualifiers,
    f.valid_from, f.valid_from_precision, f.valid_from_grade, f.valid_to, f.valid_to_precision,
    f.attested_from, f.attested_to, f.end_derived, f.corrected_ends,
    f.confidence, f.invalidated_at, f.implied, f.derived_by_rule,
    provenance.memory, provenance.described
    FROM facts f JOIN entities s ON s.id = f.subject_id
    LEFT JOIN relation_types r ON r.id = f.predicate_id
    LEFT JOIN entities o ON o.id = f.object_id
    CROSS JOIN LATERAL (
        WITH source_ids(id) AS (
            SELECT f.id
            UNION SELECT statement_id FROM typed_fact_sources WHERE fact_id = f.id
            UNION SELECT statement_id FROM implied_fact_sources WHERE fact_id = f.id)
        SELECT COALESCE(bool_or(s.kind = 'memory'), false) AS memory,
               COALESCE(bool_or(ch.origin = 'described'), false) AS described
        FROM source_ids src JOIN fact_evidence e ON e.fact_id = src.id
        JOIN chunks ch ON ch.id = e.chunk_id
        JOIN documents d ON d.id = ch.document_id
        LEFT JOIN sources s ON s.id = d.source_id
        WHERE ch.superseded_at IS NULL AND d.deleted_at IS NULL
    ) provenance
    WHERE f.kb_id = $1 AND f.id = $2 AND f.layer = 'typed'";

async fn fact(conn: &mut PgConnection, kb_id: Uuid, id: Uuid) -> AppResult<Option<Fact>> {
    Ok(sqlx::query_as(FACT)
        .bind(kb_id)
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

async fn evidence(conn: &mut PgConnection, kb_id: Uuid, id: Uuid) -> AppResult<Vec<Evidence>> {
    Ok(sqlx::query_as(
        "WITH source_ids(id) AS (
            SELECT $2::uuid
            UNION SELECT statement_id FROM typed_fact_sources WHERE fact_id = $2
            UNION SELECT statement_id FROM implied_fact_sources WHERE fact_id = $2)
         SELECT DISTINCT ch.id AS chunk_id, d.id AS document_id, d.filename AS document,
            CASE WHEN d.doc_time_source IN ('content', 'source') THEN d.doc_time END AS dated,
            CASE WHEN fe.quote_start IS NOT NULL AND fe.quote_end > fe.quote_start
                 THEN substring(ch.text FROM fe.quote_start + 1 FOR fe.quote_end - fe.quote_start)
                 ELSE fe.quote END AS quote,
            ch.origin = 'described' AS described, COALESCE(s.kind = 'memory', false) AS memory
         FROM source_ids src JOIN fact_evidence fe ON fe.fact_id = src.id
         JOIN chunks ch ON ch.id = fe.chunk_id AND ch.kb_id = $1
         JOIN documents d ON d.id = ch.document_id AND d.kb_id = $1
         LEFT JOIN sources s ON s.id = d.source_id
         WHERE ch.superseded_at IS NULL AND d.deleted_at IS NULL
         ORDER BY document_id, chunk_id, quote LIMIT 8",
    )
    .bind(kb_id)
    .bind(id)
    .fetch_all(conn)
    .await?)
}

pub async fn load(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<Option<Case>> {
    load_conn(&mut *pool.acquire().await?, kb_id, id).await
}

async fn load_conn(conn: &mut PgConnection, kb_id: Uuid, id: Uuid) -> AppResult<Option<Case>> {
    let row: Option<(String, Uuid, Uuid, String)> = sqlx::query_as(&format!(
        "SELECT c.reason, c.old_fact_id, c.new_fact_id, {PAIR_KEY} AS pair_key
         FROM fact_conflicts c WHERE c.kb_id = $1 AND c.id = $2 AND c.status = 'open'"
    ))
    .bind(kb_id)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((reason, old_id, new_id, pair_key)) = row else {
        return Ok(None);
    };
    let (Some(old), Some(new)) = (
        fact(conn, kb_id, old_id).await?,
        fact(conn, kb_id, new_id).await?,
    ) else {
        return Ok(None);
    };
    if old.invalidated_at.is_some() || new.invalidated_at.is_some() {
        return Ok(None);
    }
    let old_evidence = evidence(conn, kb_id, old_id).await?;
    let new_evidence = evidence(conn, kb_id, new_id).await?;
    let neighbours = neighbours(conn, kb_id, &old, &new).await?;
    Ok(Some(Case {
        id,
        reason,
        old,
        new,
        old_evidence,
        new_evidence,
        neighbours,
        pair_key,
    }))
}

pub async fn precedents(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Value>> {
    let rows: Vec<(Uuid, String, Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, action, detail, created_at FROM audit_events
         WHERE kb_id = $1 AND actor_id IS NOT NULL AND action IN
           ('conflict.close_old', 'conflict.retime_new', 'conflict.keep_both',
            'conflict.reject_new', 'conflict.revert')
         ORDER BY created_at DESC, id DESC LIMIT 12",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, action, detail, at)| {
            json!({"family":"conflict", "event_id":id, "action":action,
               "detail":detail, "at":at})
        })
        .collect())
}

// Neighbours are part of what the model saw, so re-read them with the pair when
// checking freshness. Loading them separately used to accept an obsolete context.
async fn neighbours(
    conn: &mut PgConnection,
    kb_id: Uuid,
    old: &Fact,
    new: &Fact,
) -> AppResult<Vec<Fact>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT f.id FROM facts f JOIN relation_types r ON r.id = f.predicate_id
         WHERE f.kb_id = $1 AND f.layer = 'typed' AND f.invalidated_at IS NULL
           AND f.predicate_id = $2 AND f.id <> ALL($3)
           AND ((r.functional AND f.subject_id = ANY($4))
             OR (r.inverse_functional AND f.object_id = ANY($5)))
         ORDER BY f.valid_from NULLS LAST, f.id LIMIT 8",
    )
    .bind(kb_id)
    .bind(old.predicate_id)
    .bind([old.id, new.id])
    .bind([old.subject_id, new.subject_id])
    .bind([old.object_id, new.object_id])
    .fetch_all(&mut *conn)
    .await?;
    let mut rows = Vec::new();
    for id in ids {
        if let Some(f) = fact(conn, kb_id, id).await? {
            rows.push(f)
        }
    }
    Ok(rows)
}

/// The engine's lock order is timeline, fact, conflict. Never retain these locks
/// across a model call. Re-read after locking, since a concurrent rewrite may
/// have changed the IDs while this transaction was waiting.
async fn lock_case(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    expected: &Case,
) -> AppResult<bool> {
    match temporal::lock_conflict(tx, kb_id, expected.id).await {
        Ok(()) => {}
        Err(AppError::NotFound | AppError::Conflict(_)) => return Ok(false),
        Err(e) => return Err(e),
    }
    Ok(load_conn(tx, kb_id, expected.id).await?.as_ref() == Some(expected))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Parameters {
    pub date: Option<DateTime<Utc>>,
    pub precision: Option<String>,
}

impl Parameters {
    fn effective(&self, item: &Case, action: &str) -> AppResult<Self> {
        if !matches!(action, "close_old" | "retime_new") {
            return Ok(Self::default());
        }
        let (date, precision) = match self.date {
            Some(date) => (date, self.precision.as_deref().unwrap_or("day")),
            None if action == "close_old" => (
                item.new.valid_from.ok_or_else(|| {
                    AppError::invalid(
                        "close_at_required",
                        "The new fact has no start; supply a close date.",
                    )
                })?,
                item.new.valid_from_precision.as_deref().unwrap_or("day"),
            ),
            None => {
                return Err(AppError::invalid(
                    "date_required",
                    "Retiming a fact requires a date.",
                ))
            }
        };
        if !matches!(
            precision,
            "year" | "month" | "day" | "hour" | "minute" | "second"
        ) {
            return Err(AppError::invalid(
                "bad_precision",
                "Unknown date precision.",
            ));
        }
        Ok(Self {
            date: Some(crate::graph::truncate_to(date, Some(precision))),
            precision: Some(precision.into()),
        })
    }
}

fn date_supported(params: &Parameters, item: &Case) -> bool {
    let (Some(date), Some(precision)) = (params.date, params.precision.as_deref()) else {
        return true;
    };
    if item.new.valid_from == Some(date)
        && item.new.valid_from_precision.as_deref() == Some(precision)
    {
        return true;
    }
    if precision == "day"
        && item
            .new_evidence
            .iter()
            .any(|e| !e.described && e.dated.is_some_and(|d| d.date_naive() == date.date_naive()))
    {
        return true;
    }
    let forms = match precision {
        "year" => vec![format!("in {}", date.year()), format!("{}年", date.year())],
        "month" => vec![
            date.format("%Y-%m").to_string(),
            date.format("%B %Y").to_string(),
            format!("{}年{}月", date.year(), date.month()),
        ],
        "day" => vec![
            date.format("%Y-%m-%d").to_string(),
            date.format("%B %-d, %Y").to_string(),
            date.format("%-d %B %Y").to_string(),
            date.format("%b %-d, %Y").to_string(),
            format!("{}年{}月{}日", date.year(), date.month(), date.day()),
        ],
        _ => return false,
    };
    item.new_evidence
        .iter()
        .filter(|e| !e.described)
        .filter_map(|e| e.quote.as_deref())
        .any(|quote| {
            let text = quote
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            forms.iter().any(|f| {
                let form = f.to_lowercase();
                text.match_indices(&form).any(|(i, _)| {
                    let before = text[..i].chars().next_back();
                    let after = text[i + form.len()..].chars().next();
                    !before.is_some_and(|c| c.is_ascii_digit())
                        && !after.is_some_and(|c| {
                            c.is_ascii_digit() || (precision != "day" && matches!(c, '-' | '/'))
                        })
                })
            })
        })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
struct ConflictState {
    id: Uuid,
    pair_key: String,
    old_fact_id: Uuid,
    new_fact_id: Uuid,
    reason: String,
    status: String,
    resolution: Option<String>,
    resolved_at: Option<DateTime<Utc>>,
}

async fn conflicts_of(
    conn: &mut PgConnection,
    kb_id: Uuid,
    facts: &[Uuid],
) -> AppResult<Vec<ConflictState>> {
    Ok(sqlx::query_as(&format!("SELECT c.id, {PAIR_KEY} AS pair_key,
        c.old_fact_id, c.new_fact_id, c.reason, c.status, c.resolution, c.resolved_at
        FROM fact_conflicts c WHERE c.kb_id = $1 AND (c.old_fact_id = ANY($2) OR c.new_fact_id = ANY($2)) ORDER BY c.id"))
        .bind(kb_id).bind(facts).fetch_all(conn).await?)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Footprint {
    facts: Vec<Value>,
    conflicts: Vec<ConflictState>,
    associations: Value,
}

async fn footprint(conn: &mut PgConnection, kb_id: Uuid, roots: &[Uuid]) -> AppResult<Footprint> {
    let facts: Vec<Value> = sqlx::query_scalar(
        "WITH RECURSIVE family(id) AS (
            SELECT id FROM facts WHERE kb_id = $1 AND id = ANY($2)
            UNION SELECT f.id FROM facts f JOIN family p ON f.supersedes = p.id WHERE f.kb_id = $1)
         SELECT to_jsonb(f) FROM facts f JOIN family p ON p.id = f.id ORDER BY f.id",
    )
    .bind(kb_id)
    .bind(roots)
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = facts
        .iter()
        .filter_map(|f| f["id"].as_str()?.parse().ok())
        .collect();
    let conflicts = conflicts_of(conn, kb_id, &ids).await?;
    let associations: Value = sqlx::query_scalar(
        "WITH source_ids(id) AS (
            SELECT unnest($1::uuid[])
            UNION SELECT statement_id FROM typed_fact_sources WHERE fact_id = ANY($1)
            UNION SELECT statement_id FROM implied_fact_sources WHERE fact_id = ANY($1))
         SELECT jsonb_build_object(
           'evidence', COALESCE((SELECT jsonb_agg(v ORDER BY v::text) FROM (
             SELECT jsonb_build_object('row',to_jsonb(e),'superseded',ch.superseded_at,
                 'deleted',d.deleted_at,'dated',d.doc_time,'date_source',d.doc_time_source) AS v
             FROM source_ids s JOIN fact_evidence e ON e.fact_id = s.id
             JOIN chunks ch ON ch.id = e.chunk_id JOIN documents d ON d.id = ch.document_id) evidence), '[]'),
           'qualifiers', COALESCE((SELECT jsonb_agg(to_jsonb(q) ORDER BY q.fact_id,q.qualifier_type_id)
                FROM fact_qualifiers q WHERE q.fact_id = ANY($1)), '[]'),
           'properties', COALESCE((SELECT jsonb_agg(jsonb_build_object('id',r.id,
                'description',r.description,'temporal',r.temporal,'functional',r.functional,
                'inverse_functional',r.inverse_functional) ORDER BY r.id)
                FROM relation_types r WHERE r.id IN (SELECT predicate_id FROM facts WHERE id = ANY($1))), '[]'))")
        .bind(&ids).fetch_one(conn).await?;
    Ok(Footprint {
        facts,
        conflicts,
        associations,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Inverse {
    Keep,
    Restore { fact: Uuid },
    Rewrite { original: Uuid, corrected: Uuid },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Undo {
    inverse: Inverse,
    roots: Vec<Uuid>,
    before_conflicts: Vec<ConflictState>,
    affected_pairs: Vec<String>,
    after: Footprint,
}

async fn perform(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    item: &Case,
    action: &str,
    params: &Parameters,
) -> AppResult<Undo> {
    let mut roots = vec![item.old.id, item.new.id];
    let before_conflicts: Vec<ConflictState> = conflicts_of(tx, kb_id, &roots)
        .await?
        .into_iter()
        .filter(|c| c.status == "open")
        .collect();
    for c in &before_conflicts {
        roots.extend([c.old_fact_id, c.new_fact_id])
    }
    let precision = params.precision.as_deref().unwrap_or("day");
    let inverse = match action {
        "keep_both" => {
            temporal::resolve_conflict_in(tx, kb_id, item.id, "keep", None, "day").await?;
            Inverse::Keep
        }
        "close_old" => {
            let corrected =
                temporal::resolve_conflict_in(tx, kb_id, item.id, "close", params.date, precision)
                    .await?
                    .ok_or_else(|| AppError::Conflict("the old fact was not closed".into()))?;
            Inverse::Rewrite {
                original: item.old.id,
                corrected,
            }
        }
        "reject_new" => {
            temporal::resolve_conflict_in(tx, kb_id, item.id, "reject_new", None, "day").await?;
            Inverse::Restore { fact: item.new.id }
        }
        "retime_new" => {
            let corrected = temporal::correct_interval(
                &mut **tx,
                item.new.id,
                crate::graph::Validity {
                    from: params.date,
                    from_precision: Some(precision),
                    from_grade: None,
                    to: item.new.valid_to,
                    to_precision: item.new.valid_to_precision.as_deref(),
                    attested_at: None,
                },
            )
            .await?
            .ok_or_else(|| AppError::Conflict("the new fact was not retimed".into()))?;
            let report = temporal::reconcile_in(tx, kb_id, &[corrected]).await?;
            // Tidy may rewrite neighbours as well. Include their parents so a
            // later change to them makes undo refuse rather than overwrite it.
            let parents: Vec<Uuid> = sqlx::query_scalar(
                "SELECT supersedes FROM facts WHERE id = ANY($1) AND supersedes IS NOT NULL",
            )
            .bind(&report.corrected)
            .fetch_all(&mut **tx)
            .await?;
            roots.extend(parents);
            Inverse::Rewrite {
                original: item.new.id,
                corrected,
            }
        }
        _ => {
            return Err(AppError::invalid(
                "agent_answer",
                "Unknown conflict action.",
            ))
        }
    };
    roots.sort_unstable();
    roots.dedup();
    let after = footprint(tx, kb_id, &roots).await?;
    // A rejection may answer several conflicts. Remember each changed question:
    // after a revert, a sibling must not reject the same fact again. Unchanged
    // neighbouring conflicts can still be reviewed on their own merits.
    let affected_pairs = before_conflicts
        .iter()
        .filter(|c| !after.conflicts.contains(c))
        .map(|c| c.pair_key.clone())
        .collect();
    Ok(Undo {
        inverse,
        roots,
        before_conflicts,
        affected_pairs,
        after,
    })
}

async fn gate(
    conn: &mut PgConnection,
    kb_id: Uuid,
    item: &Case,
    action: &str,
    params: &Parameters,
) -> AppResult<Option<String>> {
    if action == "retime_new"
        && params.date == item.new.valid_from
        && params.precision == item.new.valid_from_precision
        && item.new.valid_from_grade.as_deref() != Some("C")
    {
        return Ok(Some("the proposed start is already recorded".into()));
    }
    if action == "close_old"
        && params
            .date
            .zip(item.new.valid_from)
            .is_some_and(|(end, start)| end > start)
    {
        return Ok(Some(
            "closing the old value after the new start would leave the overlap".into(),
        ));
    }
    if matches!(action, "close_old" | "retime_new") && !date_supported(params, item) {
        return Ok(Some(
            "the date or its precision is not supported by the new fact's evidence".into(),
        ));
    }
    if item.old_evidence.is_empty() || item.new_evidence.is_empty() {
        return Ok(Some("both sides need current source evidence".into()));
    }
    if item.reason == "described_evidence" || item.old.described || item.new.described {
        return Ok(Some(
            "a visually described statement needs a person's review".into(),
        ));
    }
    if action != "keep_both" {
        let changed = if action == "close_old" {
            &item.old
        } else {
            &item.new
        };
        if changed.implied || changed.derived_by_rule.is_some() {
            return Ok(Some(
                "the fact is computed by a rule; review its source rule".into(),
            ));
        }
        // The prompt has bounded excerpts; a ninth source must not bypass the
        // protection for a person's memory. These flags inspect all sources.
        if item.old.memory || item.new.memory {
            return Ok(Some(
                "a person's memory statement needs a person's decision".into(),
            ));
        }
        let impact = crate::execution_gate::impact_of_fact_in(conn, kb_id, changed.id).await?;
        if let Some(hold) = crate::execution_gate::hold(&impact) {
            return Ok(Some(hold.explain()));
        }
    }
    Ok(None)
}

pub struct Decision<'a> {
    pub run_id: Uuid,
    pub action: &'a str,
    pub confidence: f32,
    pub why: &'a str,
    pub params: Parameters,
    pub precedents: &'a [Value],
}

#[derive(Debug)]
pub struct Recorded {
    pub id: Uuid,
    pub applied: bool,
}

/// No model call or pooled second connection is allowed while these locks are
/// held. A failed write of the decision rolls its graph mutation back as well.
pub async fn record(
    pool: &PgPool,
    kb_id: Uuid,
    item: &Case,
    d: Decision<'_>,
) -> AppResult<Option<Recorded>> {
    let mut tx = pool.begin().await?;
    if !lock_case(&mut tx, kb_id, item).await? {
        return Ok(None);
    }
    let enabled: bool = sqlx::query_scalar("SELECT governance FROM knowledge_bases WHERE id = $1")
        .bind(kb_id)
        .fetch_one(&mut *tx)
        .await?;
    if !enabled {
        return Ok(None);
    }
    let seen: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM agent_decisions
        WHERE kb_id = $1 AND target_kind = 'conflict' AND status <> 'superseded'
          AND (target_id = $2 OR detail->>'pair_key' = $3
            OR (status = 'reverted' AND detail->'undo'->'affected_pairs' ? $3)))",
    )
    .bind(kb_id)
    .bind(item.id)
    .bind(&item.pair_key)
    .fetch_one(&mut *tx)
    .await?;
    if seen {
        return Ok(None);
    }
    let valid = d.confidence.is_finite()
        && (0.0..=1.0).contains(&d.confidence)
        && matches!(
            d.action,
            "close_old" | "retime_new" | "keep_both" | "reject_new" | "unsure"
        );
    let action = if valid { d.action } else { "unsure" };
    let confidence = if valid { d.confidence } else { 0.0 };
    let effective = d.params.effective(item, action);
    let mut held = effective.as_ref().err().map(ToString::to_string);
    let params = effective.unwrap_or(d.params);
    if held.is_none() && action != "unsure" && confidence >= crate::governance::AUTO_CONF {
        held = gate(&mut tx, kb_id, item, action, &params).await?;
    }
    let undo = if held.is_none() && action != "unsure" && confidence >= crate::governance::AUTO_CONF
    {
        use sqlx::Acquire;
        let mut save = tx.begin().await?;
        match perform(&mut save, kb_id, item, action, &params).await {
            Ok(undo) => {
                save.commit().await?;
                Some(undo)
            }
            Err(
                e @ (AppError::NotFound
                | AppError::Conflict(_)
                | AppError::Invalid { .. }
                | AppError::Validation(_)),
            ) => {
                save.rollback().await?;
                held = Some(e.to_string());
                None
            }
            Err(e) => return Err(e),
        }
    } else {
        None
    };
    let reason = match held {
        Some(h) => format!("held for a person: {h}; {}", d.why),
        None => d.why.to_string(),
    };
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO agent_decisions
        (id, kb_id, run_id, target_kind, target_id, action, confidence, reason, precedents,
         status, summary, detail, calls)
        VALUES ($1,$2,$3,'conflict',$4,$5,$6,$7,$8,$9,$10,$11,0)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(d.run_id)
    .bind(item.id)
    .bind(action)
    .bind(confidence)
    .bind(&reason)
    .bind(json!(d.precedents))
    .bind(if undo.is_some() {
        "applied"
    } else {
        "proposed"
    })
    .bind(item.summary())
    .bind(json!({"pair_key":item.pair_key,"snapshot":item,"params":params,"undo":undo}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if undo.is_some() {
        let mut detail = audit_detail(item, Some(id), action, Some(&reason));
        detail["date"] = json!(params.date);
        detail["date_precision"] = json!(params.precision);
        let _ = crate::audit::record_opt(
            pool,
            Some(kb_id),
            None,
            &format!("conflict.{action}"),
            "conflict",
            Some(item.id),
            detail,
        )
        .await;
    }
    Ok(Some(Recorded {
        id,
        applied: undo.is_some(),
    }))
}

fn audit_detail(item: &Case, decision: Option<Uuid>, action: &str, why: Option<&str>) -> Value {
    json!({"old_subject":item.old.subject,"old_object":item.old.object,
        "new_subject":item.new.subject,"new_object":item.new.object,"predicate":item.old.predicate,
        "agent_decision":decision,"agent_action":decision.map(|_| action),"summary":item.summary(),"why":why})
}

pub async fn reviewed_today(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM agent_decisions WHERE kb_id = $1
        AND target_kind = 'conflict' AND created_at >= date_trunc('day', now())",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?)
}

/// Only withdrawn/repointed proposals expire. Applied and reverted decisions
/// keep their lineage identity, including after undo gives a conflict a new ID.
pub async fn expire(pool: &PgPool, kb_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE agent_decisions d SET status = 'superseded', decided_at = now()
        WHERE d.kb_id = $1 AND d.target_kind = 'conflict' AND d.status = 'proposed'
          AND NOT EXISTS (SELECT 1 FROM fact_conflicts c WHERE c.id = d.target_id
            AND c.kb_id = d.kb_id AND c.status = 'open'
            AND c.old_fact_id::text = d.detail->'snapshot'->'old'->>'id'
            AND c.new_fact_id::text = d.detail->'snapshot'->'new'->>'id')",
    )
    .bind(kb_id)
    .execute(pool)
    .await?;
    Ok(())
}

fn changed() -> AppError {
    AppError::CodedConflict {
        code: "agent_inputs_changed",
        message: "The facts changed after this decision. Review their current state.".into(),
    }
}

async fn live_successor(conn: &mut PgConnection, kb_id: Uuid, id: Uuid) -> AppResult<Uuid> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "WITH RECURSIVE family(id) AS (
            SELECT id FROM facts WHERE kb_id = $1 AND id = $2
            UNION SELECT f.id FROM facts f JOIN family p ON f.supersedes = p.id WHERE f.kb_id = $1)
         SELECT f.id FROM facts f JOIN family p ON p.id = f.id WHERE f.invalidated_at IS NULL",
    )
    .bind(kb_id)
    .bind(id)
    .fetch_all(conn)
    .await?;
    match ids.as_slice() {
        [id] => Ok(*id),
        _ => Err(changed()),
    }
}

async fn restore_conflicts(
    tx: &mut Transaction<'_, Postgres>,
    kb_id: Uuid,
    before: &[ConflictState],
) -> AppResult<()> {
    for c in before {
        let old = live_successor(tx, kb_id, c.old_fact_id).await?;
        let new = live_successor(tx, kb_id, c.new_fact_id).await?;
        if old == new {
            return Err(changed());
        }
        // The engine may have carried this row or created its successor while
        // restoring the timeline. Reopen that live pair, not dead fact IDs.
        let pair: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM fact_conflicts WHERE kb_id = $1
               AND ((old_fact_id = $2 AND new_fact_id = $3) OR (old_fact_id = $3 AND new_fact_id = $2))
             ORDER BY (id = $4) DESC, id LIMIT 1 FOR UPDATE")
            .bind(kb_id).bind(old).bind(new).bind(c.id).fetch_optional(&mut **tx).await?;
        if let Some(id) = pair {
            sqlx::query(
                "UPDATE fact_conflicts SET status = 'open', resolution = NULL, resolved_at = NULL
                WHERE kb_id = $1 AND id = $2",
            )
            .bind(kb_id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO fact_conflicts (id,kb_id,old_fact_id,new_fact_id,reason)
                VALUES ($1,$2,$3,$4,$5)",
            )
            .bind(Uuid::now_v7())
            .bind(kb_id)
            .bind(old)
            .bind(new)
            .bind(&c.reason)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

async fn undo(
    pool: &PgPool,
    kb_id: Uuid,
    decision_id: Uuid,
    user_id: Uuid,
    why: Option<&str>,
) -> AppResult<()> {
    let d = crate::governance::get(pool, kb_id, decision_id).await?;
    if d.target_kind != "conflict" || d.status != "applied" {
        return Err(AppError::invalid(
            "agent_answer",
            "Only an applied conflict decision can be reverted.",
        ));
    }
    let undo: Undo = serde_json::from_value(d.detail["undo"].clone()).map_err(|_| changed())?;
    let item: Case = serde_json::from_value(d.detail["snapshot"].clone()).map_err(|_| changed())?;
    let mut tx = pool.begin().await?;
    let timelines = temporal::timelines_of(&mut *tx, kb_id, &undo.roots, None).await?;
    temporal::lock_timelines(&mut tx, kb_id, &timelines).await?;
    let ids: Vec<Uuid> = undo
        .after
        .facts
        .iter()
        .filter_map(|f| f["id"].as_str()?.parse().ok())
        .collect();
    sqlx::query("SELECT id FROM facts WHERE kb_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE")
        .bind(kb_id)
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
    let conflicts: Vec<Uuid> = undo.after.conflicts.iter().map(|c| c.id).collect();
    sqlx::query(
        "SELECT id FROM fact_conflicts WHERE kb_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE",
    )
    .bind(kb_id)
    .bind(conflicts)
    .fetch_all(&mut *tx)
    .await?;
    let status: Option<String> = sqlx::query_scalar(
        "SELECT status FROM agent_decisions
        WHERE kb_id = $1 AND id = $2 AND target_kind = 'conflict' FOR UPDATE",
    )
    .bind(kb_id)
    .bind(decision_id)
    .fetch_optional(&mut *tx)
    .await?;
    if status.as_deref() != Some("applied") {
        return Err(changed());
    }
    if footprint(&mut tx, kb_id, &undo.roots).await? != undo.after {
        return Err(changed());
    }
    match undo.inverse {
        Inverse::Keep => {}
        Inverse::Restore { fact } => temporal::restore_in(&mut tx, kb_id, fact).await?,
        Inverse::Rewrite {
            original,
            corrected,
        } => temporal::undo_rewrite_in(&mut tx, kb_id, original, corrected).await?,
    }
    restore_conflicts(&mut tx, kb_id, &undo.before_conflicts).await?;
    sqlx::query(
        "UPDATE agent_decisions SET status = 'reverted', decided_at = now(), decided_by = $3
        WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(decision_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    enqueue_in(&mut tx, kb_id).await?;
    tx.commit().await?;
    let _ = crate::audit::record(
        pool,
        Some(kb_id),
        user_id,
        "conflict.revert",
        "conflict",
        Some(item.id),
        audit_detail(&item, Some(decision_id), &d.action, why),
    )
    .await;
    Ok(())
}

/// The ordinary conflict card and the Agent queue use this same writer. The
/// answer settles the proposal in the graph transaction, including when a
/// person chooses a different action or date from the suggestion.
pub async fn resolve(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    action: &str,
    params: Parameters,
    user_id: Uuid,
    why: Option<&str>,
) -> AppResult<()> {
    human_apply(pool, kb_id, conflict_id, None, action, params, user_id, why).await
}

pub async fn answer(
    pool: &PgPool,
    kb_id: Uuid,
    decision_id: Uuid,
    action: &str,
    params: Parameters,
    user_id: Uuid,
    why: Option<&str>,
) -> AppResult<()> {
    if action == "revert" {
        return undo(pool, kb_id, decision_id, user_id, why).await;
    }
    let d = crate::governance::get(pool, kb_id, decision_id).await?;
    if d.target_kind == "conflict" && d.status == "superseded" {
        return Err(changed());
    }
    if d.target_kind != "conflict" || d.status != "proposed" {
        return Err(AppError::invalid(
            "agent_answer",
            "This conflict decision is not awaiting an answer.",
        ));
    }
    // An explicit date is an edit. With no edit, accepting the proposed action
    // uses its reviewed parameters; overriding it uses the new action's inputs.
    let params = if params.date.is_none() && action == d.action {
        serde_json::from_value(d.detail["params"].clone()).map_err(|_| changed())?
    } else {
        params
    };
    human_apply(
        pool,
        kb_id,
        d.target_id,
        Some(decision_id),
        action,
        params,
        user_id,
        why,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn human_apply(
    pool: &PgPool,
    kb_id: Uuid,
    conflict_id: Uuid,
    expected: Option<Uuid>,
    action: &str,
    params: Parameters,
    user_id: Uuid,
    why: Option<&str>,
) -> AppResult<()> {
    if !matches!(
        action,
        "close_old" | "retime_new" | "keep_both" | "reject_new"
    ) {
        return Err(AppError::invalid(
            "agent_answer",
            "Unknown action for a conflict.",
        ));
    }
    let item = load(pool, kb_id, conflict_id).await?.ok_or_else(|| {
        if expected.is_some() {
            changed()
        } else {
            AppError::NotFound
        }
    })?;
    let mut tx = pool.begin().await?;
    if !lock_case(&mut tx, kb_id, &item).await? {
        return Err(changed());
    }
    let proposal: Option<(Uuid, String, Value)> = sqlx::query_as(
        "SELECT id, action, detail FROM agent_decisions WHERE kb_id = $1
         AND target_kind = 'conflict' AND target_id = $2 AND status = 'proposed' FOR UPDATE",
    )
    .bind(kb_id)
    .bind(conflict_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(expected) = expected {
        if proposal.as_ref().map(|d| d.0) != Some(expected) {
            return Err(changed());
        }
        let reviewed: Case =
            serde_json::from_value(proposal.as_ref().unwrap().2["snapshot"].clone())
                .map_err(|_| changed())?;
        if reviewed != item {
            sqlx::query(
                "UPDATE agent_decisions SET status = 'superseded', decided_at = now()
                WHERE kb_id = $1 AND id = $2 AND status = 'proposed'",
            )
            .bind(kb_id)
            .bind(expected)
            .execute(&mut *tx)
            .await?;
            enqueue_in(&mut tx, kb_id).await?;
            tx.commit().await?;
            return Err(changed());
        }
    }
    let effective = params.effective(&item, action)?;
    perform(&mut tx, kb_id, &item, action, &effective).await?;
    if let Some((id, suggested, detail)) = &proposal {
        let old_params: Option<Parameters> = serde_json::from_value(detail["params"].clone()).ok();
        let old_snapshot: Option<Case> = serde_json::from_value(detail["snapshot"].clone()).ok();
        let accepted = suggested == action
            && old_params.as_ref() == Some(&effective)
            && old_snapshot.as_ref() == Some(&item);
        sqlx::query(
            "UPDATE agent_decisions SET status = $3, decided_at = now(), decided_by = $4
            WHERE kb_id = $1 AND id = $2",
        )
        .bind(kb_id)
        .bind(id)
        .bind(if accepted { "accepted" } else { "overridden" })
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    }
    // Reconsider sibling suggestions with the person's decision as precedent.
    sqlx::query(
        "UPDATE agent_decisions d SET status = 'superseded', decided_at = now()
        FROM fact_conflicts c WHERE d.kb_id = $1 AND d.target_kind = 'conflict'
          AND d.target_id = c.id AND d.status = 'proposed' AND c.kb_id = $1 AND c.id <> $2
          AND (c.old_fact_id = ANY($3) OR c.new_fact_id = ANY($3))",
    )
    .bind(kb_id)
    .bind(conflict_id)
    .bind([item.old.id, item.new.id])
    .execute(&mut *tx)
    .await?;
    enqueue_in(&mut tx, kb_id).await?;
    tx.commit().await?;
    let _ = crate::audit::record(
        pool,
        Some(kb_id),
        user_id,
        &format!("conflict.{action}"),
        "conflict",
        Some(conflict_id),
        {
            let suggested = proposal.as_ref().map(|p| p.1.as_str()).unwrap_or(action);
            let mut detail = audit_detail(&item, proposal.as_ref().map(|p| p.0), suggested, why);
            detail["date"] = json!(effective.date);
            detail["date_precision"] = json!(effective.precision);
            // Preserve the ordinary conflict endpoint's audit fields as well.
            detail["close_at"] = json!(params.date);
            detail["close_at_precision"] = json!(params.date.and(params.precision.as_deref()));
            detail
        },
    )
    .await;
    Ok(())
}

pub(crate) async fn enqueue_in(tx: &mut Transaction<'_, Postgres>, kb_id: Uuid) -> AppResult<()> {
    let enabled: bool = sqlx::query_scalar("SELECT governance FROM knowledge_bases WHERE id = $1")
        .bind(kb_id)
        .fetch_one(&mut **tx)
        .await?;
    if enabled {
        crate::jobs::enqueue_unless_queued_tx(tx, "govern", json!({"kb_id":kb_id})).await?;
    }
    Ok(())
}
