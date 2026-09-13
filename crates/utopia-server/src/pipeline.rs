//! 摄入管道：parse → chunk → 全文索引 → embedding（可选）→ ready。
//! 每步幂等：重跑会先清掉旧分块与旧索引条目。

use crate::llm_util;
use crate::state::AppState;
use futures_util::{stream, StreamExt};
use utopia_core::models::{LlmSettings, Proposer};
use utopia_llm::LlmClient;
use uuid::Uuid;

/// 这份文档的来源要不要抽取。没有来源的文档（直接上传、记忆片段）照旧抽。
async fn source_extracts(state: &AppState, source_id: Option<Uuid>) -> anyhow::Result<bool> {
    let Some(id) = source_id else {
        return Ok(true);
    };
    Ok(utopia_store::sources::get(&state.pool, id)
        .await?
        .extracts())
}

/// 每次送多少条去嵌入。
///
/// 与 ontology_index 的 64 不同，这里没量过，先不动：那边的注释说批大小要拿真实文本量，
/// 块文本比类标签长得多，16 未必错。并发与批大小两个旋钮别在一刀里同时拧，否则结果读不出来。
const EMBED_BATCH: usize = 16;

/// 同时在飞的嵌入批数上限（#513）。
///
/// 嵌入模型的并发闸门（`model_concurrency`，缺省 10）是部署级共享的：本体补齐
/// （ontology_index，上限 4）、类型消解、每次提问的查询嵌入都走它。摄入是用户等着看的
/// 前台活，比后台补齐更有资格用闸门；但它也是能一次带来上千个分块的那一路，不设上限
/// 就把补齐、消解、检索全饿死——ontology_index 记着一次 `join_all` 把闸门占死、五篇文档
/// 卡在 embedding 十五分钟的事故。4 与补齐相同：两路同时跑合计 8，闸门还剩 2 给别人。
const EMBED_JOBS: usize = 4;

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
    // 排队之后被删了（#268）：墓碑不重建分块、不回索引；清过的连原文都没了
    if doc.deleted_at.is_some() {
        tracing::info!(document = %document_id, "skipping a deleted document");
        return Ok(());
    }

    // 1. 解析（CPU 密集，放 blocking 线程）
    utopia_store::documents::set_status(&state.pool, document_id, "parsing").await?;
    state.emit_document(doc.kb_id, document_id);
    let bytes = state.blob.get(&doc.sha256).await?;
    let filename = doc.filename.clone();
    let parsed =
        tokio::task::spawn_blocking(move || utopia_ingest::parse(&filename, &bytes)).await??;
    // 解析出来的正文可能夹着 NUL，入库之前剥掉（见 `without_nul`）。之后的长度、分块、
    // 全文索引、嵌入读的都是这一份，彼此的偏移才对得上
    let text = without_nul(&parsed.text);
    let text_len = text.chars().count() as i32;

    // 2. 分块 + 入库
    let pieces = utopia_ingest::chunk_text(&text);
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
    if let Some((settings, client)) = embedder(settings.as_ref()) {
        utopia_store::documents::set_status(&state.pool, document_id, "embedding").await?;
        state.emit_document(doc.kb_id, document_id);
        embed_pending(state, settings, &client, document_id).await?;
    }

    utopia_store::documents::set_ready(&state.pool, document_id, text_len, chunk_count).await?;

    // 来源说了不抽取的，到这里为止：可搜、可问，不进图。
    //
    // schema 文档就是这一类（0035 决定 7）——它是给问数检索表结构的语料，进抽取的
    // 结果是抽取器把每个列名当成一个实体（宽表语料上四十个概念实体里二十八个是
    // 列名，#553）。状态记成 `skipped` 而不是留在 `none`：`none` 在 Library 里读作
    // 「还没排到」，而它永远不会排到
    if !source_extracts(state, doc.source_id).await? {
        utopia_store::documents::set_graph_status(&state.pool, document_id, "skipped").await?;
        state.emit_document(doc.kb_id, document_id);
        tracing::info!(%document_id, chunks = chunk_count, "文档处理完成，来源不抽取");
        return Ok(());
    }

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
/// `proposer`：说这句话的人，以及经 MCP 时那个 agent。一路传到抽取，落在
/// `pending_facts.proposed_by` / `proposed_token`（0015、0026）
pub async fn memory_ingest(
    state: &AppState,
    document_id: Uuid,
    proposer: Proposer,
) -> anyhow::Result<()> {
    let doc = utopia_store::documents::get(&state.pool, document_id).await?;
    if doc.deleted_at.is_some() {
        tracing::info!(document = %document_id, "skipping a deleted document");
        return Ok(());
    }
    let kb_row = utopia_store::kbs::get(&state.pool, doc.kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb_row.workspace_id).await?;

    if let Some((settings, client)) = embedder(settings.as_ref()) {
        embed_pending(state, settings, &client, document_id).await?;
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
            serde_json::json!({
                "document_id": document_id,
                "proposed_by": proposer.user_id,
                "proposed_token": proposer.token_id,
            }),
        )
        .await?;
    }
    state.emit_document(doc.kb_id, document_id);
    Ok(())
}

