use utopia_core::models::{ChunkView, Document};
use utopia_core::{AppError, AppResult};
use utopia_ingest::ChunkPiece;
use pgvector::Vector;
use sqlx::PgPool;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    kb_id: Uuid,
    filename: &str,
    mime: &str,
    size_bytes: i64,
    sha256: &str,
    source_id: Option<Uuid>,
    doc_time: Option<chrono::DateTime<chrono::Utc>>,
    external_key: Option<&str>,
) -> AppResult<Document> {
    sqlx::query_as(
        "INSERT INTO documents (id, kb_id, filename, mime, size_bytes, sha256, source_id,
                                doc_time, doc_time_source, external_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, COALESCE($8, now()), $9, $10) RETURNING *",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(filename)
    .bind(mime)
    .bind(size_bytes)
    .bind(sha256)
    .bind(source_id)
    .bind(doc_time)
    .bind(if doc_time.is_some() { "source" } else { "upload_time" })
    .bind(external_key)
    .fetch_one(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(format!("File already exists (identical content): {filename}"))
        }
        _ => AppError::Db(e),
    })
}

pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Document>> {
    let rows = sqlx::query_as("SELECT * FROM documents WHERE kb_id = $1 ORDER BY created_at DESC")
        .bind(kb_id)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

pub async fn get(pool: &PgPool, id: Uuid) -> AppResult<Document> {
    sqlx::query_as("SELECT * FROM documents WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// 按来源内逻辑身份查文档（同步三路判定用）。
pub async fn find_by_external_key(
    pool: &PgPool,
    source_id: Uuid,
    external_key: &str,
) -> AppResult<Option<Document>> {
    Ok(
        sqlx::query_as("SELECT * FROM documents WHERE source_id = $1 AND external_key = $2")
            .bind(source_id)
            .bind(external_key)
            .fetch_optional(pool)
            .await?,
    )
}

/// 按来源 + 内容哈希查文档（改名/移动识别用）。
pub async fn find_by_source_sha(
    pool: &PgPool,
    source_id: Uuid,
    sha256: &str,
) -> AppResult<Option<Document>> {
    Ok(sqlx::query_as(
        "SELECT * FROM documents WHERE source_id = $1 AND sha256 = $2 LIMIT 1",
    )
    .bind(source_id)
    .bind(sha256)
    .fetch_optional(pool)
    .await?)
}

/// 迁移前的历史文档（无 external_key）按文件名认领——0008 之前建的行的一次性兜底。
pub async fn find_legacy_by_filename(
    pool: &PgPool,
    source_id: Uuid,
    filename: &str,
) -> AppResult<Option<Document>> {
    Ok(sqlx::query_as(
        "SELECT * FROM documents
         WHERE source_id = $1 AND external_key IS NULL AND filename = $2 LIMIT 1",
    )
    .bind(source_id)
    .bind(filename)
    .fetch_optional(pool)
    .await?)
}

/// 给历史文档补上逻辑身份。
pub async fn adopt_external_key(pool: &PgPool, id: Uuid, external_key: &str) -> AppResult<()> {
    sqlx::query("UPDATE documents SET external_key = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(external_key)
        .execute(pool)
        .await?;
    Ok(())
}

/// 变更：原地替换文档内容（新 sha），状态回 pending 待重跑管道。
#[allow(clippy::too_many_arguments)]
pub async fn replace_content(
    pool: &PgPool,
    id: Uuid,
    filename: &str,
    mime: &str,
    size_bytes: i64,
    sha256: &str,
    doc_time: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET filename = $2, mime = $3, size_bytes = $4, sha256 = $5,
                doc_time = COALESCE($6, doc_time),
                status = 'pending', graph_status = 'none', error = NULL,
                missing_since = NULL, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(filename)
    .bind(mime)
    .bind(size_bytes)
    .bind(sha256)
    .bind(doc_time)
    .execute(pool)
    .await?;
    Ok(())
}

/// 移动/改名：同内容换了路径，只更新身份，不重跑管道。
pub async fn update_location(
    pool: &PgPool,
    id: Uuid,
    filename: &str,
    external_key: &str,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET filename = $2, external_key = $3, missing_since = NULL,
                updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(filename)
    .bind(external_key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 记录一个内容版本（版本号自增）。
pub async fn record_version(
    pool: &PgPool,
    document_id: Uuid,
    sha256: &str,
    size_bytes: i64,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO document_versions (id, document_id, version, sha256, size_bytes)
         VALUES ($1, $2,
                 (SELECT coalesce(max(version), 0) + 1 FROM document_versions WHERE document_id = $2),
                 $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(document_id)
    .bind(sha256)
    .bind(size_bytes)
    .execute(pool)
    .await?;
    Ok(())
}

/// 全集对账（仅适用于能看到来源完整现状的类型，如 url 的配置列表）：
/// 本轮见到的 key 清除 missing 标记，没见到的打上标记。
/// rss（滑动窗口）与 custom 的增量响应（?since=）不可用此函数——缺席≠删除。
pub async fn reconcile_missing(
    pool: &PgPool,
    source_id: Uuid,
    seen_keys: &[String],
) -> AppResult<()> {
    clear_missing_keys(pool, source_id, seen_keys).await?;
    sqlx::query(
        "UPDATE documents SET missing_since = now(), updated_at = now()
         WHERE source_id = $1 AND missing_since IS NULL
           AND external_key IS NOT NULL AND NOT (external_key = ANY($2))",
    )
    .bind(source_id)
    .bind(seen_keys)
    .execute(pool)
    .await?;
    Ok(())
}

/// 本轮出现的条目清除 missing 标记（条目失而复得）。
pub async fn clear_missing_keys(
    pool: &PgPool,
    source_id: Uuid,
    keys: &[String],
) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET missing_since = NULL, updated_at = now()
         WHERE source_id = $1 AND missing_since IS NOT NULL AND external_key = ANY($2)",
    )
    .bind(source_id)
    .bind(keys)
    .execute(pool)
    .await?;
    Ok(())
}

/// 显式墓碑（custom 响应的 deleted[]）：来源声明删除才打标，绝不因缺席推断。
pub async fn mark_missing_keys(
    pool: &PgPool,
    source_id: Uuid,
    keys: &[String],
) -> AppResult<u64> {
    let res = sqlx::query(
        "UPDATE documents SET missing_since = now(), updated_at = now()
         WHERE source_id = $1 AND missing_since IS NULL AND external_key = ANY($2)",
    )
    .bind(source_id)
    .bind(keys)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// 该来源下所有已标记 missing 的文档 id（批量清理用）。
pub async fn list_missing(pool: &PgPool, source_id: Uuid) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM documents WHERE source_id = $1 AND missing_since IS NOT NULL",
    )
    .bind(source_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn set_status(pool: &PgPool, id: Uuid, status: &str) -> AppResult<()> {
    sqlx::query("UPDATE documents SET status = $2, error = NULL, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_failed(pool: &PgPool, id: Uuid, error: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET status = 'failed', error = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_ready(pool: &PgPool, id: Uuid, text_len: i32, chunk_count: i32) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET status = 'ready', error = NULL, text_len = $2, chunk_count = $3,
                updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(text_len)
    .bind(chunk_count)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM documents WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 重建文档分块（事务内，幂等）。返回 (chunk_id, text) 供全文索引。
///
/// 认领式增量：新块先按文本内容在现行块里找同款——找到即"认领"（原行只更新
/// 序号/偏移/版本，身份延续：embedding、extracted_at、证据链接全部原地保鲜，
/// 未变段落不重抽也不会被误标"证据过期"）；没同款的才新建。落选的旧块软删除
/// （superseded_at 打标）而非物理删除——fact_evidence 引用不断链、旧版可回放，
/// embedding 清空（旧版不参与检索，向量是存储大头不留）。
pub async fn replace_chunks(
    pool: &PgPool,
    kb_id: Uuid,
    document_id: Uuid,
    pieces: &[ChunkPiece],
) -> AppResult<Vec<(String, String)>> {
    let mut tx = pool.begin().await?;
    let (version,): (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(version), 1) FROM document_versions WHERE document_id = $1",
    )
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await?;

    // 认领池：现行块按文本分组（同文重复块按多重集配对，各认领各的）
    let old: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, text FROM chunks WHERE document_id = $1 AND superseded_at IS NULL",
    )
    .bind(document_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut claim_pool: std::collections::HashMap<String, Vec<Uuid>> =
        std::collections::HashMap::new();
    for (id, text) in old {
        claim_pool.entry(text).or_default().push(id);
    }

    // 第一阶段：认领（原行更新）与待插清单——新块必须等软删跑完再插，
    // 否则"落选"判定会把本轮刚插入的新块一并软删
    let mut adopted: Vec<Uuid> = Vec::new();
    let mut to_insert: Vec<(Uuid, &ChunkPiece)> = Vec::new();
    let mut out = Vec::with_capacity(pieces.len());
    for piece in pieces {
        let claimed = claim_pool.get_mut(&piece.text).and_then(|ids| ids.pop());
        if let Some(id) = claimed {
            sqlx::query(
                "UPDATE chunks SET seq = $2, char_start = $3, char_end = $4, doc_version = $5
                 WHERE id = $1",
            )
            .bind(id)
            .bind(piece.seq)
            .bind(piece.char_start)
            .bind(piece.char_end)
            .bind(version)
            .execute(&mut *tx)
            .await?;
            adopted.push(id);
            out.push((id.to_string(), piece.text.clone()));
        } else {
            let id = Uuid::now_v7();
            to_insert.push((id, piece));
            out.push((id.to_string(), piece.text.clone()));
        }
    }

    // 第二阶段：落选旧块（新版里没有同款文本）→ 软删
    sqlx::query(
        "UPDATE chunks SET superseded_at = now(), embedding = NULL
         WHERE document_id = $1 AND superseded_at IS NULL AND NOT (id = ANY($2))",
    )
    .bind(document_id)
    .bind(&adopted)
    .execute(&mut *tx)
    .await?;

    // 第三阶段：插入新块
    for (id, piece) in to_insert {
        sqlx::query(
            "INSERT INTO chunks
                (id, kb_id, document_id, seq, text, char_start, char_end, doc_version)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(kb_id)
        .bind(document_id)
        .bind(piece.seq)
        .bind(&piece.text)
        .bind(piece.char_start)
        .bind(piece.char_end)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(out)
}

/// 抽取完成一个分块即打标（认领的块携带标记跳过重抽；也让中断的抽取可续跑）。
pub async fn mark_chunk_extracted(pool: &PgPool, chunk_id: Uuid) -> AppResult<()> {
    sqlx::query("UPDATE chunks SET extracted_at = now() WHERE id = $1")
        .bind(chunk_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 清除文档现行分块的抽取标记（手动 Extract = 强制全量重抽）。
pub async fn clear_extraction_marks(pool: &PgPool, document_id: Uuid) -> AppResult<()> {
    sqlx::query(
        "UPDATE chunks SET extracted_at = NULL
         WHERE document_id = $1 AND superseded_at IS NULL",
    )
    .bind(document_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// 文档查看器：全部分块（按顺序）。
pub async fn chunks_full(
    pool: &PgPool,
    document_id: Uuid,
) -> AppResult<Vec<utopia_core::models::ChunkFull>> {
    let rows = sqlx::query_as(
        "SELECT id, seq, text FROM chunks
         WHERE document_id = $1 AND superseded_at IS NULL ORDER BY seq",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 抽取用分块：文本 + 摄入阶段已算好的 embedding（消解 v2 复用，零额外 embed 调用）。
#[derive(Debug, sqlx::FromRow)]
pub struct ChunkForExtract {
    pub id: Uuid,
    pub seq: i32,
    pub text: String,
    pub embedding: Option<Vector>,
}

pub async fn chunks_for_extraction(
    pool: &PgPool,
    document_id: Uuid,
) -> AppResult<Vec<ChunkForExtract>> {
    // 只取未抽取的分块：认领的未变段落携带 extracted_at 跳过（增量抽取 + 断点续抽）
    let rows = sqlx::query_as(
        "SELECT id, seq, text, embedding FROM chunks
         WHERE document_id = $1 AND superseded_at IS NULL AND extracted_at IS NULL
         ORDER BY seq",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn set_graph_status(pool: &PgPool, id: Uuid, status: &str) -> AppResult<()> {
    sqlx::query("UPDATE documents SET graph_status = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

/// 该文档尚未 embedding 的分块（id + 文本）。
pub async fn chunks_pending_embedding(
    pool: &PgPool,
    document_id: Uuid,
) -> AppResult<Vec<(Uuid, String)>> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, text FROM chunks
         WHERE document_id = $1 AND embedding IS NULL AND superseded_at IS NULL ORDER BY seq",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn set_embeddings(pool: &PgPool, items: &[(Uuid, Vec<f32>)]) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    for (id, emb) in items {
        sqlx::query("UPDATE chunks SET embedding = $2 WHERE id = $1")
            .bind(id)
            .bind(Vector::from(emb.clone()))
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 向量近邻检索（余弦距离，顺扫；P1 规模足够）。
pub async fn vector_search(
    pool: &PgPool,
    kb_id: Uuid,
    embedding: &[f32],
    limit: i64,
) -> AppResult<Vec<Uuid>> {
    let query_vec = Vector::from(embedding.to_vec());
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM chunks
         WHERE kb_id = $1 AND embedding IS NOT NULL AND superseded_at IS NULL
           AND vector_dims(embedding) = vector_dims($2)
         ORDER BY embedding <=> $2
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(&query_vec)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 按 id 集合取分块（带文档名），保持传入顺序。
pub async fn chunks_by_ids(pool: &PgPool, kb_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<ChunkView>> {
    let rows: Vec<ChunkView> = sqlx::query_as(
        "SELECT c.id, c.document_id, c.seq, c.text, d.filename
         FROM chunks c JOIN documents d ON d.id = c.document_id
         WHERE c.kb_id = $1 AND c.id = ANY($2)",
    )
    .bind(kb_id)
    .bind(ids)
    .fetch_all(pool)
    .await?;
    // 恢复 RRF 排名顺序
    let mut by_id: std::collections::HashMap<Uuid, ChunkView> =
        rows.into_iter().map(|c| (c.id, c)).collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}
