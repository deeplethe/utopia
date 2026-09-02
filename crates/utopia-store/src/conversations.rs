//! Chat 会话持久化：对话/消息仓储。轨迹（steps）与引用（sources）随
//! assistant 消息落库，历史回放与实时流共用同一数据形状。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::models::{ConversationMessage, ConversationView};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

pub async fn create(pool: &PgPool, kb_id: Uuid, user_id: Uuid, title: &str) -> AppResult<Uuid> {
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

/// 我在这个库里的会话。**可搜、可翻页**——标题会重（同一个问题问两次就重了），
/// 而固定一百条之后的会话界面上根本不存在。
///
/// 搜的是标题与消息正文两处：人记得住的往往是「我问过那个关于 Q3 的」，
/// 而那句话在正文里，标题可能被截成了别的样子。
pub async fn list(
    pool: &PgPool,
    kb_id: Uuid,
    user_id: Uuid,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<ConversationView>, i64)> {
    const WHERE: &str = "WHERE c.kb_id = $1 AND c.user_id = $2
           AND ($3::text IS NULL
                OR c.title ILIKE '%' || $3 || '%'
                OR EXISTS (SELECT 1 FROM conversation_messages m
                            WHERE m.conversation_id = c.id
                              AND m.content ILIKE '%' || $3 || '%'))";
    let rows: Vec<ConversationView> = sqlx::query_as(&format!(
        "SELECT c.id, c.title, c.created_at, c.updated_at,
                (SELECT count(*) FROM conversation_messages m
                 WHERE m.conversation_id = c.id) AS message_count
         FROM conversations c
         {WHERE}
         ORDER BY c.updated_at DESC
         LIMIT $4 OFFSET $5"
    ))
    .bind(kb_id)
    .bind(user_id)
    .bind(q)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM conversations c {WHERE}"))
        .bind(kb_id)
        .bind(user_id)
        .bind(q)
        .fetch_one(pool)
        .await?;
    Ok((rows, total))
}

/// 改一个会话的标题。
///
/// **标题本来是从第一句话自动取的**，而那句话往往不是它后来变成的样子——
/// 一段对话跑偏是常态，改名让人能按自己记得的方式找回它。
pub async fn rename(
    pool: &PgPool,
    kb_id: Uuid,
    user_id: Uuid,
    conversation_id: Uuid,
    title: &str,
) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(AppError::invalid(
            "bad_title",
            "Title must be 1-120 characters",
        ));
    }
    let res = sqlx::query(
        "UPDATE conversations SET title = $4
          WHERE id = $3 AND kb_id = $1 AND user_id = $2",
    )
    .bind(kb_id)
    .bind(user_id)
    .bind(conversation_id)
    .bind(title)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
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

pub async fn messages(pool: &PgPool, conversation_id: Uuid) -> AppResult<Vec<ConversationMessage>> {
    let rows: Vec<ConversationMessage> = sqlx::query_as(
        "SELECT id, role, content, steps, sources, created_at
         FROM conversation_messages WHERE conversation_id = $1 ORDER BY created_at",
    )
    .bind(conversation_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 一轮回放的历史。
///
/// 三样东西，各自回答一个不同的问题：正文（说过什么）、实体（认下了谁）、
/// 最近一轮的工具往返（**做过什么**）。
///
/// 第三样是后加的。原先的判断是「回放身份足矣：有了 id，下一轮直接调
/// entity_facts」——省下的是每轮堆积的 chunk 正文，那个考虑没错。但它把
/// 「我已经查过了」这件事也一起省掉了：跨轮之后模型只看得见自己写的散文，
/// 于是接着说「翻译」时重查一遍，还落到了另一批同名实体上。
///
/// 折中是**只回放最近一轮**：需要的是「我刚做过什么」，不是二十轮的输出。
pub struct History {
    /// `(role, content)`，按时间序
    pub turns: Vec<(String, String)>,
    /// 这场对话里已经认下的实体（去重）
    pub entities: Vec<serde_json::Value>,
    /// **最近一轮助手做过什么**：带 `tool_calls` 的助手消息与配套的 tool 结果。
    ///
    /// 只有最近一轮。这一段是为了让模型知道自己刚做过什么——接着说
    /// 「翻译」「短一点」的时候，证据就在眼前，不必重查（也就不会重查成
    /// 另一批同名实体）。搬二十轮的工具输出回来是另一回事，那正是当初
    /// 只存正文的理由。
    pub last_tool_exchange: Vec<serde_json::Value>,
}

pub async fn recent_context(pool: &PgPool, conversation_id: Uuid, n: i64) -> AppResult<History> {
    let mut rows: Vec<(
        String,
        String,
        serde_json::Value,
        serde_json::Value,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT role, content, resolved, tool_exchange, created_at FROM conversation_messages
         WHERE conversation_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(conversation_id)
    .bind(n)
    .fetch_all(pool)
    .await?;
    rows.reverse();
    // 实体按 id 去重、保持首次出现的顺序：同一个实体在几轮里反复出现是常态，
    // 每轮各列一遍只是把同一件事说三遍
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entities: Vec<serde_json::Value> = Vec::new();
    for (_, _, res, _, _) in &rows {
        for e in res.as_array().into_iter().flatten() {
            let Some(id) = e["id"].as_str() else { continue };
            if seen.insert(id.to_string()) {
                entities.push(e.clone());
            }
        }
    }
    // 最后一条助手消息的那一段。**倒着找**——最后一条通常是刚落库的用户消息
    let last_tool_exchange = rows
        .iter()
        .rev()
        .find(|(role, _, _, _, _)| role == "assistant")
        .and_then(|(_, _, _, ex, _)| ex.as_array().cloned())
        .unwrap_or_default();
    Ok(History {
        turns: rows.into_iter().map(|(r, c, _, _, _)| (r, c)).collect(),
        entities,
        last_tool_exchange,
    })
}

/// 一轮除了正文之外留下的东西。
///
/// **四个都是 `serde_json::Value`，散着传编译器帮不上忙**——传错顺序会得到
/// 一条能落库、也能读回来、只是内容张冠李戴的记录。与 `RelationAxioms` 同一条理由。
#[derive(Default)]
pub struct TurnRecord {
    /// 行动轨迹：调了什么、拿到多少（界面显示）
    pub steps: serde_json::Value,
    /// 引用清单
    pub sources: serde_json::Value,
    /// 这一轮认下的实体（id / 名字 / 类型）。下一轮回放，让模型接着走而不是重搜
    pub resolved: serde_json::Value,
    /// 这一轮调了什么、拿回什么（已截断的那一份）。下一轮回放最近的一段——
    /// 没有它，模型跨轮之后就不知道自己查过，于是重查
    pub tool_exchange: serde_json::Value,
}

impl TurnRecord {
    /// 用户消息：四样都空。
    pub fn empty() -> Self {
        Self {
            steps: serde_json::json!([]),
            sources: serde_json::json!([]),
            resolved: serde_json::json!([]),
            tool_exchange: serde_json::json!([]),
        }
    }
}

pub async fn append_message(
    pool: &PgPool,
    conversation_id: Uuid,
    role: &str,
    content: &str,
    rec: &TurnRecord,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO conversation_messages
             (id, conversation_id, role, content, steps, sources, resolved, tool_exchange)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(conversation_id)
    .bind(role)
    .bind(content)
    .bind(&rec.steps)
    .bind(&rec.sources)
    .bind(&rec.resolved)
    .bind(&rec.tool_exchange)
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
