//! 混合检索：BM25（Tantivy）+ 向量（pgvector）→ RRF 融合。
//! embedding 未配置或请求失败时静默降级为纯 BM25。
//!
//! **`as_of` 只在向量与取块这两处是完整的**（0019 开放问题②）。Tantivy 的索引
//! 只有"现在"一个版本：重解析会把文档的块整批换掉，旧版本不在索引里。所以
//! 带时刻检索时，BM25 找回来的东西**是对的**（随后按当时的活性过滤），但它
//! 找不回当时有、如今已被顶掉的那些块——召回缺一角，命中不会出错。
//! 要补那一角得给全文索引也建版本，那是另一件事，不在这一刀里。
//!
//! **两路并行，BM25 在阻塞线程池上跑**（#515）。`SearchIndex::search` 是同步的：
//! 切词、走 mmap 段、逐条取文档，跑多久就占一条 runtime 线程多久；仓库里另外六处
//! Tantivy 调用都包了 `spawn_blocking`，这里是每次提问都走的最热一处，从前没包，
//! 两核机器上两个人同时提问就把 SSE、健康检查一起卡住。两路从前还串着：BM25 跑完
//! 才去调嵌入端点，嵌入回来才查向量，而嵌入是一次网络往返、整个函数最贵的一项，
//! 跟 BM25 毫无依赖。并起来之后延迟是两路取大，不是相加。

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
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Vec<ChunkView>> {
    let bm25 = {
        let search = state.search.clone();
        let kb = kb_id.to_string();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || search.search(&kb, &query, RECALL_PER_CHANNEL))
    };
    // `join!` 不是 `try_join!`：向量那一路失败是降级不是错误（见 vector_channel），
    // `try_join!` 会把模型故障变成检索报错
    let (bm25, vector) = tokio::join!(
        bm25,
        vector_channel(state, kb_id, workspace_id, query, as_of)
    );
    let bm25 = bm25
        .map_err(|e| utopia_core::AppError::Other(anyhow::anyhow!("BM25 检索线程退出：{e}")))?
        .map_err(utopia_core::AppError::Other)?;
    let lists = channel_lists(bm25.into_iter().map(|h| h.chunk_id).collect(), vector?);

    // Keep the bounded channel candidates until their record-time and document
    // filters have run: newer hits must not consume a historical query's limit.
    let candidate_limit = lists.iter().map(Vec::len).sum();
    let fused = utopia_search::rrf_fuse(&lists, candidate_limit);
    let ids: Vec<Uuid> = fused.iter().filter_map(|s| s.parse().ok()).collect();
    let mut chunks =
        utopia_store::documents::chunks_by_ids(&state.pool, kb_id, &ids, as_of).await?;
    chunks.truncate(top_k);
    Ok(chunks)
}

/// 向量那一路。没配嵌入模型、嵌入请求失败、回了空，都是 `Ok(None)`：检索照常，
/// 只剩 BM25——这就是模块头说的静默降级。向量查询本身失败仍是 `Err`：那是库的事，
/// 不是模型的事，吞掉会把坏库伪装成没配模型。
async fn vector_channel(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
) -> AppResult<Option<Vec<String>>> {
    let settings = utopia_store::settings::get(&state.pool, workspace_id).await?;
    let Some(client) = settings.as_ref().and_then(llm_util::embed_client) else {
        return Ok(None);
    };
    let mut embeddings = match client.embed(&[query.to_string()]).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "查询 embedding 失败，降级为纯 BM25");
            return Ok(None);
        }
    };
    if embeddings.is_empty() {
        return Ok(None);
    }
    let ids = utopia_store::documents::vector_search(
        &state.pool,
        kb_id,
        &embeddings.remove(0),
        RECALL_PER_CHANNEL as i64,
        as_of,
    )
    .await?;
    Ok(Some(ids.into_iter().map(|id| id.to_string()).collect()))
}

/// 送进 RRF 的列表：BM25 恒在第一位，向量有就在第二位。顺序钉死、不看谁先回来——
/// 今天的 RRF 不看顺序，将来若给通道加权就看了，不能让它取决于哪路网络快。
fn channel_lists(bm25: Vec<String>, vector: Option<Vec<String>>) -> Vec<Vec<String>> {
    let mut lists = vec![bm25];
    if let Some(vector) = vector {
        lists.push(vector);
    }
    lists
}

#[cfg(test)]
mod tests {
    use super::channel_lists;

    #[test]
    fn the_channels_keep_their_places_whichever_answered_first() {
        let lists = channel_lists(vec!["b".into()], Some(vec!["v".into()]));
        assert_eq!(lists, vec![vec!["b".to_string()], vec!["v".to_string()]]);
    }

    #[test]
    fn a_missing_vector_channel_leaves_bm25_alone() {
        let lists = channel_lists(vec!["b".into()], None);
        assert_eq!(lists, vec![vec!["b".to_string()]]);
    }
}

#[cfg(test)]
#[path = "retrieval_tests.rs"]
mod retrieval_tests;
