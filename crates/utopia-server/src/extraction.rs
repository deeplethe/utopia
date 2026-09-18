//! 图谱抽取任务的入口，以及开放写入路径（`extraction_open`）共用的那几块。
//!
//! 抽取只写开放图谱（0044 决定 2）：每篇文档——记忆日志也在内——都从这里进
//! `run_open`，提示词里没有本体，也没有带本体的第二条路。这里留下的是那条路借用的东西：
//! 限流退避的 chat 调用、来源给置信度设的上限、片段核对、丢弃信号、未抽完的判据、
//! 名字到实体的消解（句柄、同名审核对）。
//! 与摄入管道分离（两段式可用）：索引完成即可搜可问，抽取慢慢跑。
//! 消解灰区只入审核队列并触发独立的攒批裁决任务——LLM 裁决永不阻塞本任务。

use crate::llm_util;
use crate::state::AppState;
use sqlx::PgPool;
use std::collections::HashMap;
use std::time::Duration;
use utopia_core::models::Proposer;
use uuid::Uuid;

/// 限流最多退避重试几次。**只对 429 生效**：密钥错了重试一万次还是错。
const RATE_LIMIT_TRIES: u32 = 5;
/// 单次退避的上限。总等待因此封顶在两分钟出头，一个配额永远打满的账号
/// 会干脆地失败，而不是把 worker 槽占死。
const RATE_LIMIT_CAP: Duration = Duration::from_secs(60);

/// 退避的抖动。**不引 `rand`**：这里只要「别让 N 个分块同时醒来」，纳秒时钟
/// 就够散，而多一个依赖要跟着走供应链。
///
/// 取半区间（base/2 到 base）而不是全区间：退避仍然单调增长，只是错开。
fn jitter(base: Duration) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    let half = (base.as_millis() as u64) / 2;
    base / 2 + Duration::from_millis(if half == 0 { 0 } else { nanos % half })
}

/// 抽取的 chat 调用，**限流会退避重试**。温度由调用方定（开放抽取要 0，见 `extraction_open`）。
///
/// 限流与其他失败的区别是它会自己好，所以从前那句「跳过该分块」用在它身上
/// 就是把一分钟的等待换成永久的数据缺口——实测一次 1884 块的灌入里
/// 55/60 篇文档整篇失败，而端点一直是好的。
///
/// 两处容易写错：
///
/// - **退避期间不能占着许可。** 许可只包住真正的调用，睡觉之前就还回去，
///   否则一个在等的分块会挡住本来可以通过的另一个。
/// - **`Retry-After` 多数厂商不发**，所以它只是「有则更准」，判据是错误类型
///   本身；没有它就走指数退避。
pub(crate) async fn chat_retrying_rate_limits_at(
    state: &AppState,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
    messages: &[utopia_llm::ChatMessage],
    temperature: Option<f32>,
) -> anyhow::Result<String> {
    let mut backoff = Duration::from_secs(2);
    for attempt in 1..=RATE_LIMIT_TRIES {
        // 许可只包住调用本身，出了这个块就还回去
        let outcome = {
            let _permit = llm_util::acquire_chat(state, settings).await;
            client.chat_at(messages, temperature).await
        };
        let err = match outcome {
            Ok(reply) => return Ok(reply),
            Err(e) => e,
        };
        // 会自己好的那几类（限流、端点不可用、请求没送到）退避重试，其余照原样抛出去
        let Some((what, retry_after)) = utopia_llm::transient(&err) else {
            return Err(err);
        };
        if attempt == RATE_LIMIT_TRIES {
            return Err(err.context(format!("{what}退避 {RATE_LIMIT_TRIES} 次仍未通过")));
        }
        let delay = jitter(retry_after.unwrap_or(backoff).min(RATE_LIMIT_CAP));
        tracing::warn!(
            attempt,
            delay_ms = delay.as_millis() as u64,
            from_header = retry_after.is_some(),
            why = what,
            "端点这次没答成，退避后重试"
        );
        tokio::time::sleep(delay).await;
        backoff = (backoff * 2).min(RATE_LIMIT_CAP);
    }
    unreachable!("循环内必定 return")
}

