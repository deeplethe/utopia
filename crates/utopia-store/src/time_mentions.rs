//! 一条陈述提到的时间词（0044 第一刀，#729；时间记录 0045）。
//!
//! **一条时间提及是文档的字，永远不是算出来的日期。**「去年冬天」「合同签署后 30 日内」
//! 「Q3」——落库的是这几个字和它们在 `chunks.text` 里的字符偏移，不是某个
//! `TIMESTAMPTZ`。今天抽取把「2023 年上半年」读成 1 月 1 日，读错了只能删掉文档重抽；
//! 字留着，读法可以改。把字读成日期（形状、锚点、偏移、粒度）是 0045 后面几刀的事，
//! 那几列到时候加在这张表上，这里写下的行一行不动。
//!
//! 一条开放陈述于是不写任何 `valid_*`：它在世界轴上还没有位置。

use sqlx::PgPool;
use std::collections::HashMap;
use utopia_core::AppResult;
use uuid::Uuid;

/// 一条时间提及：哪条陈述、哪一块、照抄的字、在块里的字符偏移
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TimeMention {
    pub id: Uuid,
    pub fact_id: Uuid,
    pub chunk_id: Uuid,
    /// 照抄的字
    pub text: String,
    /// 在 `chunks.text` 里的字符偏移（不是字节）
    pub char_start: i32,
}

/// 记一条时间提及。`char_start` 是字符偏移，**由服务端在块里搜出来**，不取模型报的数。
/// 同一条陈述在同一块的同一位置只有一行；再记一次回的是那一行的 id
pub async fn record(
    pool: &PgPool,
    kb_id: Uuid,
    fact_id: Uuid,
    chunk_id: Uuid,
    text: &str,
    char_start: i32,
) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (fact_id, chunk_id, char_start) DO UPDATE SET text = time_mentions.text
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(fact_id)
    .bind(chunk_id)
    .bind(text)
    .bind(char_start)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// 一批陈述各自提到的时间词，按事实 id 取回；块内按出现位置排
pub async fn for_facts(
    pool: &PgPool,
    fact_ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<TimeMention>>> {
    let mut out: HashMap<Uuid, Vec<TimeMention>> = HashMap::new();
    if fact_ids.is_empty() {
        return Ok(out);
    }
    let rows: Vec<TimeMention> = sqlx::query_as(
        "SELECT id, fact_id, chunk_id, text, char_start
         FROM time_mentions
         WHERE fact_id = ANY($1)
         ORDER BY fact_id, chunk_id, char_start",
    )
    .bind(fact_ids)
    .fetch_all(pool)
    .await?;
    for m in rows {
        out.entry(m.fact_id).or_default().push(m);
    }
    Ok(out)
}
