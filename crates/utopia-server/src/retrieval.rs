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
use serde::Serialize;
use std::collections::HashMap;
use utopia_core::models::{ChunkView, SourcedChunk};
use utopia_core::AppResult;
use uuid::Uuid;

const RECALL_PER_CHANNEL: usize = 24;

/// 翻页最深到第几条（#1271）：再深就是在拿检索当导出用
pub const MAX_DEPTH: usize = 100;

pub struct SearchOpts {
    pub top_k: usize,
    pub offset: usize,
    pub as_of: Option<chrono::DateTime<chrono::Utc>>,
}

/// 一条命中连同它的分数（#1271）。`score` 是 RRF 总分，只在同一次查询里能排先后，
/// 跨查询不可比；两个名次从 1 数，没进那一路是 None
#[derive(Debug, Serialize)]
pub struct ScoredChunk {
    #[serde(flatten)]
    pub chunk: SourcedChunk,
    pub score: f64,
    pub bm25_rank: Option<u32>,
    pub vector_rank: Option<u32>,
}

#[derive(Debug)]
pub struct SearchPage {
    pub results: Vec<ScoredChunk>,
    pub has_more: bool,
    /// 向量那一路跑了没有；false 就是模块头说的静默降级
    pub vector_used: bool,
}

pub async fn hybrid(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    top_k: usize,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    embeddings: Option<&mut llm_util::EmbedCache>,
) -> AppResult<Vec<ChunkView>> {
    let (hits, _) = fused_hits(
        state,
        kb_id,
        workspace_id,
        query,
        RECALL_PER_CHANNEL,
        as_of,
        embeddings,
    )
    .await?;
    Ok(hits
        .into_iter()
        .take(top_k)
        .map(|hit| hit.chunk.chunk)
        .collect())
}

/// 一页命中（#1271）。每一页都向每路要同样的 `MAX_DEPTH` 条，所以各页切的是
/// 同一张融合表：页与页不重不漏。到了 `MAX_DEPTH` 的那一页没有下一页（再翻就是 422），
/// `has_more` 是 false
pub async fn hybrid_scored(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    opts: SearchOpts,
    embeddings: Option<&mut llm_util::EmbedCache>,
) -> AppResult<SearchPage> {
    let (hits, vector_used) = fused_hits(
        state,
        kb_id,
        workspace_id,
        query,
        MAX_DEPTH,
        opts.as_of,
        embeddings,
    )
    .await?;
    let end = opts.offset + opts.top_k;
    let has_more = end < MAX_DEPTH && hits.len() > end;
    let results = hits
        .into_iter()
        .skip(opts.offset)
        .take(opts.top_k)
        .collect();
    Ok(SearchPage {
        results,
        has_more,
        vector_used,
    })
}

/// 两路各取 `per_channel` 条、RRF 融合、按记录轴过滤后的整张表，连同向量那一路跑了没有
async fn fused_hits(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    per_channel: usize,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    embeddings: Option<&mut llm_util::EmbedCache>,
) -> AppResult<(Vec<ScoredChunk>, bool)> {
    let bm25 = {
        let search = state.search.clone();
        let kb = kb_id.to_string();
        let query = query.to_string();
        tokio::task::spawn_blocking(move || search.search(&kb, &query, per_channel))
    };
    // `join!` 不是 `try_join!`：向量那一路失败是降级不是错误（见 vector_channel），
    // `try_join!` 会把模型故障变成检索报错
    let (bm25, vector) = tokio::join!(
        bm25,
        vector_channel(
            state,
            kb_id,
            workspace_id,
            query,
            per_channel,
            as_of,
            embeddings
        )
    );
    let bm25 = bm25
        .map_err(|e| utopia_core::AppError::Other(anyhow::anyhow!("BM25 检索线程退出：{e}")))?
        .map_err(utopia_core::AppError::Other)?;
    let vector = vector?;
    let vector_used = vector.is_some();
    let lists = channel_lists(bm25.into_iter().map(|h| h.chunk_id).collect(), vector);

    // Keep the bounded channel candidates until their record-time and document
    // filters have run: newer hits must not consume a historical query's limit.
    let candidate_limit = lists.iter().map(Vec::len).sum();
    let fused: Vec<(Uuid, utopia_search::Fused)> =
        utopia_search::rrf_fuse_scored(&lists, candidate_limit)
            .into_iter()
            .filter_map(|f| Some((f.id.parse().ok()?, f)))
            .collect();
    let ids: Vec<Uuid> = fused.iter().map(|(id, _)| *id).collect();
    let live = utopia_store::documents::chunks_by_ids(&state.pool, kb_id, &ids, as_of).await?;
    let mut fused: HashMap<Uuid, utopia_search::Fused> = fused.into_iter().collect();
    let hits = live
        .into_iter()
        .filter_map(|chunk| {
            let f = fused.remove(&chunk.chunk.id)?;
            Some(ScoredChunk {
                chunk,
                score: f.score,
                bm25_rank: position(&f.ranks, 0),
                vector_rank: position(&f.ranks, 1),
            })
        })
        .collect();
    Ok((hits, vector_used))
}

/// 第 `channel` 路里的名次，从 1 数
fn position(ranks: &[Option<usize>], channel: usize) -> Option<u32> {
    ranks
        .get(channel)
        .copied()
        .flatten()
        .map(|rank| rank as u32 + 1)
}

/// 向量那一路。没配嵌入模型、嵌入请求失败、回了空，都是 `Ok(None)`：检索照常，
/// 只剩 BM25——这就是模块头说的静默降级。向量查询本身失败仍是 `Err`：那是库的事，
/// 不是模型的事，吞掉会把坏库伪装成没配模型。
///
/// 这一轮已经嵌过这句话（挑口径、按名字查实体都会嵌问题）就直接用那个向量，不再读
/// 设置、不再调一次嵌入端点（#971 的后续）；新嵌成功的记进去给后面的用
async fn vector_channel(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    per_channel: usize,
    as_of: Option<chrono::DateTime<chrono::Utc>>,
    cache: Option<&mut llm_util::EmbedCache>,
) -> AppResult<Option<Vec<String>>> {
    let vector = match cache.as_deref().and_then(|c| c.get(query)).cloned() {
        Some(v) => v,
        None => {
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
            let v = embeddings.remove(0);
            if let Some(cache) = cache {
                cache.insert(query.to_string(), v.clone());
            }
            v
        }
    };
    let ids = utopia_store::documents::vector_search(
        &state.pool,
        kb_id,
        &vector,
        per_channel as i64,
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
