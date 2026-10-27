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

/// 不认识的字段答 422：悄悄忽略一个拼错的过滤条件，调用方会以为它生效了
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
    /// 只检索本库这个来源的文档；别的库的来源答 404
    #[serde(default)]
    pub source_id: Option<Uuid>,
    /// 只检索 `external_id` 以它开头的文档。identity 是推送来源里的，所以要 `source_id`
    /// 且它是推送来源，否则 422
    #[serde(default)]
    pub external_id_prefix: Option<String>,
}

async fn source_filter(
    state: &AppState,
    kb_id: Uuid,
    req: &SearchReq,
) -> ApiResult<Option<retrieval::SourceFilter>> {
    let source = match req.source_id {
        Some(source_id) => {
            Some(super::sources_routes::source_in_kb(state, kb_id, source_id).await?)
        }
        None => None,
    };
    let Some(prefix) = &req.external_id_prefix else {
        return Ok(source.map(|source| retrieval::SourceFilter {
            source_id: source.id,
            key_prefix: None,
        }));
    };
    let Some(source) = source.filter(|source| crate::pushed_keys::is_push_kind(&source.kind))
    else {
        return Err(AppError::invalid(
            "prefix_needs_push_source",
            "external_id_prefix needs the source_id of an API or Statements source",
        )
        .into());
    };
    Ok(Some(retrieval::SourceFilter {
        source_id: source.id,
        key_prefix: Some(crate::pushed_keys::pushed_key(&source, prefix)),
    }))
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
        filter: source_filter(&state, kb_id, &req).await?,
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
