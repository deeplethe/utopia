//! Agent/对话记忆：episodes 快速路径。
//!
//! 设计（零新轮子）：记忆空间 = 知识库本身；每条 episode = 隐式 "Memory" 来源下
//! "Memory log" 文档上追加的一个 chunk（文本内嵌事发时间戳行）。于是全链路复用：
//! chunk 进全文/向量索引（记忆可检索）、fact_evidence 指 chunk（记忆事实可溯源到
//! 原话）、extracted_at 增量抽取只处理新 episode、事实 valid_from 取事发时间、
//! 与既有 functional 事实矛盾时时态引擎自动闭合——"上月喜欢 A 本月改 B"自然
//! 变成两段区间。账本纪律：episodes 只追加，永不改写。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_core::AppResult;
use uuid::Uuid;

pub const MEMORY_SOURCE_KIND: &str = "memory";
const MEMORY_DOC_KEY: &str = "memory:log";

/// 每 KB 的隐式 Memory 来源（不可删；Library 里可见——记忆透明是特性）。
pub async fn get_or_create_memory_source(pool: &PgPool, kb_id: Uuid) -> AppResult<Uuid> {
    if let Some((id,)) = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM sources WHERE kb_id = $1 AND kind = $2 LIMIT 1",
    )
    .bind(kb_id)
    .bind(MEMORY_SOURCE_KIND)
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO sources (id, kb_id, kind, name, config) VALUES ($1, $2, $3, 'Memory', '{}')",
    )
    .bind(id)
    .bind(kb_id)
    .bind(MEMORY_SOURCE_KIND)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Memory log 文档（每 KB 一份）。绕开 documents::create 的同内容去重——
/// 它不是内容寻址的文件，sha256 填哨兵值。
pub async fn get_or_create_memory_doc(pool: &PgPool, kb_id: Uuid) -> AppResult<Uuid> {
    if let Some((id,)) = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM documents
          WHERE kb_id = $1 AND external_key = $2 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(kb_id)
    .bind(MEMORY_DOC_KEY)
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let source_id = get_or_create_memory_source(pool, kb_id).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO documents
            (id, kb_id, source_id, filename, mime, size_bytes, sha256,
             status, graph_status, external_key, doc_time_source)
         VALUES ($1, $2, $3, 'memory-log.md', 'text/markdown', 0, 'memory:log',
                 'ready', 'done', $4, 'none')",
    )
    .bind(id)
    .bind(kb_id)
    .bind(source_id)
    .bind(MEMORY_DOC_KEY)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 追加一条 episode：新 chunk（extracted_at 空 → 增量抽取会拾起；embedding 空 →
/// memory_ingest 会补）。事发时间内嵌进文本首行，抽取模型据此定 valid_from。
pub async fn append_episode(
    pool: &PgPool,
    kb_id: Uuid,
    text: &str,
    occurred_at: DateTime<Utc>,
) -> AppResult<(Uuid, Uuid)> {
    let doc_id = get_or_create_memory_doc(pool, kb_id).await?;
    let stamped = format!("[{}] {}", occurred_at.format("%Y-%m-%d %H:%M"), text.trim());
    let chunk_id = Uuid::now_v7();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text, char_start, char_end, doc_version)
         VALUES ($1, $2, $3,
                 (SELECT COALESCE(MAX(seq), -1) + 1 FROM chunks
                  WHERE document_id = $3 AND superseded_at IS NULL),
                 $4, 0, $5, 1)",
    )
    .bind(chunk_id)
    .bind(kb_id)
    .bind(doc_id)
    .bind(&stamped)
    .bind(stamped.chars().count() as i32)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE documents SET chunk_count = chunk_count + 1, size_bytes = size_bytes + $2,
                updated_at = now() WHERE id = $1",
    )
    .bind(doc_id)
    .bind(stamped.len() as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((doc_id, chunk_id))
}

/// 这篇文档是不是记忆日志。
///
/// **抽取据此决定事实要不要人点头**（0015）：记忆是人在对话里特意说的一句话，
/// 一次一条、人就在现场，确认成本最低；而摄进来的文档一次上万条，逐条确认
/// 是不可能的，那条路仍旧乐观写入 + 事后审阅。
pub async fn is_memory_document(pool: &PgPool, document_id: Uuid) -> AppResult<bool> {
    let found: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM documents d JOIN sources s ON s.id = d.source_id
          WHERE d.id = $1 AND s.kind = $2",
    )
    .bind(document_id)
    .bind(MEMORY_SOURCE_KIND)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}