/// 模型看图描述出来的事实，置信度压到这里（0040 决定 4）：低于时态引擎自动关闭的门槛，
/// 它能进图、能被搜到和引用，却不能单独把一段正确的旧值关掉——看柱状图读错一个数字是常事，
/// 按普通事实入库的话，错的数会关掉它反驳的那个对的数，而库里没有一行说这次关闭靠的是一张图。
/// 真要关，时态引擎记一条低置信冲突，交给人或治理去判
const DESCRIBED_CEILING: f32 = utopia_store::temporal::AUTO_CLOSE_MIN_CONFIDENCE - 0.05;

/// 这块文字的来源给事实置信度设的上限
pub(crate) fn origin_ceiling(origin: &str, confidence: f32) -> f32 {
    if origin == "described" {
        confidence.min(DESCRIBED_CEILING)
    } else {
        confidence
    }
}

/// 片段在不在引文里：大小写、空白都不论
pub(crate) fn span_in_quote(span: &str, quote: &str) -> bool {
    let norm = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let (s, q) = (norm(span), norm(quote));
    !s.is_empty() && q.contains(&s)
}

/// 记一条丢弃信号。抽取器有七处 `continue`，每一处都是"事实抽出来了、被挡掉、
/// 什么都不说"。信号写失败绝不能带垮整篇文档的抽取，所以这里吞掉错误。
pub(crate) async fn drop_signal(
    state: &AppState,
    kb_id: Uuid,
    document_id: Uuid,
    reason: &str,
    detail: &str,
    example: Option<&str>,
) {
    let _ = utopia_store::extraction_drops::record(
        &state.pool,
        kb_id,
        document_id,
        reason,
        detail,
        example,
    )
    .await;
}

/// 这一轮抽完了没有——没抽完就给出**要写进 graph_error 的那句话**。
///
/// 判据是「全抽完」而不是某个比例：任何比例都是拍的，而这里本来就有一个不需要拍的
/// 判据——每一块都成了才叫抽完。
///
/// `attempted` 是**本轮取到的分块数**，不是文档总块数：重试只取
/// `extracted_at IS NULL` 的块，所以第二轮的分母天然更小。措辞里说「本轮」，
/// 别让读的人以为文档只有那么几块。
pub(crate) fn incomplete_reason(unextracted: &[(i32, String)], attempted: usize) -> Option<String> {
    if unextracted.is_empty() {
        return None;
    }
    // 只举前三个：原因往往同一个（供应商不通就是所有块都不通），
    // 全列出来只是把同一句话抄二十遍
    let sample: Vec<String> = unextracted
        .iter()
        .take(3)
        .map(|(seq, why)| format!("#{seq} {why}"))
        .collect();
    let more = unextracted.len().saturating_sub(sample.len());
    let tail = if more > 0 {
        format!("；另有 {more} 个")
    } else {
        String::new()
    };
    Some(format!(
        "本轮 {attempted} 个分块里 {} 个没能抽取：{}{tail}",
        unextracted.len(),
        sample.join("；")
    ))
}

/// `proposer`：这篇文档若是记忆日志，抽出的事实等人点头，据此记下「谁说的」
/// ——人，以及经 MCP 时那个 agent（0014 的令牌）。批量摄入的文档传默认值：
/// 那条路不经待确认队列，这两位都用不上
pub async fn extract_document(
    state: &AppState,
    document_id: Uuid,
    proposer: Proposer,
) -> anyhow::Result<()> {
    match run(state, document_id, proposer).await {
        Ok(()) => Ok(()),
        Err(e) => {
            // 原因随状态落库：只进日志的错误等于没有错误
            let _ = utopia_store::documents::set_graph_failed(
                &state.pool,
                document_id,
                &format!("{e:#}"),
            )
            .await;
            if let Ok(doc) = utopia_store::documents::get(&state.pool, document_id).await {
                state.emit_document(doc.kb_id, document_id);
            }
            Err(e)
        }
    }
}

