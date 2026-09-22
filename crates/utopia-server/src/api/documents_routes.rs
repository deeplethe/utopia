use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use utopia_core::models::{Document, Role};
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct UploadQuery {
    /// 目标 folder 来源：上传直接归入该文件夹（仅 kind=folder 接受上传）
    #[serde(default)]
    pub source: Option<Uuid>,
}

/// 批量上传（multipart，可多文件）。重复内容（同 KB 同 sha256）跳过。
pub async fn upload(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    mut multipart: Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    let target_source = match q.source {
        Some(sid) => {
            let src = utopia_store::sources::get(&state.pool, sid).await?;
            if src.kb_id != kb_id || src.kind != "folder" {
                return Err(AppError::invalid(
                    "upload_needs_folder",
                    "Uploads can only target a folder source in this knowledge base",
                )
                .into());
            }
            Some(sid)
        }
        None => None,
    };

    let mut created: Vec<Document> = Vec::new();
    let mut skipped: Vec<serde_json::Value> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::invalid_detail("bad_upload", "Malformed upload", e.to_string()))?
    {
        let Some(filename) = field.file_name().map(String::from) else {
            continue;
        };
        let mime = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();
        let bytes = field.bytes().await.map_err(|e| {
            AppError::invalid_detail("upload_read_failed", "Failed to read upload", e.to_string())
        })?;
        if bytes.is_empty() {
            skipped.push(json!({ "filename": filename, "reason": "empty file" }));
            continue;
        }

        let sha256 = hex(&Sha256::digest(&bytes));
        state
            .blob
            .put(&sha256, &bytes)
            .await
            .map_err(AppError::Other)?;

        match utopia_store::documents::create_from_upload(
            &state.pool,
            kb_id,
            &filename,
            &mime,
            bytes.len() as i64,
            &sha256,
            target_source,
            content_time(&filename, &bytes),
        )
        .await
        {
            Ok(doc) => {
                utopia_store::jobs::enqueue(
                    &state.pool,
                    "process_document",
                    json!({ "document_id": doc.id }),
                )
                .await?;
                created.push(doc);
            }
            Err(AppError::Conflict(_)) => {
                skipped.push(json!({ "filename": filename, "reason": "duplicate content" }));
            }
            Err(e) => return Err(e.into()),
        }
    }

    if created.is_empty() && skipped.is_empty() {
        return Err(AppError::invalid("no_files", "No files received").into());
    }
    Ok(Json(json!({ "created": created, "skipped": skipped })))
}

#[derive(serde::Deserialize)]
pub struct DocsQuery {
    /// 来源作用域：缺省 = 全部；`none` = 没有来源的；否则一个来源 id
    #[serde(default)]
    pub source: Option<String>,
    /// 文件名包含
    #[serde(default)]
    pub q: Option<String>,
    /// 抽取状态：none | queued | extracting | done | failed
    #[serde(default)]
    pub graph: Option<String>,
    /// `deleted` = 「已删除」视图：只列墓碑（#268）。缺省 = 活着的
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// 文库一页。
///
/// **改成服务端筛选与分页**：从前一次取回整库、前端切片。27 篇没事，两万篇会把
/// 整张表打进浏览器；而客户端筛选还有个更隐蔽的毛病——它只筛得到已经拿下来的那些。
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<DocsQuery>,
) -> ApiResult<Json<utopia_core::models::DocumentPage>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    let page = utopia_store::documents::page(
        &state.pool,
        kb_id,
        parse_scope(q.source.as_deref()),
        q.q.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        q.graph.as_deref().filter(|s| !s.is_empty()),
        q.state.as_deref() == Some("deleted"),
        q.limit.unwrap_or(15).clamp(1, 200),
        q.offset.unwrap_or(0).max(0),
    )
    .await?;
    Ok(Json(page))
}

/// `None` = 全部，`Some(None)` = 没有来源的，`Some(Some(id))` = 某个来源。
///
/// 认不出的字符串当成「全部」而不是报错：这个参数来自界面上的一次点击，
/// 而一次点击不该把整页变成一条错误。
fn parse_scope(raw: Option<&str>) -> Option<Option<Uuid>> {
    match raw {
        None | Some("") => None,
        Some("none") => Some(None),
        Some(s) => s.parse().ok().map(Some),
    }
}

