use utopia_core::models::LlmSettings;
use utopia_core::AppResult;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn get(pool: &PgPool, workspace_id: Uuid) -> AppResult<Option<LlmSettings>> {
    let row = sqlx::query_as("SELECT * FROM llm_settings WHERE workspace_id = $1")
        .bind(workspace_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// upsert；api_key 传 None 表示保留旧值（前端不回传密钥）。
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    pool: &PgPool,
    workspace_id: Uuid,
    chat_base_url: Option<&str>,
    chat_api_key: Option<&str>,
    chat_model: Option<&str>,
    embed_base_url: Option<&str>,
    embed_api_key: Option<&str>,
    embed_model: Option<&str>,
    embed_dim: Option<i32>,
) -> AppResult<LlmSettings> {
    let row = sqlx::query_as(
        "INSERT INTO llm_settings
             (workspace_id, chat_base_url, chat_api_key, chat_model,
              embed_base_url, embed_api_key, embed_model, embed_dim, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (workspace_id) DO UPDATE SET
             chat_base_url  = EXCLUDED.chat_base_url,
             chat_api_key   = COALESCE(EXCLUDED.chat_api_key, llm_settings.chat_api_key),
             chat_model     = EXCLUDED.chat_model,
             embed_base_url = EXCLUDED.embed_base_url,
             embed_api_key  = COALESCE(EXCLUDED.embed_api_key, llm_settings.embed_api_key),
             embed_model    = EXCLUDED.embed_model,
             embed_dim      = EXCLUDED.embed_dim,
             updated_at     = now()
         RETURNING *",
    )
    .bind(workspace_id)
    .bind(chat_base_url)
    .bind(chat_api_key)
    .bind(chat_model)
    .bind(embed_base_url)
    .bind(embed_api_key)
    .bind(embed_model)
    .bind(embed_dim)
    .fetch_one(pool)
    .await?;
    Ok(row)
}
