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
use utopia_store::ontology::TypeToEmbed;
use uuid::Uuid;

/// 一次送多少条去嵌入。跟文档分块那边同量级，够摊平往返又不至于把请求撑爆。
const BATCH: usize = 64;

/// 把这个库里陈掉的本体向量补齐。没配嵌入模型就直接返回 `Ok(0)`——
/// 检索是增强，不该拦住没配模型的部署。
pub async fn refresh(state: &AppState, kb_id: Uuid) -> anyhow::Result<usize> {
    let kb = utopia_store::kbs::get(&state.pool, kb_id).await?;
    let Some(settings) = utopia_store::settings::get(&state.pool, kb.workspace_id).await? else {
        return Ok(0);
    };
    let (Some(client), Some(model)) = (
        llm_util::embed_client(&settings),
        settings.embed_model.as_deref(),
    ) else {
        return Ok(0);
    };

    let stale = utopia_store::ontology::types_needing_embedding(&state.pool, kb_id, model).await?;
    if stale.is_empty() {
        return Ok(0);
    }
    let mut done = 0usize;
    for batch in stale.chunks(BATCH) {
        let texts: Vec<String> = batch.iter().map(|t| t.text.clone()).collect();
        let _permit = llm_util::acquire_embed(state, &settings).await;
        let vectors = client.embed(&texts).await?;
        // 数量对不上就整批放弃：按位置配对，错位会把 person 的向量写到
        // organization 上，而这种错一旦落库就再也看不出来了
        if vectors.len() != batch.len() {
            anyhow::bail!("嵌入返回 {} 条，送去的是 {} 条", vectors.len(), batch.len());
        }
        let items: Vec<(TypeToEmbed, Vec<f32>)> = batch.iter().cloned().zip(vectors).collect();
        utopia_store::ontology::set_type_embeddings(&state.pool, model, &items).await?;
        done += batch.len();
    }
    tracing::info!(%kb_id, count = done, "本体向量已补齐");
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
                utopia_store::ontology::nearest_entity_types(&state.pool, kb_id, v, limit).await?
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
    Class,
    /// `Some("attribute")` 只找属性、`Some("relation")` 只找关系、`None` 都找。
    /// 字面值宾语的事实要的是属性，实体宾语的要的是关系
    Predicate(Option<&'static str>),
}