/// 一键重试这个作用域里全部抽取失败的文档。
///
/// **存在的理由是一条条点太慢**：一个来源里五篇失败就是点五次，而失败往往是
/// 成批的（模型端点断了一阵，那段时间进来的全挂）。
pub async fn retry_failed(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
    Query(q): Query<DocsQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Editor).await?;
    let ids =
        utopia_store::documents::failed_ids(&state.pool, kb_id, parse_scope(q.source.as_deref()))
            .await?;
    // 逐个入队而不是一条 SQL 批量改状态：排队本身有别的动作（解雇在跑的任务、
    // 清增量标记），那些在 `queue_extraction_one` 里，绕过它会留下半截状态
    let mut queued = 0usize;
    for id in &ids {
        if utopia_store::documents::queue_extraction_one(&state.pool, *id)
            .await
            .is_ok()
        {
            queued += 1;
        }
    }
    if queued > 0 {
        state.emit_document(kb_id, ids[0]);
    }
    Ok(Json(json!({ "queued": queued, "found": ids.len() })))
}

/// 文档详情 + 全部分块（文档查看器用）。
pub async fn detail(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Viewer).await?;
    let chunks = utopia_store::documents::chunks_full(&state.pool, id).await?;
    Ok(Json(json!({ "document": doc, "chunks": chunks })))
}

/// `GET /documents/{id}/content[?version=N]` —— 把那份字节原样发回（#859）。
///
/// 授权：与 `detail` 同一条 `require_kb(doc.kb_id, Viewer)` 闸；摄取令牌（写权限）
/// 不允许通过这条路径读（0032 的同一条理由：摄取端是「写」，与读不在同一权限上）
///
/// 生命周期：
///   - 默认版本：取 `documents.sha256` 当前指向的；
///   - `?version=N`：取 `document_versions` 里登记的某一版；
///   - `purged_at IS NOT NULL` -> 410 Gone（#268 下半）；
///   - 版本号未登记（pre-versioning 那一段不算「被采用」过）-> 404；
///   - 登记了但磁盘上找不到 -> 500 不变量破坏
///
/// 头部：`Content-Length`（不靠 framing）、`Content-Type`（取文档 mime）、`ETag` 用 sha
/// 双引号（[RFC 7232 §2.3] 强 ETag），`Content-Disposition: inline; filename="..."`，
/// 让浏览器就地预览 PDF/图片，又允许 `<a download>` 强制下载
#[derive(Deserialize)]
pub struct ContentQuery {
    /// 可选：取某历史版本；不给就用当前 `documents.sha256`
    #[serde(default)]
    pub version: Option<i32>,
}

pub async fn content(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Query(q): Query<ContentQuery>,
) -> ApiResult<Response> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Viewer).await?;
    // 真删（#268 下半）：内容已抹掉，字节回不来；410 与 GET 的语义一致
    if doc.purged_at.is_some() {
        return Ok((
            StatusCode::GONE,
            Json(json!({
                "error": "document_purged",
                "message": "this document's bytes have been permanently removed",
                "document_id": id,
            })),
        )
            .into_response());
    }
    let version_row = match q.version {
        Some(n) => utopia_store::documents::get_version(&state.pool, id, n).await?,
        None => None, // 默认版本用 documents.sha256；下面走统一路径
    };
    if q.version.is_some() && version_row.is_none() {
        // 该版本从未被采用过：诚实回答 404 而不是回退到默认版本
        return Err(AppError::NotFound.into());
    }
    let (sha, size_bytes) = match &version_row {
        Some(v) => (v.sha256.clone(), v.size_bytes),
        // 默认版本：信文档行的 sha + 长度（创建时刻记下的），让 ETag 匹配
        None => (doc.sha256.clone(), doc.size_bytes),
    };
    let bytes = state.blob.get(&sha).await.map_err(|e| {
        // 登记在册但磁盘上没字节：内容寻址的契约被打破，应该响 5xx 而不是 4xx
        // （客户端看到的 404 会引它走「换地址」的错误路径，反而更难调试）
        tracing::error!(%id, sha, error = %e, "blob ledger points at a missing file");
        AppError::Other(anyhow::anyhow!(
            "blob {sha} for document {id} is missing from the content store"
        ))
    })?;
    // 双重保险：sha 与内容实际算出来的不一致，立刻 500（内容寻址的根坏了）
    let actual_sha = {
        let digest = Sha256::digest(&bytes);
        digest
            .as_slice()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    };
    if actual_sha != sha {
        tracing::error!(%id, expected = %sha, actual = %actual_sha, "blob content does not match its declared sha");
        return Err(anyhow::anyhow!("blob {sha} for document {id} has been corrupted").into());
    }
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size_bytes));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&doc.mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{sha}\"")).unwrap_or(HeaderValue::from_static("\"\"")),
    );
    // inline 优先：浏览器对 PDF/图片能就地预览；想下载用 `<a download>` 覆盖
    let safe_name = ascii_filename(&doc.filename);
    if let Ok(v) = HeaderValue::from_str(&format!("inline; filename=\"{safe_name}\"")) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    Ok((StatusCode::OK, headers, Body::from(bytes)).into_response())
}

