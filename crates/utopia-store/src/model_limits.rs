//! 按模型的并发限制。
//!
//! 真正的约束是供应商的速率限制，而那是按 (base_url, model) 来的——本地 Ollama
//! 可能只扛 2 个并发，托管 API 能吃 50。此前只有一个部署级 `worker_concurrency`
//! 管住所有任务，等于用一个数字管两种完全不同的东西。
//!
//! 没配过的模型走 `deployment_settings.default_model_concurrency`（缺省 10）。

use sqlx::PgPool;
use utopia_core::models::ModelLimit;
use utopia_core::AppResult;

/// 这个模型允许多少并发。没有专属配置就走部署缺省。
///
/// 每次 LLM 调用前查一次——相对于一次动辄二十几秒的调用，这个查询的成本可以
/// 忽略，换来的是"管理员改完即时生效"，不必做缓存失效。
pub async fn limit_for(pool: &PgPool, base_url: &str, model: &str) -> AppResult<usize> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT max_concurrent FROM model_concurrency WHERE base_url = $1 AND model = $2",
    )
    .bind(base_url)
    .bind(model)
    .fetch_optional(pool)
    .await?;
    if let Some((n,)) = row {
        return Ok(n.max(1) as usize);
    }
    let (dflt,): (i32,) =
        sqlx::query_as("SELECT default_model_concurrency FROM deployment_settings LIMIT 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or((10,));
    Ok(dflt.max(1) as usize)
}

/// 已配置的模型 + 部署缺省（管理页一次取回）。
pub async fn list(pool: &PgPool) -> AppResult<(Vec<ModelLimit>, i32)> {
    let rows: Vec<ModelLimit> = sqlx::query_as(
        "SELECT base_url, model, max_concurrent FROM model_concurrency ORDER BY base_url, model",
    )
    .fetch_all(pool)
    .await?;
    let (dflt,): (i32,) =
        sqlx::query_as("SELECT default_model_concurrency FROM deployment_settings LIMIT 1")
            .fetch_optional(pool)
            .await?
            .unwrap_or((10,));
    Ok((rows, dflt))
}

/// 设一个模型的并发。`max_concurrent` 为 None 表示删掉专属配置、回落到缺省。
pub async fn set(
    pool: &PgPool,
    base_url: &str,
    model: &str,
    max_concurrent: Option<i32>,
) -> AppResult<()> {
    match max_concurrent {
        Some(n) => {
            if !(1..=256).contains(&n) {
                return Err(utopia_core::AppError::Validation(
                    "max_concurrent must be between 1 and 256".into(),
                ));
            }
            sqlx::query(
                "INSERT INTO model_concurrency (base_url, model, max_concurrent)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (base_url, model)
                 DO UPDATE SET max_concurrent = EXCLUDED.max_concurrent, updated_at = now()",
            )
            .bind(base_url)
            .bind(model)
            .bind(n)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM model_concurrency WHERE base_url = $1 AND model = $2")
                .bind(base_url)
                .bind(model)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// 部署缺省并发（未单独配置的模型都走它）。
pub async fn set_default(pool: &PgPool, value: i32) -> AppResult<()> {
    if !(1..=256).contains(&value) {
        return Err(utopia_core::AppError::Validation(
            "default_model_concurrency must be between 1 and 256".into(),
        ));
    }
    sqlx::query("UPDATE deployment_settings SET default_model_concurrency = $1")
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

/// 部署里实际在用的模型（各工作区设置里出现过的），供管理页列出可配置项。
pub async fn models_in_use(pool: &PgPool) -> AppResult<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT DISTINCT chat_base_url, chat_model, 'chat' FROM llm_settings
          WHERE chat_base_url IS NOT NULL AND chat_model IS NOT NULL
         UNION
         SELECT DISTINCT embed_base_url, embed_model, 'embed' FROM llm_settings
          WHERE embed_base_url IS NOT NULL AND embed_model IS NOT NULL",
    )
    .fetch_all(pool)
    .await?)
}
