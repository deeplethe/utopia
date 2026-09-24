use sqlx::PgPool;
use utopia_core::models::LlmSettings;
use utopia_core::{secrets, AppError, AppResult};
use uuid::Uuid;

/// 出库即开封：四把 API key 在库里是封印的（`utopia_core::secrets`）。
/// 任何返回 `LlmSettings` 的查询都从这里过
fn opened(mut s: LlmSettings) -> AppResult<LlmSettings> {
    s.chat_api_key = secrets::open_opt(s.chat_api_key.as_deref()).map_err(AppError::Other)?;
    s.embed_api_key = secrets::open_opt(s.embed_api_key.as_deref()).map_err(AppError::Other)?;
    s.ocr_api_key = secrets::open_opt(s.ocr_api_key.as_deref()).map_err(AppError::Other)?;
    s.transcribe_api_key =
        secrets::open_opt(s.transcribe_api_key.as_deref()).map_err(AppError::Other)?;
    Ok(s)
}

pub async fn get(pool: &PgPool, workspace_id: Uuid) -> AppResult<Option<LlmSettings>> {
    let row: Option<LlmSettings> =
        sqlx::query_as("SELECT * FROM llm_settings WHERE workspace_id = $1")
            .bind(workspace_id)
            .fetch_optional(pool)
            .await?;
    row.map(opened).transpose()
}

/// 任取一个配了对话模型的工作区设置。给端点探针用：端点地址是部署共用的，
/// 从哪个工作区的配置读到的都是同一个地方，而探针没有"当前工作区"这个上下文。
pub async fn any_with_chat(pool: &PgPool) -> AppResult<Option<LlmSettings>> {
    let row: Option<LlmSettings> = sqlx::query_as(
        "SELECT * FROM llm_settings
         WHERE chat_base_url IS NOT NULL AND chat_model IS NOT NULL
         ORDER BY workspace_id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    row.map(opened).transpose()
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
    // 入库即封印；None 仍是 None（保留旧值）
    let chat_api_key = secrets::seal_opt(chat_api_key);
    let embed_api_key = secrets::seal_opt(embed_api_key);
    let row: LlmSettings = sqlx::query_as(
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
    opened(row)
}

/// 对话模型的推理强度，单独改：它跟着对话模型那张卡走，但 `upsert` 的整体替换不认识它，
/// 老调用方不传也不该把它清掉。`None` = 清空（回到端点默认）
pub async fn set_chat_reasoning_effort(
    pool: &PgPool,
    workspace_id: Uuid,
    effort: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE llm_settings SET chat_reasoning_effort = $2, updated_at = now() WHERE workspace_id = $1",
    )
    .bind(workspace_id)
    .bind(effort)
    .execute(pool)
    .await?;
    Ok(())
}

/// 版面识别服务的设置，单独存：它在管理页上是自己的一张卡片，存它不该碰对话和嵌入那几列
/// （反过来也一样——`upsert` 不写这三列）。`api_key` 传 None 保留旧值；地址传 None = 关掉
pub async fn upsert_ocr(
    pool: &PgPool,
    workspace_id: Uuid,
    base_url: Option<&str>,
    api_key: Option<&str>,
    backend: Option<&str>,
) -> AppResult<LlmSettings> {
    let api_key = secrets::seal_opt(api_key);
    let row: LlmSettings = sqlx::query_as(
        "INSERT INTO llm_settings (workspace_id, ocr_base_url, ocr_api_key, ocr_backend, updated_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (workspace_id) DO UPDATE SET
             ocr_base_url = EXCLUDED.ocr_base_url,
             ocr_api_key  = COALESCE(EXCLUDED.ocr_api_key, llm_settings.ocr_api_key),
             ocr_backend  = EXCLUDED.ocr_backend,
             updated_at   = now()
         RETURNING *",
    )
    .bind(workspace_id)
    .bind(base_url)
    .bind(api_key)
    .bind(backend)
    .fetch_one(pool)
    .await?;
    opened(row)
}

/// 转写模型的设置，跟版面识别服务一样单独存（管理页上各是一张卡片）
pub async fn upsert_transcribe(
    pool: &PgPool,
    workspace_id: Uuid,
    base_url: Option<&str>,
    api_key: Option<&str>,
    model: Option<&str>,
) -> AppResult<LlmSettings> {
    let api_key = secrets::seal_opt(api_key);
    let row: LlmSettings = sqlx::query_as(
        "INSERT INTO llm_settings
             (workspace_id, transcribe_base_url, transcribe_api_key, transcribe_model, updated_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (workspace_id) DO UPDATE SET
             transcribe_base_url = EXCLUDED.transcribe_base_url,
             transcribe_api_key  = COALESCE(EXCLUDED.transcribe_api_key, llm_settings.transcribe_api_key),
             transcribe_model    = EXCLUDED.transcribe_model,
             updated_at          = now()
         RETURNING *",
    )
    .bind(workspace_id)
    .bind(base_url)
    .bind(api_key)
    .bind(model)
    .fetch_one(pool)
    .await?;
    opened(row)
}
