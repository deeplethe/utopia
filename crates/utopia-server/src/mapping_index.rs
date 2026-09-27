//! 口径的检索（#574）：问数按问题挑几条确认口径，而不是按字典序塞前三十条。
//!
//! 两路、RRF 合并、取前几条，照分块检索（`retrieval::hybrid`）的形状：
//!
//! - **向量**：口径的「名字：说明（单位）」嵌进 `concept_mappings.embedding`，问题
//!   嵌一次，pgvector 排序。没配嵌入模型就没有这一路。
//! - **词面**：全部确认口径拉回来在 Rust 里打分——问题切成词（英文按空白与标点，
//!   中文按两字），数命中了几个。一个库几十到几百条，比在 SQL 里做中文分词便宜，
//!   也不用 pg_trgm。
//!
//! 降级链：向量 + 词面 → 只词面（没嵌入模型）→ 两路都空时按字典序取前几条
//! （今天的行为）。每一级都比上一级差一点，没有一级是「没有口径」。
//!
//! 索引是**懒的**：`relevant` 先把没嵌的补上再查。一次问数通常没有要补的；
//! 刚确认了一批才会有，那一次多等一个嵌入调用。谁都不用记得去刷新。
//!
//! 问题的向量走这一轮的缓存（`llm_util::EmbedCache`，#971 的后续）：对话先在这里嵌一次，
//! 随后的检索分块和按名字查实体都拿同一个向量，一轮只调一次嵌入端点。

use crate::llm_util;
use crate::state::AppState;
use utopia_core::models::ConceptMapping;
use utopia_store::mappings::MappingText;
use uuid::Uuid;

/// 进提示词几条。一道题最多要两条口径（客单价 = 净额 ÷ 订单数），八条够；
/// 再多就回到「相邻名字互相干扰」——17/18 漏的那题正是三个相邻名字都在场
pub const DEFINITIONS_IN_PROMPT: usize = 8;
/// 每一路召回几条再合并，与分块检索同一个数
const RECALL_PER_CHANNEL: usize = 24;
/// 一次嵌几条
const BATCH: usize = 32;
/// 一问最多补嵌几条。补嵌跑在问数的请求路径上：换了嵌入模型之后的第一问不该把
/// 几百条口径一口气嵌完才开口——补不完的下一问接着补，这期间少的那些由词面一路兜着
const REFRESH_PER_TURN: usize = 64;

/// 把还没嵌的确认口径补上。没配嵌入模型就什么都不做（词面一路照常）。
pub async fn refresh(state: &AppState, kb_id: Uuid, workspace_id: Uuid) -> anyhow::Result<usize> {
    let Some(settings) = utopia_store::settings::get(&state.pool, workspace_id).await? else {
        return Ok(0);
    };
    let (Some(client), Some(model)) = (
        llm_util::embed_client(&settings),
        settings.embed_model.as_deref().filter(|m| !m.is_empty()),
    ) else {
        return Ok(0);
    };
    let stale = utopia_store::mappings::needing_embedding(
        &state.pool,
        kb_id,
        model,
        REFRESH_PER_TURN as i64,
    )
    .await?;
    if stale.is_empty() {
        return Ok(0);
    }
    if stale.len() >= REFRESH_PER_TURN {
        tracing::info!(%kb_id, "口径向量这一问只补一部分，剩下的下一问接着补");
    }
    let mut done = 0usize;
    for batch in stale.chunks(BATCH) {
        let texts: Vec<String> = batch.iter().map(MappingText::embed_text).collect();
        let vectors = {
            let _permit = llm_util::acquire_embed(state, &settings).await;
            client.embed(&texts).await?
        };
        // 数量对不上就整批放弃：配对是按位置的，少一条就全体错位（分块那边同一句话）
        if vectors.len() != batch.len() {
            anyhow::bail!(
                "embedding returned {} vectors for {} texts",
                vectors.len(),
                batch.len()
            );
        }
        let items: Vec<(MappingText, Vec<f32>)> = batch.iter().cloned().zip(vectors).collect();
        utopia_store::mappings::set_embeddings(&state.pool, model, &items).await?;
        done += items.len();
    }
    tracing::info!(%kb_id, embedded = done, "口径向量已补齐");
    Ok(done)
}

/// 跟这个问题有关的口径，按相关度排。
pub async fn relevant(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    k: usize,
    embeddings: Option<&mut llm_util::EmbedCache>,
) -> anyhow::Result<Vec<ConceptMapping>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(utopia_store::mappings::confirmed(&state.pool, kb_id, k as i64).await?);
    }
    if let Err(e) = refresh(state, kb_id, workspace_id).await {
        // 补不上就用已有的向量和词面，不让一次嵌入失败把问数拖没
        tracing::warn!(%kb_id, error = %e, "口径向量没补上，按已有的检索");
    }

    let texts = utopia_store::mappings::confirmed_texts(&state.pool, kb_id).await?;
    let lexical: Vec<String> = lexical_rank(query, &texts, RECALL_PER_CHANNEL)
        .into_iter()
        .map(|id| id.to_string())
        .collect();

    let vector: Option<Vec<String>> =
        match vector_channel(state, kb_id, workspace_id, query, embeddings).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%kb_id, error = %e, "口径向量检索失败，只用词面");
                None
            }
        };

    let mut lists = vec![lexical];
    if let Some(v) = vector {
        lists.push(v);
    }
    let fused = utopia_search::rrf_fuse(&lists, k);
    let ids: Vec<Uuid> = fused.iter().filter_map(|s| s.parse().ok()).collect();
    if ids.is_empty() {
        // 两路都没命中（问题里一个词都对不上）：退回今天的行为，总比空着好
        return Ok(utopia_store::mappings::confirmed(&state.pool, kb_id, k as i64).await?);
    }
    Ok(utopia_store::mappings::by_ids(&state.pool, kb_id, &ids).await?)
}

