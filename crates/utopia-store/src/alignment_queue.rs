//! 对齐队列（#725 的「Alignment」一档）：对齐器两票不一致、拿不定的签名与类别词，等人来定。
//!
//! 两种条目走同一档：一条短语签名（短语 × 两端的类，几条例句，两票各选了什么）要人定
//! 属性和方向或「没有」；一个类别词（词、写法、例名、两票）要人定类或「没有」。人定了写回
//! `phrase_bindings` / `type_bindings`，`decided_by = 'person'`，代理此后不再改它；短语定了
//! 类型化图谱按新绑定重算，类别词定了它名下的实体换类、短语签名跟着变。
//!
//! 卡片由账本拼：这里只读两张绑定表和类别词签名，不调模型。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

/// 队列里的一条：短语签名或类别词。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlignmentItem {
    Phrase {
        id: Uuid,
        phrase: String,
        subject_class: Option<String>,
        object_class: Option<String>,
        object_is_value: bool,
        statement_count: i32,
        /// 判定时记下的例句（「主语 —短语→ 宾语」）
        examples: Vec<String>,
        /// 两票各选了什么：`{"first": {...} | null, "second": {...} | null}`
        votes: Option<serde_json::Value>,
        decided_at: DateTime<Utc>,
    },
    KindWord {
        kind_word: String,
        words: Vec<String>,
        examples: Vec<String>,
        phrases: Vec<String>,
        entity_count: i64,
        votes: Option<serde_json::Value>,
        decided_at: DateTime<Utc>,
    },
}

#[derive(sqlx::FromRow)]
struct PhraseRow {
    id: Uuid,
    phrase: String,
    subject_class: Option<String>,
    object_class: Option<String>,
    object_is_value: bool,
    statement_count: i32,
    examples: Vec<String>,
    votes: Option<serde_json::Value>,
    decided_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct KindWordRow {
    kind_word: String,
    words: Vec<String>,
    votes: Option<serde_json::Value>,
    decided_at: DateTime<Utc>,
}

/// 等人定的条目，先来的在前，两种混排。
pub async fn list(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<AlignmentItem>> {
    let phrases: Vec<PhraseRow> = sqlx::query_as(
        "SELECT b.id, b.phrase, st.key AS subject_class, ot.key AS object_class,
                b.object_is_value, b.statement_count, b.examples, b.votes, b.decided_at
           FROM phrase_bindings b
      LEFT JOIN entity_types st ON st.id = b.subject_type_id
      LEFT JOIN entity_types ot ON ot.id = b.object_type_id
          WHERE b.kb_id = $1 AND b.status = 'undecided'",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let words: Vec<KindWordRow> = sqlx::query_as(
        "SELECT kind_word, words, votes, decided_at
           FROM type_bindings WHERE kb_id = $1 AND status = 'undecided'",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    // 类别词的例名和短语来自签名（活着的实体数出来的），按词对上
    let sigs = if words.is_empty() {
        Vec::new()
    } else {
        crate::type_bindings::signatures(pool, kb_id).await?
    };
    let mut items: Vec<(DateTime<Utc>, AlignmentItem)> = phrases
        .into_iter()
        .map(|r| {
            (
                r.decided_at,
                AlignmentItem::Phrase {
                    id: r.id,
                    phrase: r.phrase,
                    subject_class: r.subject_class,
                    object_class: r.object_class,
                    object_is_value: r.object_is_value,
                    statement_count: r.statement_count,
                    examples: r.examples,
                    votes: r.votes,
                    decided_at: r.decided_at,
                },
            )
        })
        .collect();
    for w in words {
        let sig = sigs.iter().find(|s| s.kind_word == w.kind_word);
        items.push((
            w.decided_at,
            AlignmentItem::KindWord {
                kind_word: w.kind_word,
                words: w.words,
                examples: sig.map(|s| s.examples.clone()).unwrap_or_default(),
                phrases: sig.map(|s| s.phrases.clone()).unwrap_or_default(),
                entity_count: sig.map(|s| s.count).unwrap_or(0),
                votes: w.votes,
                decided_at: w.decided_at,
            },
        ));
    }
    items.sort_by_key(|(at, _)| *at);
    Ok(items
        .into_iter()
        .map(|(_, item)| item)
        .skip(usize::try_from(offset).unwrap_or(0))
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .collect())
}

/// 等人定的条目数，以及最老的一条从什么时候起在等。
pub async fn waiting(pool: &PgPool, kb_id: Uuid) -> AppResult<(i64, Option<DateTime<Utc>>)> {
    Ok(sqlx::query_as(
        "SELECT count(*), min(decided_at) FROM (
            SELECT decided_at FROM phrase_bindings WHERE kb_id = $1 AND status = 'undecided'
            UNION ALL
            SELECT decided_at FROM type_bindings WHERE kb_id = $1 AND status = 'undecided'
         ) q",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?)
}
