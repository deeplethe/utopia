//! 本体的向量索引：把类、关系、属性的 `label + description` 嵌成向量。
//!
//! 它服务的不是某一个功能，而是所有要问"这个说法对应本体里的哪一个"的地方：
//! 本体提案（今天把 1949 个 key 全量内联，且只有 key 没有描述）、谓词消解、
//! 类型消解。检索出候选之后再交给模型裁决，提示词才与本体规模脱钩。
//!
//! **自愈，不挂钩子。** 填充按"当时嵌的原文与模型名跟现在对不对得上"来判，
//! 所以任何改了描述的写入点都不需要记得通知这里——漏挂一个钩子会悄悄烂掉，
//! 而对比原文不会。

use crate::{llm_util, AppState};
use futures_util::{stream, StreamExt};
use utopia_store::ontology::TypeToEmbed;
use uuid::Uuid;

/// 一次送多少条去嵌入。
///
/// **64 是实测站得住的那个数。** 曾经改成 16，理由是拿合成短文本量出来的
/// "每条更快"——而真实的类文本平均 151 字符，那组数据不作数。改小之后请求数
/// 翻四倍、每个都付约 2.5 秒固定开销，整体吞吐反而掉了一半。
///
/// 换批大小之前先拿**真实的 `embedded_text`** 去量，别拿造出来的短句。
const BATCH: usize = 64;
/// 同时在飞的嵌入批数上限。
///
/// **关键不是快，是不能把闸门占死。** 这是个后台补齐任务，而嵌入模型的并发闸门
/// （`model_concurrency`，缺省 10）是全局共享的——文档分块入库也要走它。
///
/// 从前这里串行，一次一张许可证、每批之间释放，文档处理插得进来。改成
/// `join_all` 一次挂 31 个批次之后，它变成一个常驻的 10 深队列，每个批次握着
/// 许可证 60 秒：**5 篇文档全卡在 `embedding` 状态，抽取 15 分钟零进展**。
/// 不是 API 挂了，是这个任务把共享资源占满了。
///
/// 所以这里的上限必须**明显小于**闸门，给别的活留出位置。4 是实测能跑满
/// 吞吐又留有余量的：4 个 16 条并发，墙钟 18.5 秒（串行要 50 秒）。
const EMBED_JOBS: usize = 4;

/// 把这个库里陈掉的本体向量补齐。没配嵌入模型就直接返回 `Ok(0)`——
/// 检索是增强，不该拦住没配模型的部署。
pub async fn refresh(state: &AppState, kb_id: Uuid) -> anyhow::Result<usize> {
    refresh_scoped(state, kb_id, None).await
}

/// 只补一半。调用方清楚自己要哪一半时用它——类型消解只用类，
/// 等关系嵌完是白等。补漏的那一半有后台任务兜着。
pub async fn refresh_scoped(
    state: &AppState,
    kb_id: Uuid,
    only: Option<utopia_store::ontology::TypeKind>,
) -> anyhow::Result<usize> {
    // 库可能已经被删了——任务是导入时排的，中间隔着几分钟。
    // 这不是失败，是没事可做；当成错误的话它还要重试三次才放弃
    let Ok(kb) = utopia_store::kbs::get(&state.pool, kb_id).await else {
        return Ok(0);
    };
    let Some(settings) = utopia_store::settings::get(&state.pool, kb.workspace_id).await? else {
        return Ok(0);
    };
    let (Some(client), Some(model)) = (
        llm_util::embed_client(&settings),
        // 取拥有的一份:下面那些 future 要把 settings 挪进 Arc,借着它就动不了了
        settings.embed_model.clone(),
    ) else {
        return Ok(0);
    };

    let stale =
        utopia_store::ontology::types_needing_embedding(&state.pool, kb_id, &model, only).await?;
    if stale.is_empty() {
        return Ok(0);
    }
    let mut done = 0usize;
    let mut failed = 0usize;
    // **全部用拥有的数据**：这些 future 要跨 await 被并发驱动，借用过不了 Send 边界
    let client = std::sync::Arc::new(client);
    let model = std::sync::Arc::new(model);
    let settings = std::sync::Arc::new(settings);
    let batches: Vec<Vec<TypeToEmbed>> = stale.chunks(BATCH).map(<[_]>::to_vec).collect();
    let mut jobs = stream::iter(batches.into_iter().map(|batch| {
        let (client, model, settings) = (client.clone(), model.clone(), settings.clone());
        async move {
            let texts: Vec<String> = batch.iter().map(|t| t.text.clone()).collect();
            let _permit = llm_util::acquire_embed(state, &settings).await;
            let vectors = client.embed(&texts).await?;
            // 数量对不上就整批放弃：按位置配对，错位会把 person 的向量写到
            // organization 上，而这种错一旦落库就再也看不出来了
            if vectors.len() != batch.len() {
                anyhow::bail!("嵌入返回 {} 条，送去的是 {} 条", vectors.len(), batch.len());
            }
            let n = batch.len();
            let items: Vec<(TypeToEmbed, Vec<f32>)> = batch.into_iter().zip(vectors).collect();
            utopia_store::ontology::set_type_embeddings(&state.pool, &model, &items).await?;
            Ok::<usize, anyhow::Error>(n)
        }
    }))
    .buffer_unordered(EMBED_JOBS);
    // **一批失败不拖垮其余**：这是补齐任务，缺的下一轮自愈（判据是"当时嵌的
    // 原文对不对得上"，不是时间戳）。整个 bail 掉等于把已经嵌好的也白跑一遍
    while let Some(r) = jobs.next().await {
        match r {
            Ok(n) => done += n,
            Err(e) => {
                failed += 1;
                tracing::warn!(%kb_id, error = %e, "一批本体向量没嵌上，留给下一轮");
            }
        }
    }
    // **失败数要出现在这一行。** 从前它只报补齐了多少,于是 24 批全挂的那次
    // 日志照样写着"已补齐",看日志的人以为没事
    if failed > 0 {
        tracing::warn!(%kb_id, count = done, failed, "本体向量补了一部分，其余留给下一轮");
    } else {
        tracing::info!(%kb_id, count = done, "本体向量已补齐");
    }
    Ok(done)
}