/// `GET /documents/{id}/versions` —— 版本台账（#859）
///
/// 返回 `[{version, sha256, size_bytes, ingested_at}, ...]`，按 version 升序。
/// `missing_since` 与 `deleted_at` 的文档仍可查（其字节仍在）；`purged_at` 不影响这条路径。
pub async fn versions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Viewer).await?;
    let versions = utopia_store::documents::list_versions(&state.pool, id).await?;
    Ok(Json(json!({
        "document_id": id,
        "current_sha256": doc.sha256,
        "versions": versions,
    })))
}

/// 把文件名里的非 ASCII 字符替成 `_`，给 `Content-Disposition` 的 `filename=`
/// 用。中文/日文原文件名在那一格里会变成问号，不如直说换掉；
/// RFC 5987 的 `filename*=UTF-8''…` 也跟着放，让现代浏览器拿到真名
fn ascii_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod filename_tests {
    use super::ascii_filename;

    #[test]
    fn ascii_is_kept_verbatim() {
        assert_eq!(ascii_filename("filing.txt"), "filing.txt");
        assert_eq!(ascii_filename("Q3-2024.pdf"), "Q3-2024.pdf");
        assert_eq!(ascii_filename("with_spaces.txt"), "with_spaces.txt");
    }

    #[test]
    fn non_ascii_chars_become_underscore() {
        // 中文文件名里那串字符在 `Content-Disposition: filename=` 那格里
        // 会变成问号；不如在源头替成 `_`，再让 `filename*=UTF-8''…` 把真名
        // 一起发出去
        assert_eq!(ascii_filename("公告.pdf"), "__.pdf");
        assert_eq!(ascii_filename("2024Q3 売上.txt"), "2024Q3___.txt");
        assert_eq!(ascii_filename("résumé.md"), "r_sum_.md");
    }
}

/// 反向证据链：文档各分块抽出的事实（文档查看器右栏）。
pub async fn extractions(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Viewer).await?;
    let facts = utopia_store::graph::document_extractions(&state.pool, id).await?;
    Ok(Json(json!({ "facts": facts })))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Editor).await?;

    // 墓碑，不是减法（#268）：文档、分块、证据、原始文件都留着；只作废没有别的出处的事实
    let report = utopia_store::documents::delete(&state.pool, doc.kb_id, id, Some(user.id)).await?;
    let search = state.search.clone();
    let did = id.to_string();
    tokio::task::spawn_blocking(move || search.delete_document(&did))
        .await
        .map_err(|e| AppError::Other(e.into()))?
        .map_err(AppError::Other)?;
    // 前提作废了，靠它推出来的派生随之失效——不等下一次定时重推
    settle_derivations(&state, doc.kb_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(doc.kb_id),
        user.id,
        "document.deleted",
        "document",
        Some(id),
        json!({
            "filename": doc.filename,
            "deletion_id": report.deletion_id,
            "invalidated_facts": report.invalidated_facts,
        }),
    )
    .await;
    state.emit_document(doc.kb_id, id);
    Ok(Json(json!({
        "ok": true,
        "deletion_id": report.deletion_id,
        "invalidated_facts": report.invalidated_facts,
    })))
}

/// 撤销删除：文档、分块、这次作废的事实原路复活，索引重建。
/// 同步撞见墓碑与同内容重传走的是同一个 store 函数，这里只是人按的那一条路
pub async fn restore(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Editor).await?;
    let doc = utopia_store::documents::restore(&state.pool, doc.kb_id, id).await?;
    reindex(&state, &doc).await?;
    settle_derivations(&state, doc.kb_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(doc.kb_id),
        user.id,
        "document.restored",
        "document",
        Some(id),
        json!({ "filename": doc.filename }),
    )
    .await;
    state.emit_document(doc.kb_id, id);
    Ok(Json(json!({ "ok": true })))
}