/// Resolve without the document's name cache. Handles use this path so two identities claimed
/// separately in one response cannot collapse before their fact refs are bound.
#[allow(clippy::too_many_arguments)]
async fn resolve_uncached(
    pool: &PgPool,
    kb_id: Uuid,
    type_id: Option<Uuid>,
    name: &str,
    ctx: Option<&[f32]>,
    text: Option<&str>,
    exclude: &[Uuid],
    needs_adjudication: &mut bool,
) -> anyhow::Result<Uuid> {
    let r =
        utopia_store::resolution::resolve_mention(pool, kb_id, type_id, name, ctx, text, exclude)
            .await?;
    // 疑似重复对（画像灰区 / 类型漂移 / 同名并列）入审核队列。多数走批量裁决器，
    // 同名并列（`ReviewStage::Human`）分不出谁是谁，只能等人裁——它自己带着 stage。
    for review in &r.reviews {
        utopia_store::resolution::create_review(
            pool,
            kb_id,
            r.entity_id,
            review.other_id,
            review.score,
            &review.reason,
            review.stage,
        )
        .await?;
        // 只有批量可裁的才触发裁决任务；纯人工审核对不该唤醒裁决器
        if review.stage == utopia_store::resolution::ReviewStage::Adjudicating {
            *needs_adjudication = true;
        }
    }
    Ok(r.entity_id)
}

