//! Chat 会话持久化：对话/消息仓储。轨迹（steps）与引用（sources）随
//! assistant 消息落库，历史回放与实时流共用同一数据形状。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::models::{ConversationMessage, ConversationView};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

pub async fn create(
    pool: &PgPool,
    kb_id: Uuid,
    user_id: Uuid,
    title: &str,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, kb_id, user_id, title) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(kb_id)
        .bind(user_id)
        .bind(title.chars().take(80).collect::<String>())
        .execute(pool)
        .await?;
    Ok(id)
}

/// 本人在本 KB 的会话列表（新→旧）。
pub async fn list(pool: &PgPool, kb_id: Uuid, user_id: Uuid) -> AppResult<Vec<ConversationView>> {
    let rows: Vec<ConversationView> = sqlx::query_as(
        "SELECT id, title, created_at, updated_at,
                (SELECT count(*) FROM conversation_messages m
                 WHERE m.conversation_id = c.id) AS message_count
         FROM conversations c
         WHERE kb_id = $1 AND user_id = $2
         ORDER BY updated_at DESC
         LIMIT 100",
    )
    .bind(kb_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 归属校验：会话必须属于本 KB 本人。
pub async fn require_owned(
    pool: &PgPool,
    kb_id: Uuid,
    user_id: Uuid,
    conversation_id: Uuid,
) -> AppResult<()> {
    let found: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM conversations WHERE id = $1 AND kb_id = $2 AND user_id = $3",
    )
    .bind(conversation_id)
    .bind(kb_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    found.map(|_| ()).ok_or(AppError::NotFound)
}

pub async fn messages(
    pool: &PgPool,
    conversation_id: Uuid,
) -> AppResult<Vec<ConversationMessage>> {
    let rows: Vec<ConversationMessage> = sqlx::query_as(
        "SELECT id, role, content, steps, sources, created_at
         FROM conversation_messages WHERE conversation_id = $1 ORDER BY created_at",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 服务端拼上下文用：最近 n 条的 (role, content)，按时间正序。
pub async fn recent_context(
    pool: &PgPool,
    conversation_id: Uuid,
    n: i64,
) -> AppResult<Vec<(String, String)>> {
    let mut rows: Vec<(String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT role, content, created_at FROM conversation_messages
         WHERE conversation_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(conversation_id)
    .bind(n)
    .fetch_all(pool)
    .await?;
    rows.reverse();
    Ok(rows.into_iter().map(|(r, c, _)| (r, c)).collect())
}

pub async fn append_message(
    pool: &PgPool,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    steps: &serde_json::Value,
    sources: &serde_json::Value,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO conversation_messages (id, conversation_id, role, content, steps, sources)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(steps)
    .bind(sources)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE conversations SET updated_at = now() WHERE id = $1")
        .bind(conversation_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn delete(
    pool: &PgPool,
    kb_id: Uuid,
    user_id: Uuid,
    conversation_id: Uuid,
) -> AppResult<()> {
    let res =
        sqlx::query("DELETE FROM conversations WHERE id = $1 AND kb_id = $2 AND user_id = $3")
            .bind(conversation_id)
            .bind(kb_id)
            .bind(user_id)
            .execute(pool)
            .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}