/// 一批说法各自最像本体里的哪几个关系/属性。
///
/// **一次嵌完再逐个查库**，不是每个说法各发一次嵌入请求：本体提案一轮要处理
/// 十几到几十个说法，逐个发就是十几到几十次往返。
pub async fn nearest_for_each(
    state: &AppState,
    kb_id: Uuid,
    queries: &[String],
    limit: i64,
    target: Target,
) -> anyhow::Result<Vec<Vec<utopia_core::models::TypeCandidate>>> {
    let empty = || vec![Vec::new(); queries.len()];
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    let Some(settings) = utopia_store::settings::get(&state.pool, kb.workspace_id).await? else {
        return Ok(empty());
    };
    let Some(client) = llm_util::embed_client(&settings) else {
        return Ok(empty());
    };
    let mut vectors = Vec::with_capacity(queries.len());
    for batch in queries.chunks(BATCH) {
        let _permit = llm_util::acquire_embed(state, &settings).await;
        let got = client.embed(batch).await?;
        if got.len() != batch.len() {
            anyhow::bail!("嵌入返回 {} 条，送去的是 {} 条", got.len(), batch.len());
        }
        vectors.extend(got);
    }
    let mut out = Vec::with_capacity(vectors.len());
    for v in &vectors {
        out.push(match target {
            Target::Class => {
                utopia_store::ontology::nearest_entity_types(&state.pool, kb_id, v, limit, false)
                    .await?
            }
            Target::ClassLabel => {
                utopia_store::ontology::nearest_entity_types(&state.pool, kb_id, v, limit, true)
                    .await?
            }
            Target::Predicate(kind) => {
                utopia_store::ontology::nearest_relation_types(&state.pool, kb_id, v, limit, kind)
                    .await?
            }
        });
    }
    Ok(out)
}

/// 检索的是本体的哪一半。
///
/// **必须分开。** 类进 `entity_types`、关系与属性进 `relation_types`，两边的
/// 描述写的是完全不同的东西（"什么样的实体属于这里" vs "这条断言说了什么"）。
/// 拿一个词表外的类名去关系里检索，回来的一定是勉强相关的关系。
#[derive(Debug, Clone, Copy)]
pub enum Target {
    /// 整段索引（label + 描述）。长画像走这一路
    Class,
    /// **只有 label 的索引**（见 `entity_types.label_embedding`）。短说法（`district. place`）走这一路。
    /// 短说法比整段索引会被同义反复的类接管——`Map` 那一行的描述就是
    /// "A map."，赢在长度不在语义
    ClassLabel,
    /// `Some("attribute")` 只找属性、`Some("relation")` 只找关系、`None` 都找。
    /// 字面值宾语的事实要的是属性，实体宾语的要的是关系
    Predicate(Option<&'static str>),
}