async fn create_namesake_reviews(
    pool: &PgPool,
    kb_id: Uuid,
    entity_id: Uuid,
    others: &[Uuid],
) -> anyhow::Result<bool> {
    let mut created = false;
    for other_id in others.iter().copied().filter(|id| *id != entity_id) {
        utopia_store::resolution::create_review(
            pool,
            kb_id,
            entity_id,
            other_id,
            1.0,
            "namesake",
            utopia_store::resolution::ReviewStage::Human,
        )
        .await?;
        created = true;
    }
    Ok(created)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn resolve_handle(
    pool: &PgPool,
    kb_id: Uuid,
    type_id: Option<Uuid>,
    name: &str,
    ctx: Option<&[f32]>,
    text: Option<&str>,
    response_claims: &mut HashMap<String, Vec<Uuid>>,
    handled_by_name: &mut HashMap<String, Vec<Uuid>>,
    ambiguous_bare_cache: &mut HashMap<String, Uuid>,
    needs_adjudication: &mut bool,
    human_reviews_found: &mut bool,
) -> anyhow::Result<Uuid> {
    let normalized = utopia_store::resolution::normalize_name(name).to_lowercase();
    let response_excluded = response_claims
        .get(&normalized)
        .cloned()
        .unwrap_or_default();
    let document_excluded = handled_by_name
        .get(&normalized)
        .filter(|ids| ids.len() > 1)
        .cloned()
        .unwrap_or_default();
    let mut excluded = response_excluded.clone();
    for id in &document_excluded {
        if !excluded.contains(id) {
            excluded.push(*id);
        }
    }
    // A later response may allocate a fresh e-handle for an otherwise bare ambiguous name
    // instead of choosing one of the supplied k-handles. That must still become/reuse C; a new
    // response-local spelling is not permission to guess A or B.
    let reuse_document_provisional = response_excluded.is_empty() && document_excluded.len() > 1;
    let id = match reuse_document_provisional
        .then(|| ambiguous_bare_cache.get(&normalized).copied())
        .flatten()
    {
        Some(id) => id,
        None => {
            let id = resolve_uncached(
                pool,
                kb_id,
                type_id,
                name,
                ctx,
                text,
                &excluded,
                needs_adjudication,
            )
            .await?;
            if reuse_document_provisional {
                ambiguous_bare_cache.insert(normalized.clone(), id);
            }
            id
        }
    };
    if create_namesake_reviews(pool, kb_id, id, &excluded).await? {
        *human_reviews_found = true;
    }
    response_claims
        .entry(normalized.clone())
        .or_default()
        .push(id);
    let document_claims = handled_by_name.entry(normalized).or_default();
    if !document_claims.contains(&id) {
        document_claims.push(id);
    }
    Ok(id)
}

/// 每篇文档都走开放图谱（0044 决定 2）：只写文档自己的话，本体不进提示词。
/// 记忆日志同样从这里进，只是它的陈述先进待确认表等人点头（0015），点头时才落成开放陈述
async fn run(state: &AppState, document_id: Uuid, proposer: Proposer) -> anyhow::Result<()> {
    let doc = utopia_store::documents::get(&state.pool, document_id).await?;
    // 排队之后被删了（#268）：墓碑不抽——抽出来的事实会活在一个已删除的出处上
    if doc.deleted_at.is_some() {
        tracing::info!(document = %document_id, "skipping a deleted document");
        return Ok(());
    }
    let kb = utopia_store::kbs::get(&state.pool, doc.kb_id).await?;
    let settings = utopia_store::settings::get(&state.pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot extract"))?;
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot extract"))?;

    // 所有权凭证：重抽会自增 epoch，任务据此察觉自己已被接管（见 `run_open` 的分块循环）
    let my_epoch = utopia_store::documents::extract_epoch(&state.pool, document_id).await?;
    utopia_store::documents::set_graph_status(&state.pool, document_id, "extracting").await?;
    state.emit_document(doc.kb_id, document_id);
    let await_nod = utopia_store::memory::is_memory_document(&state.pool, document_id).await?;
    crate::extraction_open::run_open(
        state, &doc, &kb, &settings, &client, my_epoch, proposer, await_nod,
    )
    .await
}

#[cfg(test)]
mod origin_ceiling_tests {
    use super::*;

    #[test]
    fn a_described_fact_cannot_close_a_value_by_itself() {
        let ceiling = origin_ceiling("described", 0.95);
        assert!(ceiling < utopia_store::temporal::AUTO_CLOSE_MIN_CONFIDENCE);
        assert_eq!(ceiling, DESCRIBED_CEILING, "it still enters the graph");
        assert_eq!(origin_ceiling("described", 0.62), 0.62);
        for origin in ["stated", "ocr", "transcribed"] {
            assert_eq!(origin_ceiling(origin, 0.95), 0.95);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{incomplete_reason, resolve_handle, span_in_quote};
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn a_span_is_found_in_its_quote_regardless_of_case_and_spacing() {
        assert!(span_in_quote(
            "former openai   personnel",
            "Former OpenAI personnel have founded"
        ));
        assert!(!span_in_quote(
            "OpenAI staff",
            "Former OpenAI personnel have founded"
        ));
        assert!(!span_in_quote("", "anything"));
    }

    /// 这条判据存在的理由：一次网络抖动让六篇文档 60 块里只抽成 12 块，
    /// 六篇**全部显示"抽取完成"**，八成的内容没进图，界面上没有任何东西说出来。
    #[test]
    fn a_document_with_a_skipped_chunk_is_not_complete() {
        assert_eq!(incomplete_reason(&[], 23), None, "全抽完才算完成");
        let one = [(7, "调用失败：timeout".to_string())];
        let msg = incomplete_reason(&one, 23).expect("有块没抽成就不该算完成");
        assert!(msg.contains("23"), "分母要说出来：{msg}");
        assert!(msg.contains("#7"), "得指得出是哪一块：{msg}");
    }

    /// 原因往往是同一个（供应商不通就是所有块都不通），举三个够了，
    /// 但**剩下多少必须说**——否则读的人会以为只坏了三块。
    #[test]
    fn many_failures_are_summarised_without_hiding_the_count() {
        let many: Vec<(i32, String)> = (1..=20).map(|i| (i, "调用失败".into())).collect();
        let msg = incomplete_reason(&many, 60).unwrap();
        assert!(msg.contains("20"), "总数要在：{msg}");
        assert!(msg.contains("另有 17 个"), "省略掉的数量要说出来：{msg}");
        assert_eq!(msg.matches("调用失败").count(), 3, "只举三个");
    }

    /// 重试时 `chunks_for_extraction` 只取还没抽的块，所以分母是**本轮**的数，
    /// 不是文档总块数。措辞里说清楚，别让人以为文档只有这么几块。
    #[test]
    fn the_denominator_is_this_rounds_chunks_not_the_document() {
        let msg = incomplete_reason(&[(2, "x".into())], 3).unwrap();
        assert!(msg.starts_with("本轮 3 个分块"), "{msg}");
    }

    #[tokio::test]
    async fn namesake_handles_keep_fact_attribution_and_a_fresh_handle_gets_c() -> anyhow::Result<()>
    {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(());
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'handle-runtime-test')")
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'handle-runtime-test')",
        )
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'handle-runtime-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
        let person = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'person', 'Person')",
        )
        .bind(person)
        .bind(kb)
        .execute(&pool)
        .await?;

        let run = async {
            let mut response_claims = HashMap::new();
            let mut document_claims = HashMap::new();
            let mut bare_cache = HashMap::new();
            let (mut needs_adjudication, mut human_reviews) = (false, false);
            let a = resolve_handle(
                &pool,
                kb,
                Some(person),
                "Zhang Wei",
                None,
                None,
                &mut response_claims,
                &mut document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;
            let b = resolve_handle(
                &pool,
                kb,
                Some(person),
                "Zhang Wei",
                None,
                None,
                &mut response_claims,
                &mut document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;
            assert_ne!(a, b);
            assert!(human_reviews);
            assert!(
                !needs_adjudication,
                "namesakes must not wake the LLM worker"
            );

            // 同一回复里两个句柄各绑各的实体：事实跟着句柄走，不跟着名字走
            let refs = HashMap::from([("e1", a), ("e2", b)]);
            let (finance, platform) = (Uuid::now_v7(), Uuid::now_v7());
            let works_at = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'works_at', 'works at')",
            )
            .bind(works_at)
            .bind(kb)
            .execute(&pool)
            .await?;
            for (id, name) in [(finance, "Finance"), (platform, "Platform Engineering")] {
                sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
                    .bind(id)
                    .bind(kb)
                    .bind(name)
                    .execute(&pool)
                    .await?;
            }
            for (handle, object) in [("e1", finance), ("e2", platform)] {
                let subject = refs[handle];
                utopia_store::graph::insert_fact(
                    &pool,
                    kb,
                    subject,
                    Some(works_at),
                    object,
                    utopia_store::graph::Validity::default(),
                    0.9,
                )
                .await?;
            }
            let pairs: Vec<(Uuid, Uuid)> = sqlx::query_as(
                // 只看边：实体身上还有名字事实（0041），那些没有宾语实体
                "SELECT subject_id, object_id FROM facts
                  WHERE kb_id = $1 AND object_id IS NOT NULL ORDER BY subject_id",
            )
            .bind(kb)
            .fetch_all(&pool)
            .await?;
            assert!(pairs.contains(&(a, finance)));
            assert!(pairs.contains(&(b, platform)));
            assert!(!pairs.contains(&(a, platform)));
            assert!(!pairs.contains(&(b, finance)));
            utopia_store::resolution::refresh_disambiguators(&pool, kb, "Zhang Wei").await?;
            let labels: Vec<(Uuid, Option<String>)> = sqlx::query_as(
                "SELECT id, disambiguator FROM entities WHERE id = ANY($1) ORDER BY id",
            )
            .bind(vec![a, b])
            .fetch_all(&pool)
            .await?;
            assert!(labels.contains(&(a, Some("Finance".to_string()))));
            assert!(labels.contains(&(b, Some("Platform Engineering".to_string()))));

            // 后一次回复给同一个名字开了一个新句柄，没挑 A 或 B：文本里没有挑的依据，
            // 不许猜。事实落到一个文档级的 C 上，对 A、对 B 各一条人工审核对
            let mut later_response_claims = HashMap::new();
            let c = resolve_handle(
                &pool,
                kb,
                Some(person),
                "Zhang Wei",
                None,
                None,
                &mut later_response_claims,
                &mut document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;
            assert_ne!(c, a);
            assert_ne!(c, b);
            utopia_store::graph::insert_fact(
                &pool,
                kb,
                c,
                Some(works_at),
                finance,
                utopia_store::graph::Validity::default(),
                0.9,
            )
            .await?;
            // 再往后的回复再开一个新句柄，仍是那个 C：一个新拼写不是猜 A 或 B 的许可
            let mut another_response_claims = HashMap::new();
            let c_again = resolve_handle(
                &pool,
                kb,
                None,
                "Zhang Wei",
                None,
                None,
                &mut another_response_claims,
                &mut document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;
            assert_eq!(
                c_again, c,
                "a later response cannot evade A/B ambiguity by inventing a new e-handle"
            );
            utopia_store::graph::insert_fact(
                &pool,
                kb,
                c_again,
                Some(works_at),
                platform,
                utopia_store::graph::Validity::default(),
                0.9,
            )
            .await?;
            let c_objects: Vec<Uuid> = sqlx::query_scalar(
                "SELECT object_id FROM facts
                  WHERE kb_id = $1 AND subject_id = $2 AND object_id IS NOT NULL ORDER BY object_id",
            )
            .bind(kb)
            .bind(c)
            .fetch_all(&pool)
            .await?;
            assert_eq!(c_objects.len(), 2);
            assert!(c_objects.contains(&finance));
            assert!(c_objects.contains(&platform));

            let reviews = utopia_store::resolution::list_reviews(
                &pool,
                kb,
                utopia_store::resolution::TypeFilter::Any,
                10,
                0,
            )
            .await?;
            assert_eq!(reviews.len(), 3, "A/B, C/A and C/B");
            assert!(reviews.iter().all(|review| review.stage == "human"));
            assert!(
                utopia_store::resolution::pending_adjudications(&pool, kb, 10)
                    .await?
                    .is_empty()
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;

        let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(org)
            .execute(&pool)
            .await;
        run
    }

    /// #270：跨文档的同名并列——不靠句柄，靠画像相似度。库里已有两个「张伟」，画像质心
    /// 一模一样（同一 chunk 播的种）。新提及对两人打出同一个分。旧路径在最高分 ≥ SIM_ATTACH
    /// 时静默 attach 到先遇到的那个；修好之后走句柄这条路要：新建第三个实体、两条**人工**
    /// 审核对、且绝不唤醒 LLM 裁决器（否则两条几乎相同的画像会被自动并掉，正是要防的）。
    #[tokio::test]
    async fn a_profile_tie_across_documents_files_human_reviews_not_an_attach() -> anyhow::Result<()>
    {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(());
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'profile-tie-test')")
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'profile-tie-test')",
        )
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'profile-tie-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
        let person = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'person', 'Person')",
        )
        .bind(person)
        .bind(kb)
        .execute(&pool)
        .await?;

        let run = async {
            // 两个同名的人，画像向量完全相同
            let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
            for id in [a, b] {
                sqlx::query(
                    "INSERT INTO entities
                        (id, kb_id, type_id, canonical_name, profile_embedding, profile_n)
                     VALUES ($1, $2, $3, 'Zhang Wei', '[1,0,0]'::vector, 1)",
                )
                .bind(id)
                .bind(kb)
                .bind(person)
                .execute(&pool)
                .await?;
            }

            // 与两人画像都一致的上下文：打出的余弦相同 → 分不开
            let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
            let mut response_claims = HashMap::new();
            let mut document_claims = HashMap::new();
            let mut bare_cache = HashMap::new();
            let (mut needs_adjudication, mut human_reviews) = (false, false);
            let c = resolve_handle(
                &pool,
                kb,
                Some(person),
                "Zhang Wei",
                Some(&ctx),
                None,
                &mut response_claims,
                &mut document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;

            assert_ne!(c, a, "画像并列不该 attach 到 A——那是候选顺序掷出的硬币");
            assert_ne!(c, b, "画像并列不该 attach 到 B——那是候选顺序掷出的硬币");
            assert!(
                !needs_adjudication,
                "同名并列只能等人裁，绝不该唤醒 LLM 裁决器"
            );

            let reviews = utopia_store::resolution::list_reviews(
                &pool,
                kb,
                utopia_store::resolution::TypeFilter::Any,
                10,
                0,
            )
            .await?;
            assert_eq!(reviews.len(), 2, "对 A、对 B 各一条审核对");
            assert!(
                reviews.iter().all(|review| review.stage == "human"),
                "同名并列的审核对必须是人工阶段"
            );
            assert!(
                utopia_store::resolution::pending_adjudications(&pool, kb, 10)
                    .await?
                    .is_empty(),
                "人工审核对绝不能落进批量裁决队列，否则又会被自动并掉"
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;

        let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(org)
            .execute(&pool)
            .await;
        run
    }
}
