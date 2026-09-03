use pgvector::Vector;
use sqlx::PgPool;
use utopia_core::models::{ChunkView, Document, DocumentPage};
use utopia_core::{AppError, AppResult};
use utopia_ingest::ChunkPiece;
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
    // 同样的内容回来了，而它只是被删过：复活那一篇，而不是撞 (kb_id, sha256) 的唯一
    // 索引报「已存在」。撤销删除的一种自然形态——重传就是「我要它回来」（#268）
    if let Some((id,)) = sqlx::query_as::<_, (Uuid,)>(
        "SELECT id FROM documents WHERE kb_id = $1 AND sha256 = $2 AND deleted_at IS NOT NULL",
    )
    .bind(kb_id)
    .bind(sha256)
    .fetch_optional(pool)
    .await?
    {
        return restore(pool, kb_id, id).await;
    }
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
    .bind(if doc_time.is_some() {
        "source"
    } else {
        "upload_time"
    })
    .bind(external_key)
    .fetch_one(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => AppError::Conflict(format!(
            "File already exists (identical content): {filename}"
        )),
        _ => AppError::Db(e),
    })
}

pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Document>> {
    let rows = sqlx::query_as(
        "SELECT * FROM documents WHERE kb_id = $1 AND deleted_at IS NULL ORDER BY created_at DESC",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 文库一页，**带筛选与统计**。
///
/// 从前是 `SELECT * FROM documents WHERE kb_id = $1` 不带上限、前端客户端分页。
/// 27 篇没事，两万篇会把整张表打进浏览器；而客户端筛选还有个更隐蔽的毛病——
/// 它只筛得到已经拿下来的那些。
///
/// 统计单独算，而且**只按来源作用域算，不受名字/状态筛选影响**：
/// 「这个来源里有几篇可抽」是那两个批量按钮的作用范围，跟你此刻在搜什么无关。
#[allow(clippy::too_many_arguments)]
pub async fn page(
    pool: &PgPool,
    kb_id: Uuid,
    // None = 全部；Some(None) = 只看没有来源的；Some(Some(id)) = 某个来源
    source: Option<Option<Uuid>>,
    q: Option<&str>,
    graph_status: Option<&str>,
    limit: i64,
    offset: i64,
) -> AppResult<DocumentPage> {
    // 三个筛选都写成「参数为空就不生效」，一条 SQL 覆盖全部组合。
    // `$2 = 'any'` 那一支是「不按来源筛」，`'none'` 是「只看没有来源的」——
    // 用两个哨兵字符串而不是两个可空参数，因为 NULL 在这里有歧义：
    // 它既可能是「不筛」，也可能是「筛出 source_id IS NULL 的」
    const WHERE: &str = "WHERE kb_id = $1 AND deleted_at IS NULL
           AND ($2 = 'any'
                OR ($2 = 'none' AND source_id IS NULL)
                OR source_id::text = $2)
           AND ($3::text IS NULL OR filename ILIKE '%' || $3 || '%')
           AND ($4::text IS NULL OR graph_status = $4)";
    let scope = match source {
        None => "any".to_string(),
        Some(None) => "none".to_string(),
        Some(Some(id)) => id.to_string(),
    };

    let docs: Vec<Document> = sqlx::query_as(&format!(
        "SELECT * FROM documents {WHERE} ORDER BY created_at DESC LIMIT $5 OFFSET $6"
    ))
    .bind(kb_id)
    .bind(&scope)
    .bind(q)
    .bind(graph_status)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    let (total,): (i64,) = sqlx::query_as(&format!("SELECT count(*) FROM documents {WHERE}"))
        .bind(kb_id)
        .bind(&scope)
        .bind(q)
        .bind(graph_status)
        .fetch_one(pool)
        .await?;

    // 统计只按来源作用域算：那两个批量按钮作用于整个来源，不是你搜出来的那几条
    let stats: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           count(*) FILTER (WHERE status = 'ready'),
           count(*) FILTER (WHERE graph_status IN ('queued', 'extracting')),
           count(*) FILTER (WHERE graph_status = 'failed')
         FROM documents
         WHERE kb_id = $1 AND deleted_at IS NULL
           AND ($2 = 'any'
                OR ($2 = 'none' AND source_id IS NULL)
                OR source_id::text = $2)",
    )
    .bind(kb_id)
    .bind(&scope)
    .fetch_one(pool)
    .await?;

    Ok(DocumentPage {
        docs,
        total,
        ready: stats.0,
        extracting: stats.1,
        failed: stats.2,
    })
}

/// 这个来源（或整库）里抽取失败的文档 id。**一键重试要的就是这份名单**。
pub async fn failed_ids(
    pool: &PgPool,
    kb_id: Uuid,
    source: Option<Option<Uuid>>,
) -> AppResult<Vec<Uuid>> {
    let scope = match source {
        None => "any".to_string(),
        Some(None) => "none".to_string(),
        Some(Some(id)) => id.to_string(),
    };
    Ok(sqlx::query_scalar(
        "SELECT id FROM documents
          WHERE kb_id = $1 AND deleted_at IS NULL AND graph_status = 'failed'
            AND ($2 = 'any'
                 OR ($2 = 'none' AND source_id IS NULL)
                 OR source_id::text = $2)",
    )
    .bind(kb_id)
    .bind(&scope)
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &PgPool, id: Uuid) -> AppResult<Document> {
    sqlx::query_as("SELECT * FROM documents WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// 按 kb 收窄的取文档。**id 由模型给出时只能走这一支**：`get` 只按 id 查，
/// 一个别的库的 id 照样查得到。
pub async fn find_in_kb(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<Option<Document>> {
    Ok(sqlx::query_as(
        "SELECT * FROM documents WHERE id = $1 AND kb_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?)
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
        "SELECT * FROM documents
              WHERE source_id = $1 AND sha256 = $2 AND deleted_at IS NULL LIMIT 1",
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
         WHERE source_id = $1 AND external_key IS NULL AND filename = $2
           AND deleted_at IS NULL LIMIT 1",
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
pub async fn clear_missing_keys(pool: &PgPool, source_id: Uuid, keys: &[String]) -> AppResult<()> {
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
pub async fn mark_missing_keys(pool: &PgPool, source_id: Uuid, keys: &[String]) -> AppResult<u64> {
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
        "SELECT id FROM documents
          WHERE source_id = $1 AND missing_since IS NOT NULL AND deleted_at IS NULL",
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

/// 一次删除的产出，给审计与界面提示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionReport {
    pub deletion_id: Uuid,
    /// 随之作废的事实：只有「每条出处都已删除」的那些
    pub invalidated_facts: usize,
    pub superseded_chunks: usize,
}

/// 删除一篇文档——**认知轴上的一个事件，不是减法**（#268）。
///
/// 从前这里一句 `DELETE`，外键把分块与证据一并级联掉：事实留在图里、活着、却没了
/// 出处，而隐私政策写着「事实连同其出处保留」。现在文档打墓碑，分块走 `replace_chunks`
/// 用了很久的 `superseded_at`，**什么内容都不清**——原始文件本来也没删过；只作废
/// 「每条出处都已删除」的事实。判据看 `documents.deleted_at` 而不是分块的
/// `superseded_at`：证据停在文档**旧版本**上的事实是 stale，按 `stale_facts` 的规矩
/// 「没再提 ≠ 不成立」，交给人、不作废；只有源本身没了才作废。
///
/// 作废的事实与打标的分块记进 `document_deletions`，[`restore`] 原路读回。
/// `actor` 为 None = 引擎（来源对账的批量清理）
pub async fn delete(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    actor: Option<Uuid>,
) -> AppResult<DeletionReport> {
    let mut tx = pool.begin().await?;
    let hit = sqlx::query(
        "UPDATE documents SET deleted_at = now(), updated_at = now()
          WHERE id = $1 AND kb_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(kb_id)
    .execute(&mut *tx)
    .await?;
    if hit.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    let chunks: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE chunks SET superseded_at = now()
          WHERE document_id = $1 AND superseded_at IS NULL RETURNING id",
    )
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    // 先打了墓碑再算：这篇文档此刻已经算「已删除」，所以只剩它作出处的事实才落网；
    // 另一篇活着的文档里也有证据的一条不动——删一份重复上传不该掀掉半张图
    let facts: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE facts f SET invalidated_at = now()
          WHERE f.kb_id = $1 AND f.invalidated_at IS NULL
            AND EXISTS (SELECT 1 FROM fact_evidence fe
                        JOIN chunks c ON c.id = fe.chunk_id
                        WHERE fe.fact_id = f.id AND c.document_id = $2)
            AND NOT EXISTS (SELECT 1 FROM fact_evidence fe
                            JOIN chunks c ON c.id = fe.chunk_id
                            JOIN documents d ON d.id = c.document_id
                            WHERE fe.fact_id = f.id AND d.deleted_at IS NULL)
          RETURNING f.id",
    )
    .bind(kb_id)
    .bind(id)
    .fetch_all(&mut *tx)
    .await?;
    let chunk_ids: Vec<Uuid> = chunks.into_iter().map(|(c,)| c).collect();
    let fact_ids: Vec<Uuid> = facts.into_iter().map(|(f,)| f).collect();
    let deletion_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO document_deletions
            (id, kb_id, document_id, deleted_by, invalidated_facts, superseded_chunks)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(deletion_id)
    .bind(kb_id)
    .bind(id)
    .bind(actor)
    .bind(&fact_ids)
    .bind(&chunk_ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(DeletionReport {
        deletion_id,
        invalidated_facts: fact_ids.len(),
        superseded_chunks: chunk_ids.len(),
    })
}

/// 撤销一次删除：文档、这次打标的分块、这次作废的事实原路复活，形状照 `revert_merge`。
///
/// 只救 `document_deletions` 名单上的——更早版本的旧分块、删除之前就作废的事实
/// 都不在名单里。三条路都从这里走：人点撤销、同步撞见墓碑、同内容重传
pub async fn restore(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<Document> {
    let row: Option<(Uuid, Vec<Uuid>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT id, invalidated_facts, superseded_chunks FROM document_deletions
          WHERE document_id = $1 AND kb_id = $2 AND reverted_at IS NULL
          ORDER BY created_at DESC LIMIT 1",
    )
    .bind(id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    let Some((deletion_id, fact_ids, chunk_ids)) = row else {
        return Err(AppError::Conflict("Document is not deleted".into()));
    };
    let mut tx = pool.begin().await?;
    let hit = sqlx::query(
        "UPDATE documents SET deleted_at = NULL, updated_at = now()
          WHERE id = $1 AND kb_id = $2 AND deleted_at IS NOT NULL",
    )
    .bind(id)
    .bind(kb_id)
    .execute(&mut *tx)
    .await?;
    if hit.rows_affected() == 0 {
        return Err(AppError::Conflict("Document is not deleted".into()));
    }
    sqlx::query("UPDATE chunks SET superseded_at = NULL WHERE id = ANY($1)")
        .bind(&chunk_ids)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE facts SET invalidated_at = NULL
          WHERE id = ANY($1) AND invalidated_at IS NOT NULL",
    )
    .bind(&fact_ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE document_deletions SET reverted_at = now() WHERE id = $1")
        .bind(deletion_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    get(pool, id).await
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

/// 单篇排队重抽（手动 Extract = 强制全量）：清增量标记、解雇在跑的任务、置
/// queued、建抽取任务——与 `queue_extraction` 同一套语义，只是作用于一篇。返回 job id。
///
/// 整体一个事务。分开做时任一步失败都会留下半截状态：清了标记却没换 epoch，
/// 旧任务察觉不到自己已被顶替；或者置了 queued 却没建成任务，文档就此无人接手。
pub async fn queue_extraction_one(pool: &PgPool, document_id: Uuid) -> AppResult<i64> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE chunks SET extracted_at = NULL
         WHERE document_id = $1 AND superseded_at IS NULL",
    )
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    // 未开跑的旧任务顺手删掉：连点两次 Extract 不该攒出两个任务同抽一篇
    sqlx::query(
        "DELETE FROM jobs WHERE kind = 'extract_document' AND status = 'queued'
           AND payload->>'document_id' = $1",
    )
    .bind(document_id.to_string())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE documents SET graph_status = 'queued', graph_error = NULL,
                extract_epoch = extract_epoch + 1
         WHERE id = $1",
    )
    .bind(document_id)
    .execute(&mut *tx)
    .await?;
    let (job_id,): (i64,) = sqlx::query_as(
        "INSERT INTO jobs (kind, payload)
         VALUES ('extract_document', jsonb_build_object('document_id', $1::text))
         RETURNING id",
    )
    .bind(document_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(job_id)
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

/// 推进抽取状态（顺带清空上一轮的失败原因——重跑即翻篇）。
pub async fn set_graph_status(pool: &PgPool, id: Uuid, status: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET graph_status = $2, graph_error = NULL, updated_at = now()
         WHERE id = $1",
    )
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
         WHERE c.kb_id = $1 AND c.id = ANY($2)
           AND c.superseded_at IS NULL AND d.deleted_at IS NULL",
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

/// 一篇文档的现行分块（带文档名），按 seq 排。kb_id 进 WHERE 而不是查回来再比。
pub async fn chunks_in_document(
    pool: &PgPool,
    kb_id: Uuid,
    document_id: Uuid,
) -> AppResult<Vec<ChunkView>> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.document_id, c.seq, c.text, d.filename
         FROM chunks c JOIN documents d ON d.id = c.document_id
         WHERE c.kb_id = $1 AND c.document_id = $2 AND c.superseded_at IS NULL
         ORDER BY c.seq",
    )
    .bind(kb_id)
    .bind(document_id)
    .fetch_all(pool)
    .await?)
}

/// 批量排队全量重抽：ready 文档清增量标记 → graph_status=queued → 建抽取任务，
/// 返回待抽文档 id。`source_id` 给定则限定该来源，否则整库。
///
/// 正在抽取的文档一并重排——epoch 自增即"解雇"在跑的那个任务（见
/// `extract_epoch`），不必跳过、也不会两个 worker 同抽一篇。
/// 尚未开跑的 extract 任务顺手删掉（payload 用文本比较：历史脏 payload 无法转 uuid）。
///
/// 建任务与置状态同事务：分开做时，中途出错会留下一批 graph_status=queued
/// 却没有任务的文档——不会有 worker 来接，界面上永远停在"排队中"。
pub async fn queue_extraction(
    pool: &PgPool,
    kb_id: Uuid,
    source_id: Option<Uuid>,
) -> AppResult<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    let ids: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM documents
         WHERE kb_id = $1 AND deleted_at IS NULL AND status = 'ready'
           AND ($2::uuid IS NULL OR source_id = $2)
         ORDER BY created_at",
    )
    .bind(kb_id)
    .bind(source_id)
    .fetch_all(&mut *tx)
    .await?;
    let ids: Vec<Uuid> = ids.into_iter().map(|(id,)| id).collect();
    if ids.is_empty() {
        tx.commit().await?;
        return Ok(ids);
    }

    sqlx::query(
        "DELETE FROM jobs WHERE kind = 'extract_document' AND status = 'queued'
           AND payload->>'document_id' = ANY($1)",
    )
    .bind(ids.iter().map(|i| i.to_string()).collect::<Vec<_>>())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE chunks SET extracted_at = NULL
         WHERE document_id = ANY($1) AND superseded_at IS NULL",
    )
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE documents SET graph_status = 'queued', graph_error = NULL,
                extract_epoch = extract_epoch + 1
         WHERE id = ANY($1)",
    )
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    // payload 形状与 jobs::enqueue(json!({"document_id": id})) 一致：uuid 序列化为字符串
    sqlx::query(
        "INSERT INTO jobs (kind, payload)
         SELECT 'extract_document', jsonb_build_object('document_id', id::text)
         FROM unnest($1::uuid[]) AS t(id)",
    )
    .bind(&ids)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ids)
}

/// 抽取失败：状态与原因一起落库，界面才有东西可展示。
pub async fn set_graph_failed(pool: &PgPool, id: Uuid, error: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE documents SET graph_status = 'failed', graph_error = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// 抽取任务的所有权凭证：每次"开始新一轮抽取"自增。
///
/// 单靠 graph_status 认领无效——旧任务回读时，接手的新任务可能已把状态写回
/// extracting，旧任务会误判自己仍在岗。epoch 单调递增，旧任务一比即知已被接管。
pub async fn extract_epoch(pool: &PgPool, id: Uuid) -> AppResult<i32> {
    let (epoch,): (i32,) = sqlx::query_as("SELECT extract_epoch FROM documents WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(epoch)
}

/// 这个库还有没有在排队或正在跑的抽取。
///
/// 冷启动自动扩本体要等一批文档都抽完再动手：只看第一篇的话，
/// 先到的那篇的词汇会独占本体。最后一篇跑完的任务负责触发。
pub async fn extraction_idle(pool: &PgPool, kb_id: Uuid) -> AppResult<bool> {
    let (pending,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM documents
         WHERE kb_id = $1 AND deleted_at IS NULL AND graph_status IN ('queued', 'extracting')",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    Ok(pending == 0)
}

/// 全库分块，按文档分组，供检索索引重建用。
///
/// **一次全取**：这条只在启动发现索引落空时跑，那时候要的正是全部；
/// 而它跑完之后就再也不跑了。
pub async fn all_chunks_for_index(pool: &PgPool) -> AppResult<Vec<(Uuid, Uuid, Uuid, String)>> {
    Ok(sqlx::query_as(
        "SELECT kb_id, document_id, id, text FROM chunks
          WHERE superseded_at IS NULL
          ORDER BY document_id, seq",
    )
    .fetch_all(pool)
    .await?)
}

/// 库里一共有多少条在用的分块。启动时拿它跟索引对账。
pub async fn live_chunk_count(pool: &PgPool) -> AppResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM chunks WHERE superseded_at IS NULL")
            .fetch_one(pool)
            .await?,
    )
}
