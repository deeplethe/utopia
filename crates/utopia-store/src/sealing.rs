//! 凭据补封：升级前落库的明文凭据，在钥匙装好之后的第一次启动补成密文。
//!
//! 幂等，每次启动都跑：判据是「值没有 `enc:v1:` 前缀」，已封的不动。四处凭据
//! （`llm_settings` 两把 key、`data_sources.conn_string`、`sources.config` 的凭据键、
//! `sources.ingest_token`）在这里各扫一遍——加了新的存放处要同时加到这里，否则那一处
//! 会一直是明文而没有任何地方报警。

use sqlx::PgPool;
use utopia_core::models::SOURCE_SECRET_KEYS;
use utopia_core::{secrets, AppResult};
use uuid::Uuid;

/// 返回补封的行数。没装钥匙时什么都不做
pub async fn backfill(pool: &PgPool) -> AppResult<usize> {
    if !secrets::is_ready() {
        return Ok(0);
    }
    Ok(seal_llm_settings(pool, None).await?
        + seal_data_sources(pool, None).await?
        + seal_sources(pool, None).await?)
}

/// `only` = 只补这一个工作区（测试用；启动时传 None 扫全部）
pub async fn seal_llm_settings(pool: &PgPool, only: Option<Uuid>) -> AppResult<usize> {
    let rows: Vec<(Uuid, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT workspace_id, chat_api_key, embed_api_key FROM llm_settings
          WHERE ($1::uuid IS NULL OR workspace_id = $1)
            AND ((chat_api_key IS NOT NULL AND chat_api_key NOT LIKE 'enc:v1:%')
              OR (embed_api_key IS NOT NULL AND embed_api_key NOT LIKE 'enc:v1:%'))",
    )
    .bind(only)
    .fetch_all(pool)
    .await?;
    let n = rows.len();
    for (ws, chat, embed) in rows {
        sqlx::query(
            "UPDATE llm_settings SET chat_api_key = $2, embed_api_key = $3 WHERE workspace_id = $1",
        )
        .bind(ws)
        .bind(secrets::seal_opt(chat.as_deref()))
        .bind(secrets::seal_opt(embed.as_deref()))
        .execute(pool)
        .await?;
    }
    Ok(n)
}

pub async fn seal_data_sources(pool: &PgPool, only: Option<Uuid>) -> AppResult<usize> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, conn_string FROM data_sources
          WHERE ($1::uuid IS NULL OR id = $1) AND conn_string NOT LIKE 'enc:v1:%'",
    )
    .bind(only)
    .fetch_all(pool)
    .await?;
    let n = rows.len();
    for (id, conn) in rows {
        sqlx::query("UPDATE data_sources SET conn_string = $2 WHERE id = $1")
            .bind(id)
            .bind(secrets::seal(&conn))
            .execute(pool)
            .await?;
    }
    Ok(n)
}

/// 来源表小，整张读回来在 Rust 里判：凭据键藏在 JSON 里，SQL 判起来反而绕
pub async fn seal_sources(pool: &PgPool, only: Option<Uuid>) -> AppResult<usize> {
    let rows: Vec<(Uuid, serde_json::Value, Option<String>)> = sqlx::query_as(
        "SELECT id, config, ingest_token FROM sources WHERE ($1::uuid IS NULL OR id = $1)",
    )
    .bind(only)
    .fetch_all(pool)
    .await?;
    let mut n = 0;
    for (id, mut config, token) in rows {
        let plain_key = config.as_object().is_some_and(|o| {
            SOURCE_SECRET_KEYS.iter().any(|k| {
                o.get(*k)
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !secrets::is_sealed(s))
            })
        });
        let plain_token = token.as_deref().is_some_and(|t| !secrets::is_sealed(t));
        if !plain_key && !plain_token {
            continue;
        }
        secrets::seal_json_keys(&mut config, SOURCE_SECRET_KEYS);
        sqlx::query("UPDATE sources SET config = $2, ingest_token = $3 WHERE id = $1")
            .bind(id)
            .bind(config)
            .bind(secrets::seal_opt(token.as_deref()))
            .execute(pool)
            .await?;
        n += 1;
    }
    Ok(n)
}
