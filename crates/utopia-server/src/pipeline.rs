//! 摄入管道：parse → chunk → 全文索引 → embedding（可选）→ ready。
//! 每步幂等：重跑会先清掉旧分块与旧索引条目。

use crate::llm_util;
use crate::state::AppState;
use uuid::Uuid;

const EMBED_BATCH: usize = 16;

pub async fn process_document(state: &AppState, document_id: Uuid) -> anyhow::Result<()> {
    match run(state, document_id).await {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ =
                utopia_store::documents::set_failed(&state.pool, document_id, &e.to_string()).await;
            if let Ok(doc) = utopia_store::documents::get(&state.pool, document_id).await {
                state.emit_document(doc.kb_id, document_id);
            }
            Err(e)
        }
    }
}

async fn run(state: &AppState, document_id: Uuid) -> anyhow::Result<()> {
    let doc = utopia_store::documents::get(&state.pool, document_id).await?;

    // 1. 解析（CPU 密集，放 blocking 线程）
    utopia_store::documents::set_status(&state.pool, document_id, "parsing").await?;
    state.emit_document(doc.kb_id, document_id);
    let bytes = state.blob.get(&doc.sha256).await?;
    let filename = doc.filename.clone();
    let parsed =
        tokio::task::spawn_blocking(move || utopia_ingest::parse(&filename, &bytes)).await??;
    let text_len = parsed.text.chars().count() as i32;

    // 2. 分块 + 入库
    let pieces = utopia_ingest::chunk_text(&parsed.text);
    let chunk_pairs =
        utopia_store::documents::replace_chunks(&state.pool, doc.kb_id, document_id, &pieces)
            .await?;
    let chunk_count = chunk_pairs.len() as i32;

    // 3. 全文索引（Tantivy）
    utopia_store::documents::set_status(&state.pool, document_id, "indexing").await?;
    state.emit_document(doc.kb_id, document_id);
    let search = state.search.clone();
    let kb = doc.kb_id.to_string();
    let did = document_id.to_string();
    tokio::task::spawn_blocking(move || search.reindex_document(&kb, &did, &chunk_pairs)).await??;

    // 4. embedding（工作区配置了 embedding 模型才做；没配也算 ready，先享受 BM25 搜索）
    let kb_row = utopia_store::kbs::get(&state.pool, doc.kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb_row.workspace_id).await?;
    if let Some(client) = settings.as_ref().and_then(llm_util::embed_client) {
        utopia_store::documents::set_status(&state.pool, document_id, "embedding").await?;
        state.emit_document(doc.kb_id, document_id);
        let pending =
            utopia_store::documents::chunks_pending_embedding(&state.pool, document_id).await?;
        for batch in pending.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            let _permit = match settings.as_ref() {
                Some(s) => llm_util::acquire_embed(state, s).await,
                None => None,
            };
            let embeddings = client.embed(&texts).await?;
            if embeddings.len() != batch.len() {
                anyhow::bail!("Embedding 返回数量不匹配");
            }
            let items: Vec<(Uuid, Vec<f32>)> =
                batch.iter().map(|(id, _)| *id).zip(embeddings).collect();
            utopia_store::documents::set_embeddings(&state.pool, &items).await?;
        }
    }

    utopia_store::documents::set_ready(&state.pool, document_id, text_len, chunk_count).await?;

    // 两段式：索引就绪后，若配置了对话模型则排队图谱抽取（不阻塞可搜可问）
    if settings.as_ref().is_some_and(|s| s.chat_ready()) {
        utopia_store::documents::set_graph_status(&state.pool, document_id, "queued").await?;
        utopia_store::jobs::enqueue(
            &state.pool,
            "extract_document",
            serde_json::json!({ "document_id": document_id }),
        )
        .await?;
    }
    state.emit_document(doc.kb_id, document_id);

    tracing::info!(%document_id, chunks = chunk_count, "文档处理完成");
    Ok(())
}

/// 记忆摄入（episodes 快速路径的后半程）：新 episode chunk 补 embedding、
/// 重建全文索引、触发增量抽取（extracted_at 为空的新 chunk 才会被抽）。
/// 免解析免分块——episode 落库时已是 chunk。
///
/// `proposed_by`：说这句话的人。一路传到抽取，落在 `pending_facts.proposed_by`（0015）
pub async fn memory_ingest(
    state: &AppState,
    document_id: Uuid,
    proposed_by: Option<Uuid>,
) -> anyhow::Result<()> {
    let doc = utopia_store::documents::get(&state.pool, document_id).await?;
    let kb_row = utopia_store::kbs::get(&state.pool, doc.kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb_row.workspace_id).await?;

    if let Some(client) = settings.as_ref().and_then(llm_util::embed_client) {
        let pending =
            utopia_store::documents::chunks_pending_embedding(&state.pool, document_id).await?;
        for batch in pending.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            let _permit = match settings.as_ref() {
                Some(s) => llm_util::acquire_embed(state, s).await,
                None => None,
            };
            let embeddings = client.embed(&texts).await?;
            if embeddings.len() != batch.len() {
                anyhow::bail!("Embedding 返回数量不匹配");
            }
            let items: Vec<(Uuid, Vec<f32>)> =
                batch.iter().map(|(id, _)| *id).zip(embeddings).collect();
            utopia_store::documents::set_embeddings(&state.pool, &items).await?;
        }
    }

    let chunks = utopia_store::documents::chunks_full(&state.pool, document_id).await?;
    let pairs: Vec<(String, String)> = chunks
        .iter()
        .map(|c| (c.id.to_string(), c.text.clone()))
        .collect();
    let search = state.search.clone();
    let kb = doc.kb_id.to_string();
    let did = document_id.to_string();
    tokio::task::spawn_blocking(move || search.reindex_document(&kb, &did, &pairs)).await??;

    if settings.as_ref().is_some_and(|s| s.chat_ready()) {
        utopia_store::documents::set_graph_status(&state.pool, document_id, "queued").await?;
        utopia_store::jobs::enqueue(
            &state.pool,
            "extract_document",
            serde_json::json!({ "document_id": document_id, "proposed_by": proposed_by }),
        )
        .await?;
    }
    state.emit_document(doc.kb_id, document_id);
    Ok(())
}
