use axum::extract::{Multipart, Path, Query, State};
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

        match utopia_store::documents::create(
            &state.pool,
            kb_id,
            &filename,
            &mime,
            bytes.len() as i64,
            &sha256,
            target_source,
            None,
            None,
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

    utopia_store::documents::delete(&state.pool, id).await?;
    let search = state.search.clone();
    let did = id.to_string();
    tokio::task::spawn_blocking(move || search.delete_document(&did))
        .await
        .map_err(|e| AppError::Other(e.into()))?
        .map_err(AppError::Other)?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(doc.kb_id),
        user.id,
        "document.deleted",
        "document",
        Some(id),
        json!({ "filename": doc.filename }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
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
