//! An old version keeps no vector.
//!
//! `replace_chunks` supersedes a chunk the new version of its document no longer
//! carries and keeps the row whole: the text for version replay, the vector for an
//! as-of search ([0019](../../../docs/decisions/0019-the-second-clock-can-be-rewound.md)
//! open question ②), and the row itself because `fact_evidence`, `pending_facts`,
//! `time_mentions` and `statement_qualifiers` cascade on it. A `statements` source pushes
//! everything on every pass, so a base of mirrored statements grows a superseded chunk, vector included, for every
//! paragraph that changed, forever. The vector is the larger part of the row by far.
//!
//! The sweep runs once a day per base. A chunk superseded longer ago than `older_than`
//! loses its vector. It loses its row too when nothing refers to it and its document
//! is not deleted: evidence on a superseded chunk is what tells a fact its source moved on
//! (the close time is the chunk's `superseded_at`), and the chunks of a deleted document
//! come back when the deletion is reverted.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

use crate::vector_index::{self, Target};

pub const JOB_KIND: &str = "prune_superseded";
pub const DEFAULT_AGE_DAYS: i64 = 7;
const BATCH: i64 = 10_000;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub vectors_cleared: u64,
    pub chunks_deleted: u64,
}

/// Every base, for the daily tick: a base with nothing old enough costs two index probes.
pub async fn bases(pool: &PgPool) -> AppResult<Vec<Uuid>> {
    let ids: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM knowledge_bases")
        .fetch_all(pool)
        .await?;
    Ok(ids.into_iter().map(|(id,)| id).collect())
}

pub async fn prune_superseded(
    pool: &PgPool,
    kb_id: Uuid,
    older_than: Duration,
) -> AppResult<Pruned> {
    let cutoff = Utc::now() - older_than;
    let pruned = Pruned {
        vectors_cleared: clear_vectors(pool, kb_id, cutoff).await?,
        chunks_deleted: delete_unreferenced(pool, kb_id, cutoff).await?,
    };
    if pruned != Pruned::default() {
        request_index(pool, kb_id).await?;
    }
    Ok(pruned)
}

async fn clear_vectors(pool: &PgPool, kb_id: Uuid, cutoff: DateTime<Utc>) -> AppResult<u64> {
    let mut total = 0;
    loop {
        let cleared = sqlx::query(
            "UPDATE chunks SET embedding = NULL
              WHERE id IN (SELECT id FROM chunks
                            WHERE kb_id = $1 AND superseded_at < $2 AND embedding IS NOT NULL
                            LIMIT $3)",
        )
        .bind(kb_id)
        .bind(cutoff)
        .bind(BATCH)
        .execute(pool)
        .await?
        .rows_affected();
        total += cleared;
        if cleared == 0 {
            return Ok(total);
        }
    }
}

async fn delete_unreferenced(pool: &PgPool, kb_id: Uuid, cutoff: DateTime<Utc>) -> AppResult<u64> {
    let mut total = 0;
    loop {
        let deleted = sqlx::query(
            "DELETE FROM chunks
              WHERE id IN (SELECT c.id FROM chunks c
                             JOIN documents d ON d.id = c.document_id
                            WHERE c.kb_id = $1 AND c.superseded_at < $2 AND d.deleted_at IS NULL
                              AND NOT EXISTS (SELECT 1 FROM fact_evidence e WHERE e.chunk_id = c.id)
                              AND NOT EXISTS (SELECT 1 FROM pending_facts p WHERE p.chunk_id = c.id)
                              AND NOT EXISTS (SELECT 1 FROM time_mentions m WHERE m.chunk_id = c.id)
                              AND NOT EXISTS (SELECT 1 FROM statement_qualifiers q WHERE q.chunk_id = c.id)
                            LIMIT $3)",
        )
        .bind(kb_id)
        .bind(cutoff)
        .bind(BATCH)
        .execute(pool)
        .await?
        .rows_affected();
        total += deleted;
        if deleted == 0 {
            return Ok(total);
        }
    }
}

/// The HNSW index drops the pruned entries on its own; this only makes sure the base's live
/// dimension has an index at all, the way every write of a vector does.
async fn request_index(pool: &PgPool, kb_id: Uuid) -> AppResult<()> {
    let dims: Option<(i32,)> = sqlx::query_as(
        "SELECT vector_dims(embedding) FROM chunks
          WHERE kb_id = $1 AND superseded_at IS NULL AND embedding IS NOT NULL
          LIMIT 1",
    )
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    if let Some((dims,)) = dims {
        vector_index::request(pool, Target::Chunks, dims as usize).await?;
    }
    Ok(())
}
