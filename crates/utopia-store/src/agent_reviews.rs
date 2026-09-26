//! 本体代理看过的形状（0061 cut 1.1）：提了、已有、没提各记一行，下一轮据此不再送。

use serde_json::Value;
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Review {
    /// phrase | kind_word
    pub kind: String,
    pub shape: Value,
    /// proposed | existing | declined
    pub outcome: String,
    pub target: Option<String>,
    /// 看它时词表的指纹；变了，已有与没提的形状再送一次
    pub basis: String,
}

pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Review>> {
    Ok(sqlx::query_as(
        "SELECT kind, shape, outcome, target, basis FROM ontology_agent_reviews WHERE kb_id = $1",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 记一轮看过的。同一形状再看是刷新
pub async fn record(pool: &PgPool, kb_id: Uuid, items: &[Review]) -> AppResult<()> {
    for it in items {
        sqlx::query(
            "INSERT INTO ontology_agent_reviews (kb_id, kind, shape, outcome, target, basis)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (kb_id, kind, shape) DO UPDATE
               SET outcome = EXCLUDED.outcome, target = EXCLUDED.target,
                   basis = EXCLUDED.basis, reviewed_at = now()",
        )
        .bind(kb_id)
        .bind(&it.kind)
        .bind(&it.shape)
        .bind(&it.outcome)
        .bind(&it.target)
        .bind(&it.basis)
        .execute(pool)
        .await?;
    }
    Ok(())
}