/// 向量那一路。缓存里有这句话的向量就不再嵌（也不占嵌入闸门的名额），模型名还是要
/// 读设置：只比同一个模型嵌出来的口径
async fn vector_channel(
    state: &AppState,
    kb_id: Uuid,
    workspace_id: Uuid,
    query: &str,
    cache: Option<&mut llm_util::EmbedCache>,
) -> anyhow::Result<Option<Vec<String>>> {
    let Some(settings) = utopia_store::settings::get(&state.pool, workspace_id).await? else {
        return Ok(None);
    };
    let (Some(client), Some(model)) = (
        llm_util::embed_client(&settings),
        settings.embed_model.as_deref().filter(|m| !m.is_empty()),
    ) else {
        return Ok(None);
    };
    let vector = match cache.as_deref().and_then(|c| c.get(query)).cloned() {
        Some(v) => v,
        None => {
            let mut vectors = {
                let _permit = llm_util::acquire_embed(state, &settings).await;
                client.embed(&[query.to_string()]).await?
            };
            if vectors.is_empty() {
                return Ok(None);
            }
            let v = vectors.remove(0);
            if let Some(cache) = cache {
                cache.insert(query.to_string(), v.clone());
            }
            v
        }
    };
    let ids = utopia_store::mappings::vector_search(
        &state.pool,
        kb_id,
        model,
        &vector,
        RECALL_PER_CHANNEL as i64,
    )
    .await?;
    Ok(Some(ids.into_iter().map(|id| id.to_string()).collect()))
}

/// 问题切成词：英文按空白与标点切、转小写；中文按两字（一个字太碎，三个字漏太多）。
/// 单个 ASCII 字符不算词（「a」「x」命中一切）。
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut prev_cjk: Option<char> = None;
    for c in s.chars() {
        let cjk = ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c);
        if cjk {
            if word.len() > 1 {
                out.push(word.to_lowercase());
            }
            word.clear();
            if let Some(p) = prev_cjk {
                out.push(format!("{p}{c}"));
            }
            prev_cjk = Some(c);
            continue;
        }
        prev_cjk = None;
        if c.is_alphanumeric() {
            word.push(c);
        } else {
            if word.len() > 1 {
                out.push(word.to_lowercase());
            }
            word.clear();
        }
    }
    if word.len() > 1 {
        out.push(word.to_lowercase());
    }
    out.sort();
    out.dedup();
    out
}

/// 词面一路：每条口径的文字命中了问题里几个词，多的在前；一个都没命中的不要。
fn lexical_rank(query: &str, texts: &[MappingText], limit: usize) -> Vec<Uuid> {
    let qs = tokens(query);
    if qs.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, Uuid)> = texts
        .iter()
        .map(|t| {
            let hay = t.lexical_text().to_lowercase();
            (qs.iter().filter(|q| hay.contains(q.as_str())).count(), t.id)
        })
        .filter(|(n, _)| *n > 0)
        .collect();
    // 命中多的在前；同分按 id 定序，两次查同一个问题拿到同一个顺序
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::{lexical_rank, tokens};
    use utopia_store::mappings::MappingText;
    use uuid::Uuid;

    fn t(id: u128, name: &str, summary: &str, expr: &str) -> MappingText {
        MappingText {
            id: Uuid::from_u128(id),
            concept_name: name.into(),
            summary: Some(summary.into()),
            unit: None,
            table_name: Some("dw.dwd_ord_dtl".into()),
            expr: Some(expr.into()),
        }
    }

    #[test]
    fn a_question_is_cut_into_words_in_either_language() {
        // 输出是排好序去了重的；期望值同样排一遍，不靠手排码点
        let sorted = |v: &[&str]| {
            let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
            v.sort();
            v
        };
        assert_eq!(
            tokens("What is our total GMV?"),
            sorted(&["what", "is", "our", "total", "gmv"])
        );
        // 中文按两字；单个 ASCII 字符不算词
        assert_eq!(
            tokens("扣掉退款之后的净销售额是多少？"),
            sorted(&[
                "扣掉", "掉退", "退款", "款之", "之后", "后的", "的净", "净销", "销售", "售额",
                "额是", "是多", "多少",
            ])
        );
        assert_eq!(tokens("a x GMV"), vec!["gmv"]);
    }

    #[test]
    fn the_definition_that_shares_the_most_words_comes_first() {
        let texts = vec![
            t(1, "GMV", "已支付、非测试单的实付金额，元", "sum(amt_pay)"),
            t(2, "净销售额", "GMV 扣掉退款", "sum(amt_pay - amt_rfnd)"),
            t(3, "客单价", "净销售额除以有效订单数", "…"),
            t(4, "运费收入", "已支付订单的运费", "sum(amt_frght)"),
        ];
        let got = lexical_rank("扣掉退款之后的净销售额是多少", &texts, 8);
        assert_eq!(
            got[0],
            Uuid::from_u128(2),
            "净销售额那条命中「扣掉」「退款」「净销」「销售」「售额」"
        );
        assert!(
            got.contains(&Uuid::from_u128(3)),
            "客单价的说明里也有「净销售额」"
        );
        assert!(!got.contains(&Uuid::from_u128(4)), "运费一个词都不沾");
        // 一个词都对不上：空，让调用方降级到按字典序
        assert!(lexical_rank("zzz", &texts, 8).is_empty());
    }
}
