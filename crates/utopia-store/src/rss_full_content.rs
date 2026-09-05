//! Durable discovery and hydration state for opt-in full-content RSS sources.
//!
//! The ledger is deliberately separate from `documents`: a feed item can be
//! discovered before it has acceptable content, and it must remain hydratable
//! after it leaves the publisher's sliding feed window.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

pub const HYDRATION_JOB_KIND: &str = "hydrate_rss_entry";

/// Re-admit failed hydration jobs through the same source lock, generation,
/// purge fence and capacity limit as first admission. Generic jobs exclude these.
pub async fn requeue_failed(pool: &PgPool, scope: crate::jobs::RequeueScope<'_>) -> AppResult<u64> {
    let mut count = 0;
    if scope.kind.is_some_and(|kind| kind != HYDRATION_JOB_KIND) {
        return Ok(count);
    }
    // RSS retries share the same source lock and capacity as first admission.
    // Never reinstate a superseded job simply because it still exists in history.
    let sources: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT e.source_id FROM rss_full_content_entries e
         JOIN jobs j ON j.id = e.current_job_id
         JOIN sources s ON s.id = e.source_id
         WHERE j.status = 'failed' AND j.kind = 'hydrate_rss_entry'
           AND ($1::uuid IS NULL OR s.kb_id = $1)
           AND ($2::timestamptz IS NULL OR j.updated_at >= $2)
         ORDER BY e.source_id",
    )
    .bind(scope.kb_id)
    .bind(scope.failed_since)
    .fetch_all(pool)
    .await?;
    for source_id in sources {
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT id FROM sources WHERE id = $1 FOR UPDATE")
            .bind(source_id)
            .execute(&mut *tx)
            .await?;
        let generation: Option<i32> = sqlx::query_scalar(
            "SELECT rss_generation FROM sources WHERE id=$1
             AND rss_baselined_at IS NOT NULL AND config->>'content_mode'='full_new_items'",
        )
        .bind(source_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(generation) = generation else {
            continue;
        };
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM rss_full_content_entries e
             JOIN jobs j ON j.id = e.current_job_id
             WHERE e.source_id = $1 AND e.activation_generation = $2
               AND j.status IN ('queued', 'running')",
        )
        .bind(source_id)
        .bind(generation)
        .fetch_one(&mut *tx)
        .await?;
        let capacity = (25 - live).max(0);
        let jobs: Vec<i64> = sqlx::query_scalar(
            "SELECT j.id FROM rss_full_content_entries e
             JOIN jobs j ON j.id = e.current_job_id
             WHERE e.source_id = $1 AND e.activation_generation = $2
               AND j.kind = 'hydrate_rss_entry' AND j.status = 'failed'
               AND ($3::timestamptz IS NULL OR j.updated_at >= $3)
               AND j.payload ? 'observed_at'
               AND NOT EXISTS (
                 SELECT 1 FROM document_deletions dd JOIN documents d ON d.id = dd.document_id
                 WHERE d.source_id = e.source_id AND dd.external_key = e.external_key
                   AND GREATEST(dd.created_at, d.purged_at) >= (j.payload->>'observed_at')::timestamptz)
             ORDER BY j.id LIMIT $4 FOR UPDATE OF e, j",
        ).bind(source_id).bind(generation).bind(scope.failed_since).bind(capacity)
            .fetch_all(&mut *tx).await?;
        count += sqlx::query(
            "UPDATE jobs SET status = 'queued', attempts = 0, last_error = NULL,
             locked_at = NULL, run_at = now(), updated_at = now()
             WHERE id = ANY($1) AND status = 'failed'",
        )
        .bind(&jobs)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
    }
    Ok(count)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Activation {
    pub source_id: Uuid,
    pub activation_generation: i32,
    pub activation_state: String,
    pub activation_at: Option<DateTime<Utc>>,
    pub baseline_count: i32,
    pub last_discovery_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Entry {
    pub id: Uuid,
    pub source_id: Uuid,
    pub activation_generation: i32,
    pub external_key: String,
    pub title: String,
    pub article_url: Option<String>,
    pub summary: String,
    pub embedded_html: Option<String>,
    pub doc_time: Option<DateTime<Utc>>,
    pub state: String,
    pub hydration_job_id: Option<i64>,
    pub attempt_count: i32,
    pub document_id: Option<Uuid>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub first_seen_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Values obtained from one bounded feed response. The database remains the
/// authority for lifecycle state; `has_usable_source` only controls the first
/// state for a newly discovered row.
#[derive(Debug, Clone)]
pub struct NewEntry {
    pub external_key: String,
    pub title: String,
    pub article_url: Option<String>,
    pub summary: String,
    pub embedded_html: Option<String>,
    pub doc_time: Option<DateTime<Utc>>,
    pub has_usable_source: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoveryStats {
    pub discovered: usize,
    pub terminal: usize,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Counts {
    pub activation_state: String,
    pub activation_generation: i32,
    pub baseline_count: i32,
    pub last_discovery_at: Option<DateTime<Utc>>,
    pub pending_count: i64,
    pub queued_count: i64,
    pub hydrating_count: i64,
    pub retrying_count: i64,
    pub complete_count: i64,
    pub terminal_count: i64,
}

const ACTIVATION_STATE: &str = "CASE WHEN config->>'content_mode' IS DISTINCT FROM 'full_new_items' THEN 'disabled' WHEN rss_baselined_at IS NULL THEN 'pending' ELSE 'active' END";

fn activation_select() -> String {
    format!(
        "SELECT s.id AS source_id, rss_generation AS activation_generation,
        {ACTIVATION_STATE} AS activation_state, rss_baselined_at AS activation_at,
        (SELECT count(*)::int FROM rss_full_content_entries e WHERE e.source_id=s.id
           AND e.activation_generation=s.rss_generation AND e.entry_kind='baseline') AS baseline_count,
        (SELECT max(observed_at) FROM rss_full_content_entries e WHERE e.source_id=s.id
           AND e.activation_generation=s.rss_generation) AS last_discovery_at
        FROM sources s WHERE s.id=$1 AND s.kind='rss'"
    )
}

pub async fn initialize_source(
    tx: &mut Transaction<'_, Postgres>,
    source_id: Uuid,
) -> AppResult<()> {
    sqlx::query("UPDATE sources SET rss_generation = 1 WHERE id=$1 AND rss_generation=0")
        .bind(source_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn enable_source(tx: &mut Transaction<'_, Postgres>, source_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE sources SET rss_generation=rss_generation+1, rss_baselined_at=NULL WHERE id=$1",
    )
    .bind(source_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn get_activation(pool: &PgPool, source_id: Uuid) -> AppResult<Option<Activation>> {
    Ok(sqlx::query_as(&activation_select())
        .bind(source_id)
        .fetch_optional(pool)
        .await?)
}

/// Bind network evidence to the configuration/generation before starting I/O.
/// Database time avoids comparing clocks from different machines. Capturing
/// before fetch is conservative: a deletion during the fetch requires a new fetch.
pub async fn begin_feed_observation(
    pool: &PgPool,
    source_id: Uuid,
    expected_config: &serde_json::Value,
) -> AppResult<(Activation, DateTime<Utc>)> {
    let mut tx = pool.begin().await?;
    let config: serde_json::Value =
        sqlx::query_scalar("SELECT config FROM sources WHERE id = $1 FOR UPDATE")
            .bind(source_id)
            .fetch_one(&mut *tx)
            .await?;
    if config.get("feed_url") != expected_config.get("feed_url")
        || config.get("content_mode") != expected_config.get("content_mode")
    {
        return Err(AppError::Conflict(
            "RSS configuration changed before fetch".into(),
        ));
    }
    let activation: Activation = sqlx::query_as(&activation_select())
        .bind(source_id)
        .fetch_one(&mut *tx)
        .await?;
    let observed_at = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((activation, observed_at))
}

// One projection for row diagnostics, counts and source summaries. No raw
// job error is exported; queue state and document state remain authoritative.
pub(crate) const ENTRY_SELECT: &str = r#"
SELECT e.id, e.source_id, e.activation_generation, e.external_key, e.title,
 e.article_url, e.summary, e.embedded_html, e.doc_time,
 CASE
 WHEN s.rss_generation <> e.activation_generation OR s.config->>'content_mode' IS DISTINCT FROM 'full_new_items' THEN 'superseded'
 WHEN e.entry_kind='baseline' THEN 'baseline'
 WHEN d.id IS NOT NULL AND d.deleted_at IS NULL THEN 'complete'
 WHEN EXISTS (SELECT 1 FROM document_deletions dd JOIN documents old ON old.id=dd.document_id
   WHERE old.source_id=e.source_id AND dd.external_key=e.external_key
   AND GREATEST(dd.created_at,old.purged_at)>=COALESCE((j.payload->>'observed_at')::timestamptz,e.observed_at)) THEN 'deleted'
 WHEN e.entry_kind='no_source' THEN 'terminal'
 WHEN j.status='running' THEN 'hydrating'
 WHEN j.status='queued' AND j.attempts>0 THEN 'retry_wait'
 WHEN j.status='queued' THEN 'queued'
 WHEN j.status='failed' THEN 'terminal'
 WHEN j.status='done' THEN 'superseded'
 ELSE 'pending' END AS state,
 e.current_job_id AS hydration_job_id, COALESCE(j.attempts,0) AS attempt_count,
 d.id AS document_id,
 CASE WHEN d.id IS NOT NULL AND d.deleted_at IS NULL THEN NULL
      WHEN e.entry_kind='no_source' THEN 'no_usable_content_source'
      WHEN j.status='failed' THEN 'acquisition_failed' END AS error_code,
 CASE WHEN d.id IS NOT NULL AND d.deleted_at IS NULL THEN NULL
      WHEN e.entry_kind='no_source' THEN 'No usable content source was observed'
      WHEN j.status='failed' THEN 'Full-content acquisition failed; retry is available' END AS error_detail,
 e.first_seen_at, GREATEST(e.updated_at,j.updated_at,d.updated_at) AS updated_at,
 CASE WHEN j.status='done' AND d.deleted_at IS NULL THEN j.updated_at END AS completed_at
FROM rss_full_content_entries e JOIN sources s ON s.id=e.source_id
LEFT JOIN jobs j ON j.id=e.current_job_id
LEFT JOIN documents d ON d.source_id=e.source_id AND d.external_key=e.external_key AND d.purged_at IS NULL
"#;

async fn lock_activation(
    tx: &mut Transaction<'_, Postgres>,
    source_id: Uuid,
) -> AppResult<Option<(String, i32)>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {ACTIVATION_STATE}, rss_generation FROM sources WHERE id=$1 FOR UPDATE"
    ))
    .bind(source_id)
    .fetch_optional(&mut **tx)
    .await?)
}

pub async fn record_baseline(
    pool: &PgPool,
    source_id: Uuid,
    generation: i32,
    entries: &[NewEntry],
) -> AppResult<usize> {
    let mut tx = pool.begin().await?;
    if lock_activation(&mut tx, source_id).await? != Some(("pending".into(), generation)) {
        return Err(AppError::Conflict(
            "RSS activation changed before baseline".into(),
        ));
    }
    let mut inserted = 0;
    for e in entries {
        inserted += sqlx::query("INSERT INTO rss_full_content_entries(id,source_id,activation_generation,external_key,title,entry_kind)
          VALUES($1,$2,$3,$4,$5,'baseline') ON CONFLICT DO NOTHING")
            .bind(Uuid::now_v7()).bind(source_id).bind(generation).bind(&e.external_key).bind(&e.title)
            .execute(&mut *tx).await?.rows_affected() as usize;
    }
    sqlx::query("UPDATE sources SET rss_baselined_at=clock_timestamp() WHERE id=$1")
        .bind(source_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(inserted)
}

pub async fn discover(
    pool: &PgPool,
    source_id: Uuid,
    generation: i32,
    entries: &[NewEntry],
) -> AppResult<DiscoveryStats> {
    let observed_at = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await?;
    discover_observed(pool, source_id, generation, entries, observed_at).await
}

pub async fn discover_observed(
    pool: &PgPool,
    source_id: Uuid,
    generation: i32,
    entries: &[NewEntry],
    observed_at: DateTime<Utc>,
) -> AppResult<DiscoveryStats> {
    let mut tx = pool.begin().await?;
    if lock_activation(&mut tx, source_id).await? != Some(("active".into(), generation)) {
        return Err(AppError::Conflict(
            "RSS activation changed before discovery".into(),
        ));
    }
    let mut stats = DiscoveryStats::default();
    for e in entries {
        let kind = if e.has_usable_source {
            "candidate"
        } else {
            "no_source"
        };
        let n=sqlx::query("INSERT INTO rss_full_content_entries
            (id,source_id,activation_generation,external_key,title,article_url,summary,embedded_html,doc_time,entry_kind,observed_at)
            VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT DO NOTHING")
            .bind(Uuid::now_v7()).bind(source_id).bind(generation).bind(&e.external_key).bind(&e.title)
            .bind(&e.article_url).bind(&e.summary).bind(&e.embedded_html).bind(e.doc_time).bind(kind).bind(observed_at)
            .execute(&mut *tx).await?.rows_affected();
        if n > 0 {
            stats.discovered += 1;
            stats.terminal += usize::from(!e.has_usable_source);
            continue;
        }
        // Only actual later evidence can replace a deletion-invalidated claim.
        // The running job keeps its immutable pre-fetch observation timestamp.
        // A summary-only refresh is not fresh evidence for the retained body.
        // Keep that body and its old timestamp together; otherwise a poorer
        // post-purge response could authorize replay of pre-purge content.
        sqlx::query("UPDATE rss_full_content_entries e SET current_job_id=NULL
            WHERE source_id=$1 AND activation_generation=$2 AND external_key=$3
            AND entry_kind<>'baseline' AND observed_at<=$4 AND $5
            AND EXISTS(SELECT 1 FROM document_deletions dd JOIN documents d ON d.id=dd.document_id
              WHERE d.source_id=e.source_id AND dd.external_key=e.external_key
              AND GREATEST(dd.created_at,d.purged_at)>=COALESCE(
                (SELECT (payload->>'observed_at')::timestamptz FROM jobs WHERE id=e.current_job_id),e.observed_at))")
            .bind(source_id).bind(generation).bind(&e.external_key).bind(observed_at).bind(e.has_usable_source)
            .execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE rss_full_content_entries SET title=$4, article_url=$5, summary=$6,
            embedded_html=$7, doc_time=$8, observed_at=$9, updated_at=clock_timestamp(),
            entry_kind=CASE WHEN $10 THEN 'candidate' ELSE entry_kind END
            WHERE source_id=$1 AND activation_generation=$2 AND external_key=$3
              AND entry_kind<>'baseline' AND observed_at<=$9
              AND ($10 OR entry_kind='no_source')
              AND NOT EXISTS(SELECT 1 FROM jobs WHERE id=current_job_id AND status='done')",
        )
        .bind(source_id)
        .bind(generation)
        .bind(&e.external_key)
        .bind(&e.title)
        .bind(&e.article_url)
        .bind(&e.summary)
        .bind(&e.embedded_html)
        .bind(e.doc_time)
        .bind(observed_at)
        .bind(e.has_usable_source)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(stats)
}

pub async fn claim_pending_and_enqueue(
    pool: &PgPool,
    source_id: Uuid,
    generation: i32,
    max_inflight: i64,
    max_attempts: i32,
) -> AppResult<usize> {
    let mut tx = pool.begin().await?;
    if lock_activation(&mut tx, source_id).await? != Some(("active".into(), generation)) {
        return Ok(0);
    }
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM rss_full_content_entries e JOIN jobs j ON j.id=e.current_job_id
        WHERE source_id=$1 AND activation_generation=$2 AND j.status IN ('queued','running')",
    )
    .bind(source_id)
    .bind(generation)
    .fetch_one(&mut *tx)
    .await?;
    let capacity = (max_inflight.clamp(0, 25) - live).max(0);
    let entries:Vec<(Uuid,DateTime<Utc>)>=sqlx::query_as("SELECT id,observed_at FROM rss_full_content_entries e
        WHERE source_id=$1 AND activation_generation=$2 AND entry_kind='candidate' AND current_job_id IS NULL
        AND NOT EXISTS(SELECT 1 FROM document_deletions dd JOIN documents d ON d.id=dd.document_id
           WHERE d.source_id=e.source_id AND dd.external_key=e.external_key
           AND GREATEST(dd.created_at,d.purged_at)>=e.observed_at)
        ORDER BY first_seen_at,id LIMIT $3 FOR UPDATE")
        .bind(source_id).bind(generation).bind(capacity).fetch_all(&mut *tx).await?;
    for (id, observed_at) in &entries {
        let job = crate::jobs::enqueue_with_max_attempts_tx(
            &mut tx,
            HYDRATION_JOB_KIND,
            serde_json::json!({"source_id":source_id,"rss_entry_id":id,"observed_at":observed_at}),
            max_attempts,
        )
        .await?;
        sqlx::query("UPDATE rss_full_content_entries SET current_job_id=$2,updated_at=clock_timestamp() WHERE id=$1")
            .bind(id).bind(job).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(entries.len())
}

pub async fn get_entry(pool: &PgPool, entry_id: Uuid) -> AppResult<Option<Entry>> {
    Ok(sqlx::query_as(&format!("{ENTRY_SELECT} WHERE e.id=$1"))
        .bind(entry_id)
        .fetch_optional(pool)
        .await?)
}

// The generic queue owns attempts and execution transitions. This is only a
// claim check, not a second persisted state machine.
pub async fn current_attempt(pool: &PgPool, entry_id: Uuid, job_id: i64) -> AppResult<Option<i32>> {
    Ok(sqlx::query_scalar(
        "SELECT j.attempts FROM rss_full_content_entries e JOIN jobs j ON j.id=e.current_job_id
        JOIN sources s ON s.id=e.source_id WHERE e.id=$1 AND j.id=$2 AND j.status='running'
        AND e.entry_kind='candidate' AND s.rss_generation=e.activation_generation
        AND s.rss_baselined_at IS NOT NULL AND s.config->>'content_mode'='full_new_items'",
    )
    .bind(entry_id)
    .bind(job_id)
    .fetch_optional(pool)
    .await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn complete_hydration(
    pool: &PgPool,
    entry_id: Uuid,
    job_id: i64,
    source_id: Uuid,
    generation: i32,
    kb_id: Uuid,
    external_key: &str,
    filename: &str,
    mime: &str,
    size_bytes: i64,
    content_sha256: &str,
    doc_time: Option<DateTime<Utc>>,
    _content_source: &str,
    _final_url: Option<&str>,
) -> AppResult<Option<Uuid>> {
    let mut tx = pool.begin().await?;
    if lock_activation(&mut tx, source_id).await? != Some(("active".into(), generation)) {
        return Ok(None);
    }
    // Match source, generation, identity, KB, current running job and immutable
    // deletion precondition inside the same source-serialized publication txn.
    let allowed: Option<Uuid> = sqlx::query_scalar(
        "SELECT e.id FROM rss_full_content_entries e
        JOIN sources s ON s.id=e.source_id JOIN jobs j ON j.id=e.current_job_id
        WHERE e.id=$1 AND j.id=$2 AND e.source_id=$3 AND e.activation_generation=$4
          AND e.external_key=$5 AND s.kb_id=$6 AND e.entry_kind='candidate' AND j.status='running'
          AND j.payload ? 'observed_at'
          AND NOT EXISTS(SELECT 1 FROM document_deletions dd JOIN documents d ON d.id=dd.document_id
             WHERE d.source_id=e.source_id AND dd.external_key=e.external_key
             AND GREATEST(dd.created_at,d.purged_at)>=(j.payload->>'observed_at')::timestamptz)
        FOR UPDATE OF e,j",
    )
    .bind(entry_id)
    .bind(job_id)
    .bind(source_id)
    .bind(generation)
    .bind(external_key)
    .bind(kb_id)
    .fetch_optional(&mut *tx)
    .await?;
    if allowed.is_none() {
        return Ok(None);
    }
    let document = crate::documents::upsert_source_document_tx(
        &mut tx,
        kb_id,
        source_id,
        external_key,
        filename,
        mime,
        size_bytes,
        content_sha256,
        doc_time,
    )
    .await?;
    sqlx::query(
        "UPDATE jobs SET status='done', last_error=NULL, updated_at=clock_timestamp() WHERE id=$1",
    )
    .bind(job_id)
    .execute(&mut *tx)
    .await?;
    // Accepted content belongs to documents/blob storage, not the observation.
    sqlx::query("UPDATE rss_full_content_entries SET summary='',embedded_html=NULL,updated_at=clock_timestamp() WHERE id=$1")
        .bind(entry_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(document.id))
}

pub async fn list_current_entries(
    pool: &PgPool,
    source_id: Uuid,
    limit: i64,
) -> AppResult<Vec<Entry>> {
    Ok(sqlx::query_as(&format!("{ENTRY_SELECT} WHERE e.source_id=$1 AND e.activation_generation=s.rss_generation ORDER BY e.updated_at DESC,e.id DESC LIMIT $2"))
        .bind(source_id).bind(limit.clamp(1,100)).fetch_all(pool).await?)
}

pub async fn counts(pool: &PgPool, source_id: Uuid) -> AppResult<Option<Counts>> {
    let Some(a) = get_activation(pool, source_id).await? else {
        return Ok(None);
    };
    let (pending_count,queued_count,hydrating_count,retrying_count,complete_count,terminal_count):(i64,i64,i64,i64,i64,i64)=sqlx::query_as(
        &format!("WITH projected AS ({ENTRY_SELECT}) SELECT count(*) FILTER(WHERE state='pending'), count(*) FILTER(WHERE state='queued'),
         count(*) FILTER(WHERE state='hydrating'), count(*) FILTER(WHERE state='retry_wait'), count(*) FILTER(WHERE state='complete'),
         count(*) FILTER(WHERE state IN ('terminal','deleted','superseded')) FROM projected WHERE source_id=$1 AND activation_generation=$2"))
        .bind(source_id).bind(a.activation_generation).fetch_one(pool).await?;
    Ok(Some(Counts {
        activation_state: a.activation_state,
        activation_generation: a.activation_generation,
        baseline_count: a.baseline_count,
        last_discovery_at: a.last_discovery_at,
        pending_count,
        queued_count,
        hydrating_count,
        retrying_count,
        complete_count,
        terminal_count,
    }))
}