/// 工作区配了嵌入模型才有客户端；设置与客户端一起交出去，闸门许可证要按设置取。
fn embedder(settings: Option<&LlmSettings>) -> Option<(&LlmSettings, LlmClient)> {
    let s = settings?;
    llm_util::embed_client(s).map(|c| (s, c))
}

/// 把这篇文档还没有向量的分块全部嵌完，返回嵌了多少条。文档摄入与记忆摄入共用——
/// 从前两处各抄一份同样的循环，且都是一批等完再发下一批。
///
/// 批与批之间互不依赖（向量按位置配对，只在一批之内），`EMBED_JOBS` 个批次同时在飞，
/// 每个批次握一张闸门许可证只到自己写完。
///
/// **数量对不上就整批放弃。** 配对是按位置的，少一条就全体错位，把一条的向量写到另一条
/// 身上；这种错落库之后再也看不出来——正文还在，向量是别人的。
///
/// **任一批失败整篇失败。** 串行时错误自然一路 unwind；并发下要明确地把它传出去，让调用方
/// 把文档标成 failed，而不是留它停在 embedding。已经写进去的向量留着，重跑只补缺的
/// （`chunks_pending_embedding` 只挑 embedding 为空的）。
async fn embed_pending(
    state: &AppState,
    settings: &LlmSettings,
    client: &LlmClient,
    document_id: Uuid,
) -> anyhow::Result<usize> {
    let pending =
        utopia_store::documents::chunks_pending_embedding(&state.pool, document_id).await?;
    // 批次先切成拥有的数据：借来的切片捕进并发驱动的 future 过不了 Send 边界
    //（ontology_index 里同一句话）
    let batches: Vec<Vec<(Uuid, String)>> =
        pending.chunks(EMBED_BATCH).map(<[_]>::to_vec).collect();
    let mut batches = stream::iter(batches)
        .map(|batch| async move {
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            let _permit = llm_util::acquire_embed(state, settings).await;
            let embeddings = client.embed(&texts).await?;
            if embeddings.len() != batch.len() {
                anyhow::bail!(
                    "嵌入返回 {} 条，送去的是 {} 条",
                    embeddings.len(),
                    batch.len()
                );
            }
            let items: Vec<(Uuid, Vec<f32>)> =
                batch.iter().map(|(id, _)| *id).zip(embeddings).collect();
            utopia_store::documents::set_embeddings(&state.pool, &items).await?;
            Ok::<usize, anyhow::Error>(items.len())
        })
        .buffer_unordered(EMBED_JOBS);
    let mut done = 0;
    while let Some(written) = batches.next().await {
        done += written?;
    }
    Ok(done)
}

/// 正文里的 NUL（`\0`）剥掉。
///
/// PDF 的文本层会夹带它——字体的 `ToUnicode` 映射与编码过的内容流是常见来源——而
/// Postgres 的 `TEXT` 不收 0x00：`chunks.text` 一插就报
/// `invalid byte sequence for encoding "UTF8": 0x00`，事务回滚，**整篇文档钉在 failed**，
/// 坏的只是其中几个字节。剥在这里，是因为正文进库只有这一条路（`replace_chunks` 只有
/// 这一个调用方），而且要在算长度和分块**之前**：放到更下游，存下的 `text_len` 与分块
/// 偏移就和真正入库的正文对不上了。
///
/// 绝大多数文档一个 NUL 都没有，那时原样借用，不为每篇文档复制一整份正文。
/// （#611，Jun Du / @plpycoin 报告并给出修法）
fn without_nul(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\0') {
        std::borrow::Cow::Owned(text.replace('\0', ""))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod pipeline_tests;
