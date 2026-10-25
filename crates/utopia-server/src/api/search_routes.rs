use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::SessionOrToken;
use crate::error::ApiResult;
use crate::retrieval;
use crate::state::AppState;

/// 不认识的字段答 422：过滤条件还没做，悄悄忽略一个 `filters` 会让调用方以为它生效了
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchReq {
    pub q: String,
    #[serde(default)]
    pub top_k: Option<usize>,
    #[serde(default)]
    pub offset: usize,
    /// 记录轴（0019）：按**那一刻库里有的东西**检索。YYYY-MM-DD 或 RFC3339。
    /// 命中的是当时活着的块、当时还没被删的文档
    #[serde(default)]
    pub as_of: Option<String>,
}

pub async fn search(
    State(state): State<AppState>,
    reader: SessionOrToken,
    Path(kb_id): Path<Uuid>,
    Json(req): Json<SearchReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let q = req.q.trim();
    if q.is_empty() {
        return Err(AppError::invalid("empty_query", "Search query cannot be empty").into());
    }
    let kb = super::documents_routes::require_reader_kb(&state, &reader, kb_id).await?;

    let top_k = req.top_k.unwrap_or(10).min(50);
    if req.offset.saturating_add(top_k) > retrieval::MAX_DEPTH {
        return Err(AppError::invalid(
            "page_too_deep",
            format!("offset + top_k may not exceed {}", retrieval::MAX_DEPTH),
        )
        .into());
    }
    let as_of = super::graph_routes::parse_instant("as_of", req.as_of.as_deref())?;
    let opts = retrieval::SearchOpts {
        top_k,
        offset: req.offset,
        as_of,
    };
    let page = retrieval::hybrid_scored(&state, kb_id, kb.workspace_id, q, opts, None).await?;
    Ok(Json(json!({
        "results": page.results,
        "offset": req.offset,
        "top_k": top_k,
        "has_more": page.has_more,
        "vector_used": page.vector_used,
    })))
}
