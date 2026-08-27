//! 混合检索：BM25（Tantivy）+ 向量（pgvector）→ RRF 融合。
//! embedding 未配置或请求失败时静默降级为纯 BM25。

use crate::llm_util;
use crate::state::AppState;
use utopia_core::models::ChunkView;
use utopia_core::AppResult;
use uuid::Uuid;

const RECALL_PER_CHANNEL: usize = 24;

pub async fn hybrid(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    top_k: usize,
) -> AppResult<Vec<ChunkView>> {
    let mut lists: Vec<Vec<String>> = Vec::new();

    // BM25
    let bm25 = state
        .search
        .search(&kb_id.to_string(), query, RECALL_PER_CHANNEL)
        .map_err(utopia_core::AppError::Other)?;
    lists.push(bm25.into_iter().map(|h| h.chunk_id).collect());

    // 向量（可选通道）
    let settings = utopia_store::settings::get(&state.pool, workspace_id).await?;
    if let Some(client) = settings.as_ref().and_then(llm_util::embed_client) {
        match client.embed(&[query.to_string()]).await {
            Ok(mut embeddings) if !embeddings.is_empty() => {
                let ids = utopia_store::documents::vector_search(
                    &state.pool,
                    kb_id,
                    &embeddings.remove(0),
                    RECALL_PER_CHANNEL as i64,
                )
                .await?;
                lists.push(ids.into_iter().map(|id| id.to_string()).collect());
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "查询 embedding 失败，降级为纯 BM25"),
        }
    }

    let fused = utopia_search::rrf_fuse(&lists, top_k);
    let ids: Vec<Uuid> = fused.iter().filter_map(|s| s.parse().ok()).collect();
    utopia_store::documents::chunks_by_ids(&state.pool, kb_id, &ids).await
}