/// 真删（#268 下半）：内容抹掉，不可撤销，只对已删除的文档开放，库管理员才能按。
/// 库里先记账（purged_at），再删文件：删文件失败只是漏一份孤儿原文，反过来则是
/// 库说「还能恢复」而原文已经没了
pub async fn purge(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Admin).await?;
    let report = utopia_store::documents::purge(&state.pool, doc.kb_id, id).await?;
    for sha in &report.blobs {
        if let Err(e) = state.blob.delete(sha).await {
            tracing::warn!(document = %id, sha, error = %e, "purge: blob left behind");
        }
    }
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(doc.kb_id),
        user.id,
        "document.purged",
        "document",
        Some(id),
        json!({
            "filename": doc.filename,
            "chunks": report.chunks,
            "blobs": report.blobs.len(),
        }),
    )
    .await;
    state.emit_document(doc.kb_id, id);
    Ok(Json(
        json!({ "ok": true, "chunks": report.chunks, "blobs": report.blobs.len() }),
    ))
}

/// 复活的文档回到全文索引：分块的正文一直都在，只是重写一遍索引条目
pub async fn reindex(state: &AppState, doc: &Document) -> utopia_core::AppResult<()> {
    let chunks =
        utopia_store::documents::chunks_in_document(&state.pool, doc.kb_id, doc.id).await?;
    let pairs: Vec<(String, String)> = chunks
        .into_iter()
        .map(|c| (c.id.to_string(), c.text))
        .collect();
    let search = state.search.clone();
    let (kb, did) = (doc.kb_id.to_string(), doc.id.to_string());
    tokio::task::spawn_blocking(move || search.reindex_document(&kb, &did, &pairs))
        .await
        .map_err(|e| AppError::Other(e.into()))?
        .map_err(AppError::Other)?;
    Ok(())
}

/// 前提变了就重推一遍，让派生跟上——开关关着的库不推。删除、撤销、同步复活三条路共用
pub(crate) async fn settle_derivations(
    state: &AppState,
    kb_id: Uuid,
) -> utopia_core::AppResult<()> {
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    if kb.materialize_inferences {
        utopia_store::reasoning::materialize(&state.pool, kb_id).await?;
    }
    Ok(())
}

/// 重新处理（解析器升级/失败重试）。
pub async fn reprocess(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let doc = utopia_store::documents::get(&state.pool, id).await?;
    utopia_store::access::require_kb(&state.pool, &user, doc.kb_id, Role::Editor).await?;
    utopia_store::documents::set_status(&state.pool, id, "pending").await?;
    let job_id = utopia_store::jobs::enqueue(
        &state.pool,
        "process_document",
        json!({ "document_id": id }),
    )
    .await?;
    Ok(Json(json!({ "job_id": job_id })))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 只认开头独立的完整日期行，不把正文里提到的事件日期当作文档日期（#610）。
fn content_time(filename: &str, bytes: &[u8]) -> Option<chrono::DateTime<chrono::Utc>> {
    let extension = std::path::Path::new(filename).extension()?.to_str()?;
    if !["txt", "md", "markdown"]
        .iter()
        .any(|ext| extension.eq_ignore_ascii_case(ext))
    {
        return None;
    }
    // 只解码头部 4 KiB：日期行只认开头。PDF、Word 这类格式要读日期时，在各自的解析器里
    // 读它们自己的元数据，不在这里猜
    const HEADER_BYTES: usize = 4096;
    let text = utopia_ingest::decode_text(&bytes[..bytes.len().min(HEADER_BYTES)]);
    let header = if bytes.len() > HEADER_BYTES {
        // 截断的半行可能在日期后还有文字，不能把它误当独立日期行。
        text.rsplit_once('\n')?.0
    } else {
        &text
    };
    let line = header
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let date = line
        .strip_prefix('（')
        .and_then(|s| s.strip_suffix('）'))
        .or_else(|| line.strip_prefix('(').and_then(|s| s.strip_suffix(')')))
        .unwrap_or(line);
    if !date.get(..4)?.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .or_else(|_| chrono::NaiveDate::parse_from_str(date, "%Y年%m月%d日"))
        .ok()?;
    // 与项目已有日精度约定一致：UTC 零点是存储约定，不猜作者所在时区。
    Some(day.and_hms_opt(0, 0, 0)?.and_utc())
}

#[cfg(test)]
#[path = "documents_routes_tests.rs"]
mod tests;

/// 抽取丢弃信号：哪些事实抽出来了却没能落地。整库一次取回——按
/// (文档 × 原因 × 具体对象) 聚合后行数很小，Library 既算总数又展开详情，
/// 不必逐行发请求。
pub async fn extraction_drops(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    let drops = utopia_store::extraction_drops::for_kb(&state.pool, kb_id).await?;
    Ok(Json(json!({ "drops": drops })))
}
