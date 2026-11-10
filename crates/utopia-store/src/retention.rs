//! An old version keeps no vector, where a deployment asks for that.
//!
//! `replace_chunks` supersedes a chunk the new version of its document no longer
//! carries and keeps the row whole: the text for version replay and the vector for an
//! as-of search ([0019](../../../docs/decisions/0019-the-second-clock-can-be-rewound.md)
//! open question ②). A `statements` source pushes everything on every pass, so a base of
//! mirrored statements grows a superseded chunk, vector included, for every paragraph
//! that changed, forever. The vector is the larger part of the row by far.
//!
//! Off unless `UTOPIA_SUPERSEDED_VECTOR_DAYS` is set. Then a sweep runs once a day per
//! base, and a chunk superseded longer ago than that many days loses its vector. The row
//! stays: its text, and whatever evidence, pending fact, time mention or role word points
//! at it. A vector can be computed again from the text; a row could not.
//!
//! A deleted document is left whole. `documents::delete` supersedes its live chunks and
//! `documents::restore` makes them live again, so a vector cleared in between would bring
//! the document back unsearchable by meaning.

use chrono::{Duration, Utc};
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

pub const JOB_KIND: &str = "prune_superseded";
pub const SHORTEST_WINDOW_DAYS: i64 = 1;
pub const LONGEST_WINDOW_DAYS: i64 = 3650;
const BATCH: i64 = 10_000;

/// How long a superseded chunk keeps its vector. Zero or less would clear the vector of a
/// chunk superseded a moment ago, and `Duration::days` panics on a count it cannot hold
pub fn window(days: i64) -> Duration {
    Duration::days(days.clamp(SHORTEST_WINDOW_DAYS, LONGEST_WINDOW_DAYS))
}

/// Every base, for the daily tick: a base with nothing old enough costs one index probe.
pub async fn bases(pool: &PgPool) -> AppResult<Vec<Uuid>> {
    let ids: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM knowledge_bases")
        .fetch_all(pool)
        .await?;
    Ok(ids.into_iter().map(|(id,)| id).collect())
}

/// Clears the vector of every chunk of the base superseded more than `days` ago
/// ([`window`]) and returns how many it cleared
pub async fn prune_superseded(pool: &PgPool, kb_id: Uuid, days: i64) -> AppResult<u64> {
    let cutoff = Utc::now() - window(days);
    let mut total = 0;
    loop {
        let cleared = sqlx::query(
            "UPDATE chunks SET embedding = NULL
              WHERE id IN (SELECT c.id FROM chunks c JOIN documents d ON d.id = c.document_id
                            WHERE c.kb_id = $1 AND c.superseded_at < $2
                              AND d.deleted_at IS NULL AND c.embedding IS NOT NULL
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

#[cfg(test)]
mod tests {
    use super::window;
    use chrono::Duration;

    #[test]
    fn a_window_of_30_days_is_30_days() {
        assert_eq!(window(30), Duration::days(30));
    }

    #[test]
    fn a_window_of_0_or_minus_5_days_is_1_day() {
        assert_eq!(window(0), Duration::days(1));
        assert_eq!(window(-5), Duration::days(1));
    }

    #[test]
    fn a_window_of_i64_max_days_is_3650_days() {
        assert_eq!(window(i64::MAX), Duration::days(3650));
    }
}
