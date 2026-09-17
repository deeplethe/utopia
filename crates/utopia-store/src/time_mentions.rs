//! 一条陈述提到的时间词（0044 第一刀，#729）和它的读法（0045）。
//!
//! **一条时间提及是文档的字，永远不是模型算出来的日期。**「去年冬天」「合同签署后 30 日内」
//! 「Q3」——落库的是这几个字和它们在 `chunks.text` 里的字符偏移，不是某个模型写的
//! `TIMESTAMPTZ`。字留着，读法可以改；读错了改读法，不用删掉文档重抽。
//!
//! 读法分两层（0045 决定 2：模型读，代码算）：
//!
//! - **解释**（`shape` / `reference` / `granularity`）是模型给的：形状（点、区间、截至、
//!   时长、没给日期的结束）、参照（照写的绝对值，或锚点 + 偏移，或没有）、字说到梯子的
//!   哪一级。这里不认这些字段的取值——它们是抽取合同的词汇，库只照存。
//! - **解算**（`grade` / `resolved_*`）是代码从解释算出来的：两端各自的时刻与精度，梯子与
//!   `facts` 同一张（0024），结束端多一个 `unknown`。A 是原文写明的绝对日期，B 是按文档
//!   自己给的锚点算的，C 是没算出来（四列全空——算不出来的等着，决定 4）。`resolved_at`
//!   是最近一次算的时刻：锚点后来到了要重算。
//!
//! 一条陈述的 `when` 与 `ended` 各是一条提及（`role`），同一处字可以两次出现。陈述在世界
//! 轴上的位置（`facts.valid_*`）由 [`crate::graph::set_open_validity`] 按解算写，不由这里写。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use utopia_core::AppResult;
use uuid::Uuid;

/// 一条时间提及：哪条陈述、哪一块、照抄的字、在块里的字符偏移，以及它的读法（有了的话）
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TimeMention {
    pub id: Uuid,
    pub fact_id: Uuid,
    pub chunk_id: Uuid,
    /// 照抄的字
    pub text: String,
    /// 在 `chunks.text` 里的字符偏移（不是字节）
    pub char_start: i32,
    /// 来自陈述的哪个槽：`when` / `ended`
    pub role: String,
    /// 模型的解释：形状、参照、粒度。没读过为空
    pub shape: Option<String>,
    pub reference: Option<serde_json::Value>,
    pub granularity: Option<String>,
    /// 代码的解算：A 绝对、B 按锚点算出、C 没算出来。没算过为空
    pub grade: Option<String>,
    pub resolved_from: Option<DateTime<Utc>>,
    pub resolved_from_precision: Option<String>,
    pub resolved_to: Option<DateTime<Utc>>,
    /// 结束端多一个 `unknown`：结束了，不知哪天
    pub resolved_to_precision: Option<String>,
}

const COLUMNS: &str =
    "id, fact_id, chunk_id, text, char_start, role, shape, reference, granularity,
                       grade, resolved_from, resolved_from_precision, resolved_to,
                       resolved_to_precision";

/// 记一条时间提及。`char_start` 是字符偏移，**由服务端在块里搜出来**，不取模型报的数；
/// `role` 是它来自陈述的哪个槽（`when` / `ended`）。同一条陈述在同一块的同一位置、同一
/// 个槽只有一行；再记一次回的是那一行的 id
pub async fn record(
    pool: &PgPool,
    kb_id: Uuid,
    fact_id: Uuid,
    chunk_id: Uuid,
    text: &str,
    char_start: i32,
    role: &str,
) -> AppResult<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO time_mentions (id, kb_id, fact_id, chunk_id, text, char_start, role)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (fact_id, chunk_id, char_start, role) DO UPDATE SET text = time_mentions.text
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(fact_id)
    .bind(chunk_id)
    .bind(text)
    .bind(char_start)
    .bind(role)
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
    let rows: Vec<TimeMention> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM time_mentions
          WHERE fact_id = ANY($1)
          ORDER BY fact_id, chunk_id, char_start, role"
    ))
    .bind(fact_ids)
    .fetch_all(pool)
    .await?;
    for m in rows {
        out.entry(m.fact_id).or_default().push(m);
    }
    Ok(out)
}

/// 一条等着读的提及：字、位置、槽，和它所在的那句话——解算按文档的语境走一遍时的输入
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct MentionToResolve {
    pub id: Uuid,
    pub fact_id: Uuid,
    pub chunk_id: Uuid,
    pub text: String,
    pub char_start: i32,
    pub role: String,
    /// 这条陈述在这一块里的引文（`fact_evidence.quote`）；证据没留引文时退回整块的字
    pub sentence: String,
}

/// 一篇文档里所有等着读的提及：每条活着的开放陈述在这篇文档的活分块上的每一条提及，
/// 按分块顺序、块内位置排——解算沿着文档读下去，锚点才能从前一块带到后一块（0045 决定 3）。
/// 被新版本替掉的分块不在内：它的字已经不是这篇文档的字
pub async fn for_document(pool: &PgPool, document_id: Uuid) -> AppResult<Vec<MentionToResolve>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.fact_id, m.chunk_id, m.text, m.char_start, m.role,
                COALESCE(fe.quote, c.text) AS sentence
           FROM time_mentions m
           JOIN chunks c ON c.id = m.chunk_id
           JOIN facts f ON f.id = m.fact_id
           LEFT JOIN fact_evidence fe ON fe.fact_id = m.fact_id AND fe.chunk_id = m.chunk_id
          WHERE c.document_id = $1 AND c.superseded_at IS NULL
            AND f.layer = 'open' AND f.invalidated_at IS NULL
          ORDER BY c.seq, m.char_start, m.role",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?)
}

/// 模型对这条提及的解释（0045 决定 2）：形状、参照、粒度，照存。取值是抽取合同的词汇，
/// 库不认；日期算术不在这里
pub async fn set_interpretation(
    pool: &PgPool,
    id: Uuid,
    shape: &str,
    reference: &serde_json::Value,
    granularity: &str,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE time_mentions SET shape = $2, reference = $3, granularity = $4 WHERE id = $1",
    )
    .bind(id)
    .bind(shape)
    .bind(reference)
    .bind(granularity)
    .execute(pool)
    .await?;
    Ok(())
}

/// 代码对这条提及的解算：等级和两端各自的时刻与精度，`resolved_at` 记此刻。值先按精度截断
/// （[`crate::graph::truncate_to`]），存的值与精度说同一句话，数据库的 CHECK 才放行。
/// grade C 四列全空；结束端 `(None, Some("unknown"))` 是结束了不知哪天
pub async fn set_resolution(
    pool: &PgPool,
    id: Uuid,
    grade: &str,
    from: Option<DateTime<Utc>>,
    from_precision: Option<&str>,
    to: Option<DateTime<Utc>>,
    to_precision: Option<&str>,
) -> AppResult<()> {
    let from = from.map(|t| crate::graph::truncate_to(t, from_precision));
    let to = to.map(|t| crate::graph::truncate_to(t, to_precision));
    sqlx::query(
        "UPDATE time_mentions
            SET grade = $2, resolved_from = $3, resolved_from_precision = $4,
                resolved_to = $5, resolved_to_precision = $6, resolved_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(grade)
    .bind(from)
    .bind(from_precision)
    .bind(to)
    .bind(to_precision)
    .execute(pool)
    .await?;
    Ok(())
}
