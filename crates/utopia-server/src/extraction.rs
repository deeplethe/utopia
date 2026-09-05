//! 图谱抽取任务：逐分块调用 LLM → 实体消解 v2 → 事实 + 证据写入账本。
//! 与摄入管道分离（两段式可用）：索引完成即可搜可问，抽取慢慢跑。
//! 消解灰区只入审核队列并触发独立的攒批裁决任务——LLM 裁决永不阻塞本任务。

use crate::llm_util;
use crate::predicate_match::PredicateIndex;
use crate::state::AppState;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
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

/// 抽取的 chat 调用，**限流会退避重试**。
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
async fn chat_retrying_rate_limits(
    state: &AppState,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
    messages: &[utopia_llm::ChatMessage],
) -> anyhow::Result<String> {
    let mut backoff = Duration::from_secs(2);
    for attempt in 1..=RATE_LIMIT_TRIES {
        // 许可只包住调用本身，出了这个块就还回去
        let outcome = {
            let _permit = llm_util::acquire_chat(state, settings).await;
            client.chat(messages).await
        };
        let err = match outcome {
            Ok(reply) => return Ok(reply),
            Err(e) => e,
        };
        let Some(hit) = utopia_llm::rate_limited(&err) else {
            return Err(err);
        };
        if attempt == RATE_LIMIT_TRIES {
            return Err(err.context(format!("限流退避 {RATE_LIMIT_TRIES} 次仍未通过")));
        }
        let delay = jitter(hit.retry_after.unwrap_or(backoff).min(RATE_LIMIT_CAP));
        tracing::warn!(
            attempt,
            delay_ms = delay.as_millis() as u64,
            from_header = hit.retry_after.is_some(),
            "端点限流，退避后重试"
        );
        tokio::time::sleep(delay).await;
        backoff = (backoff * 2).min(RATE_LIMIT_CAP);
    }
    unreachable!("循环内必定 return")
}

const MIN_CONFIDENCE: f32 = 0.6;

/// 这串字**是不是一个东西的名字**。
///
/// 判据是**词数**不是字符数。字符数分不开真假：
/// `US District Court for the Northern District of California`（57 字符）是真实体，
/// 而 `removal was driven by growing discontent and distrust with Altman`（65 字符）
/// 是一整个从句——两者字符数相近，词数也相近（9 vs 10），但后者带着**限定动词**。
///
/// 所以两条一起看：词数封顶挡住长句，而**句中的限定动词**挡住那些不长的从句。
/// 机构名会长（"US District Court for the Northern District of California"），
/// 但不会出现 "was driven by"、"showed"、"giving off" 这种谓语。
///
/// 上限取 12 个词：实测真实体里最长的机构名是 9 个词，留三个词的余量。
/// 而被挡下的那些平均 14 个词。
const MAX_NAME_WORDS: usize = 12;

/// 句子里的谓语标志。**只列限定形式**——`used`、`flying` 这类分词在名词短语里
/// 完全正常（"equipment used by X"），列进去会误伤真实体。
const CLAUSE_MARKERS: &[&str] = &[
    "was", "were", "is", "are", "has", "have", "had", "will", "would", "showed", "said", "says",
    "became", "went", "came", "did", "does", "gave", "took", "made",
];

/// 情态动词：英语里**封闭类**的限定形式——不像 `fell`、`hits` 那样开放无边，也不会
/// 出现在名词短语里。`may`（月份、人名）与 `can`（容器）除外，它们兼作名词（#193）
const MODALS: &[&str] = &["could", "should", "might", "must", "shall"];

/// 结构信号：不靠词表也看得出的「这是一句话」（#193）。
///
/// 动词表挡不住新语料——英语有上千个限定形式，`could`、`fell`、`hits` 都不在那 19 个
/// 词里，而且第一个例句恰好 12 个词，卡在上限上。结构信号迁移得动：
/// 1. **句号结尾**：末词是小写词并以句号收尾（`… as a whole.`）。专名缩写 `Inc.` /
///    `Co.` 是大写开头，不误伤。
/// 2. **情态动词**：封闭类，见 [`MODALS`]。
///
/// 这两条**当场拒绝**——它们和词数上限一样是结构判据，不是又一份词表。
/// `words` 是小写化、去标点的词，`raw_last` 是保留原样的末词
fn reads_like_a_sentence(words: &[String], raw_last: Option<&str>) -> bool {
    if words.len() > 2 && words.iter().any(|w| MODALS.contains(&w.as_str())) {
        return true;
    }
    if let Some(stem) = raw_last.and_then(|w| w.strip_suffix('.')) {
        if stem.chars().count() >= 3 && stem.chars().all(|c| c.is_lowercase()) {
            return true;
        }
    }
    false
}

/// 弱信号：像从句，但不敢当场拒绝（#193）。
///
/// 守卫的样本全部来自一份语料，换一份就漏——换规则之前先要一份**跨语料的标注集**。
/// 命中只记 `clause_suspect`（例句进 `extraction_drops`），实体照常落库；攒够两份语料的
/// 样本再决定哪条升成硬规则。返回的是信号名，作为记录的 detail
fn clause_suspect(name: &str) -> Option<&'static str> {
    let words: Vec<String> = name
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    // 限定词起头的长串：「The removal … industry as a whole」；机构名也会长，但
    // 通常不以 the 起头（US District Court …），起头的（The New York Times）不长
    if words.len() >= 8 && matches!(words[0].as_str(), "a" | "an" | "the") {
        return Some("determiner_opens_a_long_string");
    }
    // 句中的关系词 / 从属连词：「the committee that reviewed …」
    if words.len() >= 5
        && words
            .iter()
            .skip(1)
            .any(|w| matches!(w.as_str(), "that" | "which" | "who" | "because" | "while"))
    {
        return Some("relative_or_subordinate_clause");
    }
    None
}

fn is_entity_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return false;
    }
    let raw: Vec<&str> = name.split_whitespace().collect();
    let words: Vec<String> = raw
        .iter()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    if words.len() > MAX_NAME_WORDS {
        return false;
    }
    // 一个词的名字不可能是从句，别让 "Is" 这种专名被误伤
    if words.len() > 2 && words.iter().any(|w| CLAUSE_MARKERS.contains(&w.as_str())) {
        return false;
    }
    if reads_like_a_sentence(&words, raw.last().copied()) {
        return false;
    }
    // 部分格：`745 of OpenAI's 770 employees` 是一个数量描述，不是一个东西。
    // 它没有限定动词，词数也不多，上面两条都接不住它。
    //
    // 判据收得很窄——**首词是纯数字且第二词是 of**。真实体里以数字开头的
    //（`3M`、`7-Eleven`、`23andMe`）首词不是纯数字；`2023 Nobel Prize` 首词是纯数字，
    // 但第二个词不是 `of`。宽一格就会误伤它们。
    if words.len() >= 3
        && words[1] == "of"
        && !words[0].is_empty()
        && words[0].chars().all(|c| c.is_ascii_digit())
    {
        return false;
    }
    true
}

/// 记一条丢弃信号。抽取器有七处 `continue`，每一处都是"事实抽出来了、被挡掉、
/// 什么都不说"。信号写失败绝不能带垮整篇文档的抽取，所以这里吞掉错误。
async fn drop_signal(
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
fn incomplete_reason(unextracted: &[(i32, String)], attempted: usize) -> Option<String> {
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

/// 自动扩本体的唯一入队点。**成功与失败两条路都要走到它。**
///
/// 开关在这里重读，而不是沿用调用方手上那份：失败路径压根没加载过 kb，
/// 而成功路径那份是文档**开抽时**读的——一篇 73 块的文档要跑一个多小时，
/// 期间有人在设置里关掉了开关，沿用旧值就是拿一小时前的意图办事。
async fn enqueue_bootstrap(state: &AppState, kb_id: Uuid) -> anyhow::Result<()> {
    if !utopia_store::kbs::get(&state.pool, kb_id)
        .await?
        .auto_extend_ontology
    {
        return Ok(());
    }
    if !utopia_store::documents::extraction_idle(&state.pool, kb_id).await? {
        return Ok(());
    }
    utopia_store::jobs::enqueue(
        &state.pool,
        "bootstrap_ontology",
        serde_json::json!({ "kb_id": kb_id }),
    )
    .await?;
    Ok(())
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
                // **失败也要触发自动扩本体。**
                //
                // 入队从前只写在成功路径上，于是这一串会把知识库永久卡住：
                // 前 14 篇成功（每篇都看到还有别的在飞，不触发），第 15 篇重试
                // 耗尽变 failed —— 这时 extraction_idle 恰好为真（failed 不算
                // queued/extracting），可**再没有任何一篇文档会完成来做这次检查**。
                // 结果是提案堆在池子里、本体永远停在种子那几个关系、半张图永远是
                // 兜底谓词，而界面上没有任何东西说这件事发生过。
                //
                // 任务本身幂等且会重查开关与门槛，所以这里多入队一次是安全的。
                let _ = enqueue_bootstrap(state, doc.kb_id).await;
            }
            Err(e)
        }
    }
}

/// mention → 实体 id。同一文档内同名同类型直接复用（单文档语境里罕有同名不同人，
/// 也把消解调用摊薄到每个名字一次）；跨文档歧义由 resolve_mention 的画像比对处理。
#[allow(clippy::too_many_arguments)]
async fn resolve(
    pool: &PgPool,
    kb_id: Uuid,
    // None = 模型给的类型不在本体里，或这个库根本还没有类（0009）
    type_id: Option<Uuid>,
    name: &str,
    ctx: Option<&[f32]>,
    // 本次提及所在分块的原文：画像分不开同名候选时的事实旁证（#331）
    text: Option<&str>,
    doc_cache: &mut HashMap<(Option<Uuid>, String), Uuid>,
    needs_adjudication: &mut bool,
) -> anyhow::Result<Uuid> {
    let key = (
        type_id,
        utopia_store::resolution::normalize_name(name).to_lowercase(),
    );
    if let Some(id) = doc_cache.get(&key) {
        return Ok(*id);
    }
    let id = resolve_uncached(
        pool,
        kb_id,
        type_id,
        name,
        ctx,
        text,
        &[],
        needs_adjudication,
    )
    .await?;
    doc_cache.insert(key, id);
    Ok(id)
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

#[derive(Clone, Copy)]
struct BoundEntity {
    id: Uuid,
    type_id: Option<Uuid>,
}

fn referenced_entity(
    ref_entities: &HashMap<String, BoundEntity>,
    reference: &str,
) -> Option<BoundEntity> {
    ref_entities.get(reference.trim()).copied()
}

#[derive(Debug, PartialEq, Eq)]
enum NoRefNameBinding {
    Legacy(Uuid),
    AmbiguousHandled,
    Missing,
}

fn no_ref_name_binding(
    entity_ids: &HashMap<String, Uuid>,
    handled_by_name: &HashMap<String, Vec<Uuid>>,
    name: &str,
) -> NoRefNameBinding {
    let normalized = utopia_store::resolution::normalize_name(name).to_lowercase();
    if handled_by_name
        .get(&normalized)
        .is_some_and(|ids| ids.len() > 1)
    {
        NoRefNameBinding::AmbiguousHandled
    } else {
        entity_ids
            .get(name)
            .copied()
            .map(NoRefNameBinding::Legacy)
            .unwrap_or(NoRefNameBinding::Missing)
    }
}

#[allow(clippy::too_many_arguments)]
async fn resolve_handle(
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

#[allow(clippy::too_many_arguments)]
async fn resolve_bare(
    pool: &PgPool,
    kb_id: Uuid,
    type_id: Option<Uuid>,
    name: &str,
    ctx: Option<&[f32]>,
    text: Option<&str>,
    doc_cache: &mut HashMap<(Option<Uuid>, String), Uuid>,
    handled_by_name: &HashMap<String, Vec<Uuid>>,
    ambiguous_bare_cache: &mut HashMap<String, Uuid>,
    needs_adjudication: &mut bool,
    human_reviews_found: &mut bool,
) -> anyhow::Result<Uuid> {
    let normalized = utopia_store::resolution::normalize_name(name).to_lowercase();
    let claims = handled_by_name
        .get(&normalized)
        .filter(|ids| ids.len() > 1)
        .cloned();
    let Some(claims) = claims else {
        return resolve(
            pool,
            kb_id,
            type_id,
            name,
            ctx,
            text,
            doc_cache,
            needs_adjudication,
        )
        .await;
    };
    if let Some(id) = ambiguous_bare_cache.get(&normalized) {
        return Ok(*id);
    }

    // The text supplies no evidence for choosing among the handled namesakes. Preserve the
    // fact on one document-scoped provisional entity and expose every possible identity link
    // to a person. Subsequent bare mentions in this document reuse this provisional entity.
    let id = resolve_uncached(
        pool,
        kb_id,
        type_id,
        name,
        ctx,
        text,
        &claims,
        needs_adjudication,
    )
    .await?;
    if create_namesake_reviews(pool, kb_id, id, &claims).await? {
        *human_reviews_found = true;
    }
    ambiguous_bare_cache.insert(normalized, id);
    Ok(id)
}

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

    // 所有权凭证：重抽会自增 epoch，本任务据此察觉自己已被接管（见分块循环）
    let my_epoch = utopia_store::documents::extract_epoch(&state.pool, document_id).await?;
    utopia_store::documents::set_graph_status(&state.pool, document_id, "extracting").await?;
    state.emit_document(doc.kb_id, document_id);
    let etypes = utopia_store::graph::entity_types(&state.pool, doc.kb_id).await?;
    // 这一轮落过的事实（新建或重复观察）：结尾对它们跑一遍签名检查
    let mut touched_facts: Vec<Uuid> = Vec::new();
    let rtypes = utopia_store::graph::relation_types(&state.pool, doc.kb_id).await?;
    // 关系与属性分道：属性走字面值通道，不进关系清单。
    //
    // **本体里没有对应关系时就没有谓词**（见 `facts.predicate_id`）。原词落进
    // fact_evidence.proposed_predicate，显示时由 fact_surface_predicate() 取回。
    //
    // 从前这里是一个叫 related_to 的兜底关系，且刻意不列给模型——它摆进提示词就成了
    // 逃生舱，模型读到说不清的关系时不去写原文说法，直接挑这个万能选项。
    // 现在它连行都没有了，逃生舱和"记得别列它"这两件事一起消失。
    //
    // **这两件事必须一起做，缺一件比都不做更糟。** 只删库里的行不够，还要删
    // `DEFAULT_RELATION_TYPES` 里的种子——而这里的排除过滤已经跟着删了。于是
    // `ensure_default_ontology` 七分钟后把行种回来，`related_to` 第一次被**列进
    // 提示词给模型看**。0001 量过：359 次使用里 321 次是模型从清单上挑的。
    // 谁要往种子表里加回一个兜底关系，先看 0010——不过那张表现在已经没有了
    // （`#128`），连播种函数一起退场。
    let type_key_by_id: HashMap<Uuid, &str> =
        etypes.iter().map(|t| (t.id, t.key.as_str())).collect();
    let attr_meta: HashMap<&str, &utopia_core::models::RelationType> = rtypes
        .iter()
        .filter(|r| r.kind == "attribute")
        .map(|r| (r.key.as_str(), r))
        .collect();
    let type_ids: HashMap<&str, Uuid> = etypes.iter().map(|t| (t.key.as_str(), t.id)).collect();
    let rel_ids: HashMap<&str, Uuid> = rtypes.iter().map(|r| (r.key.as_str(), r.id)).collect();
    // 模型说出的谓词往本体已有关系上落：写法、时态、被动都对齐（见 predicate_match）。
    // 没有它的时候，`produces` 明明在词表里，模型写 `produced_by` 就被降级扔了
    let pred_index = PredicateIndex::build(&rtypes);
    // 「本体认不认识这个说法」——字面值那一档与关系那一档必须用同一个判据。
    // 分开写的话，模糊匹配得上的谓词会先被字面值那一档当成属性分流走，
    // 同一个词在两条路上得到相反的回答
    let known_predicate = |p: &str| rel_ids.contains_key(p) || pred_index.lookup(p).is_some();
    // 时态对账只对带唯一性约束的状态关系生效（本体元数据）：(functional, inverse_functional, temporal)
    let rel_meta: HashMap<Uuid, (bool, bool, String)> = rtypes
        .iter()
        .map(|r| {
            (
                r.id,
                (r.functional, r.inverse_functional, r.temporal.clone()),
            )
        })
        .collect();
    let type_parents: HashMap<Uuid, &[Uuid]> = etypes
        .iter()
        .map(|t| (t.id, t.parents.as_slice()))
        .collect();

    // **本体全铺还是按分块检索。**
    //
    // 全铺是今天的行为，小本体下它对且便宜：40 个类约 2k 字符，检索反而是多余的
    // 往返。大本体下它是灾难——schema.org 实测每个分块 108k tokens，而同一份语料
    // 只给种子类时抽到 25 个实体、给全量时只剩 18 个。**多给的那 959 个类
    // 吃掉了 7 个实体。**
    //
    // 所以按预算切换：装得下就全铺，装不下就每块检索。判据量的是**实际要排的
    // 那段字**（build_lists 自己数），不是另写一个估算公式——公式会跟排版分叉。
    let full = build_lists(&etypes, &rtypes, None, None);
    let budget = utopia_store::access::ontology_prompt_budget(&state.pool).await?;
    let retrieve_per_chunk = full.chars() > budget;
    if retrieve_per_chunk {
        tracing::info!(
            %document_id, chars = full.chars(), budget,
            classes = etypes.len(),
            "本体超出提示词预算，改为按分块检索候选"
        );
    }
    // 内置类恒在：检索漏掉的分块仍要有地方落脚，否则模型无类可选
    let seed_classes: HashSet<Uuid> = etypes.iter().filter(|t| t.builtin).map(|t| t.id).collect();

    // 属性 domain 允许子类：主语类型沿 parent 链上溯命中 domain 即可
    // 沿 subClassOf 上溯。**广度优先 + 访问集**，不是单链循环：
    // 一个类可以有多个父（FOAF 的 Person 同时是 Agent 与 SpatialThing），
    // 而菱形继承会从两条路到达同一个祖先，没有访问集就会重复展开。
    //
    // 深度上限换成了访问集：写入侧 set_parents 已经查环，这里再靠"最多走十层"
    // 兜底既挡不住宽的图，也会悄悄放过深的层级。
    let type_matches_domain = |ty: Uuid, domain: Uuid| -> bool {
        let mut seen: HashSet<Uuid> = HashSet::new();
        let mut queue = vec![ty];
        while let Some(cur) = queue.pop() {
            if cur == domain {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            if let Some(ps) = type_parents.get(&cur) {
                queue.extend(ps.iter().copied());
            }
        }
        false
    };
    // 本轮要从头讲一遍这篇文档的故事，旧信号先清掉（重抽自动作数）
    let _ = utopia_store::extraction_drops::clear_for_document(&state.pool, document_id).await;

    // **记忆抽出的事实先等人点头**（0015）。一句 remember 一次一句、人就在对话里，
    // 确认成本最低的时刻就是说完那句话的时候；而批量摄入一次上万条，逐条确认
    // 不可能，那条路仍旧乐观写入 + 事后审阅。判据只有一个：这篇是不是记忆日志。
    // 实体照常消解并创建——`pending_facts.subject_id` 是外键，这是 0018 定下的取舍
    let await_nod = utopia_store::memory::is_memory_document(&state.pool, document_id).await?;
    let mut pending_count = 0usize;

    let doc_time = doc.doc_time.map(|t| t.format("%Y-%m-%d").to_string());
    let chunks = utopia_store::documents::chunks_for_extraction(&state.pool, document_id).await?;

    let mut doc_cache: HashMap<(Option<Uuid>, String), Uuid> = HashMap::new();
    // Identities introduced through handles, grouped only for detecting document-local
    // namesake ambiguity. A later bare mention gets its own provisional entity instead of
    // guessing among this group.
    let mut handled_by_name: HashMap<String, Vec<Uuid>> = HashMap::new();
    let mut ambiguous_bare_cache: HashMap<String, Uuid> = HashMap::new();
    let mut touched_names: HashSet<String> = HashSet::new();
    // 本文档已经认下的实体，按首次出现排序，送进后续分块的提示词。
    //
    // **按 entity_id 去重，不按名字**：第 3 块写"上海研究院"若消解到了第 1 块的
    // "星云科技上海研究院"，那它不该以第二个名字进清单——清单里每个实体只有
    // 一个展示形态，就是这篇文档第一次用的那个。中文里全称先出现，所以这也是较全的那个。
    let mut doc_entities: Vec<(Uuid, String, String)> = Vec::new();
    // 整块没抽成的：(seq, 原因)。收尾时据此拒绝把这篇文档标成 done
    let mut unextracted: Vec<(i32, String)> = Vec::new();
    let mut needs_adjudication = false;
    let mut human_reviews_found = false;
    let mut conflicts_found = false;
    let mut fact_count = 0usize;
    // 不设分块上限：静默截断等于丢知识，长文档的成本由部署者自己权衡
    // （成本优化走 prompt 前缀缓存与更新时 chunk 级跳过，而非丢数据）
    for chunk in chunks.iter() {
        // 被接管则安静退场：不写 failed、不碰状态，舞台留给新任务。
        // 检查放在调用 LLM 之前——取消粒度即一个分块，不必等整篇跑完
        if utopia_store::documents::extract_epoch(&state.pool, document_id).await? != my_epoch {
            tracing::info!(%document_id, "抽取任务已被新一轮接管，退出");
            return Ok(());
        }
        let ctx: Option<&[f32]> = chunk.embedding.as_ref().map(|v| v.as_slice());
        // 本体装得下就用全量那份；装不下就拿**这一块自己的向量**检索候选。
        // 向量是现成的——实体消解本来就在用它（上面那个 ctx），检索一次
        // 嵌入都不用加。检索不出来（没配嵌入模型、或这块没向量）就退回全量：
        // 提示词大是慢，没有类可选是抽不出东西
        let lists = if retrieve_per_chunk {
            match ctx {
                Some(v) => chunk_lists(state, doc.kb_id, v, &etypes, &rtypes, &seed_classes)
                    .await
                    .unwrap_or(None),
                None => None,
            }
        } else {
            None
        };
        let lists = lists.as_ref().unwrap_or(&full);
        let known: Vec<utopia_extract::KnownEntity> = doc_entities
            .iter()
            .enumerate()
            .map(|(index, (_, type_key, name))| utopia_extract::KnownEntity {
                handle: format!("k{}", index + 1),
                type_key: type_key.clone(),
                name: name.clone(),
            })
            .collect();
        let messages = utopia_extract::build_messages(
            &lists.types,
            &lists.relations,
            &lists.attributes,
            doc_time.as_deref(),
            &doc.filename,
            &known,
            &chunk.text,
        );
        // 这两处 continue 跳过的是**整个分块**——它一条事实都没产出。
        // 记下来，收尾时据此决定这篇文档算不算抽完（见循环之后）
        let reply = match chat_retrying_rate_limits(state, &settings, &client, &messages).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%document_id, seq = chunk.seq, error = %e, "抽取调用失败，跳过该分块");
                unextracted.push((chunk.seq, format!("调用失败：{e}")));
                continue;
            }
        };
        let extraction = match utopia_extract::parse_response(&reply) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!(%document_id, seq = chunk.seq, error = %e, "抽取结果解析失败，跳过该分块");
                unextracted.push((chunk.seq, format!("结果解析失败：{e}")));
                continue;
            }
        };
        // **跳过了什么必须说出来。** 逐项解析救回了整块，但被跳过的那几条
        // 如果不落信号，就成了另一种「部分抽取报告成完成」（#108 修过一次）
        if extraction.truncated {
            drop_signal(
                state,
                doc.kb_id,
                document_id,
                utopia_store::extraction_drops::reason::TRUNCATED_REPLY,
                &format!("分块 #{} 的输出被截断", chunk.seq),
                None,
            )
            .await;
        }
        let skipped = extraction.skipped_entities + extraction.skipped_facts;
        if skipped > 0 {
            tracing::warn!(
                %document_id,
                seq = chunk.seq,
                entities = extraction.skipped_entities,
                facts = extraction.skipped_facts,
                "跳过了结构不合的条目"
            );
            drop_signal(
                state,
                doc.kb_id,
                document_id,
                utopia_store::extraction_drops::reason::MALFORMED_ITEM,
                &format!(
                    "分块 #{} 跳过 {} 个实体 / {} 条事实",
                    chunk.seq, extraction.skipped_entities, extraction.skipped_facts
                ),
                None,
            )
            .await;
        }

        // 实体消解：名称 → 实体 id（本分块的事实按原文名字连线）
        let mut entity_ids: HashMap<String, Uuid> = HashMap::new();
        // 名称 → 声明类型（属性 domain 校验用：salary 不能挂在 Organization 上）
        let mut entity_type_of: HashMap<String, Option<Uuid>> = HashMap::new();
        let mut ref_entities: HashMap<String, BoundEntity> = doc_entities
            .iter()
            .enumerate()
            .map(|(index, (id, type_key, _))| {
                (
                    format!("k{}", index + 1),
                    BoundEntity {
                        id: *id,
                        type_id: type_ids.get(type_key.as_str()).copied(),
                    },
                )
            })
            .collect();
        let mut response_claims: HashMap<String, Vec<Uuid>> = HashMap::new();

        // Resolve handled definitions first. This matters for a mixed response: a later
        // handle-less same-name item must see the ambiguity rather than winning by array order.
        for e in extraction
            .entities
            .iter()
            .filter(|e| e.local_id.is_some())
            .chain(extraction.entities.iter().filter(|e| e.local_id.is_none()))
        {
            let name = e.name.trim();
            if !is_entity_name(name) {
                // 从前这里是静默 `continue`——正是 `drop_signal` 当初为之而建的
                // 那种"抽出来了、被挡掉、什么都不说"
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::NOT_AN_ENTITY_NAME,
                    &e.type_key,
                    Some(name),
                )
                .await;
                continue;
            }
            // 守卫放行、结构却像从句：只记不挡（#193 先攒标注集）
            if let Some(signal) = clause_suspect(name) {
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::CLAUSE_SUSPECT,
                    signal,
                    Some(name),
                )
                .await;
            }
            if let Some(handle) = e.local_id.as_deref().map(str::trim) {
                if ref_entities.contains_key(handle) {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::MALFORMED_ITEM,
                        "local_id collides with a known handle",
                        Some(handle),
                    )
                    .await;
                    continue;
                }
            }
            // 降级时记住模型提议的那个词：本体装不下不等于它说错了。
            // 只留计数的话，日后想加 model 类就找不出那 43 个实体——它们混在
            // concept 里面，唯一的出路是整库重抽
            let mut proposed: Option<&str> = None;
            let type_id = match type_ids.get(e.type_key.as_str()) {
                Some(id) => Some(*id),
                None => {
                    // 白名单外类型：**留空**，并记入未匹配统计（本体扩展的信号）。
                    //
                    // 从前这里降级到 concept 那行哨兵。现在「还没判出来」就是
                    // `type_id IS NULL`（0009）——实体照常建、事实照常落、证据照常有，
                    // 只是暂时没有类型。之后装一个包再跑类型消解，它会被重新分配
                    let _ = utopia_store::ontology::record_miss(
                        &state.pool,
                        doc.kb_id,
                        "entity_type",
                        &e.type_key,
                        Some(name),
                    )
                    .await;
                    proposed = Some(e.type_key.as_str());
                    None
                }
            };
            let normalized = utopia_store::resolution::normalize_name(name).to_lowercase();
            touched_names.insert(normalized.clone());
            let id = if let Some(handle) = e.local_id.as_deref() {
                let handle = handle.trim();
                let id = resolve_handle(
                    &state.pool,
                    doc.kb_id,
                    type_id,
                    name,
                    ctx,
                    Some(&chunk.text),
                    &mut response_claims,
                    &mut handled_by_name,
                    &mut ambiguous_bare_cache,
                    &mut needs_adjudication,
                    &mut human_reviews_found,
                )
                .await?;
                ref_entities.insert(handle.to_string(), BoundEntity { id, type_id });
                id
            } else {
                resolve_bare(
                    &state.pool,
                    doc.kb_id,
                    type_id,
                    name,
                    ctx,
                    Some(&chunk.text),
                    &mut doc_cache,
                    &handled_by_name,
                    &mut ambiguous_bare_cache,
                    &mut needs_adjudication,
                    &mut human_reviews_found,
                )
                .await?
            };
            if let Some(p) = proposed {
                let _ = utopia_store::resolution::set_proposed_type(&state.pool, id, p).await;
            }
            // 模型自己的说法。**跟 proposed_type 分开存**：那一列的含义是
            // "本体里没有"，增长回路靠它的稀有性设门槛；这一列每个实体都有。
            // 与粗类同名的不记——那不是更具体的说法，只是把清单抄了一遍
            if let Some(st) = e
                .specific_type
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter(|s| !s.eq_ignore_ascii_case(&e.type_key))
            {
                let _ = utopia_store::resolution::set_specific_type(&state.pool, id, st).await;
            }
            // 只记模型自己声明过类型的：主宾兜底那条路没有类型可依，
            // 把一个猜出来的类型放进清单等于让后续分块照着猜的抄。
            //
            // 本体装不下那个类时用模型自己的说法（proposed）：这份清单是给后文
            // 认人用的，"同一个名字别写成两个实体"才是它的活。从前这里只能写死
            // concept，反倒把几个不同的词抹平成同一个标签
            if !doc_entities.iter().any(|(eid, _, _)| *eid == id) {
                let tk = type_id
                    .and_then(|t| type_key_by_id.get(&t).copied())
                    .or(proposed)
                    .unwrap_or("?");
                doc_entities.push((id, tk.to_string(), name.to_string()));
            }
            // Legacy name maps retain their old last-write-wins behavior. Handled facts bind
            // through ref_entities; inserting handled definitions here only supports mixed
            // model output when the surface name is unambiguous.
            entity_ids.insert(name.to_string(), id);
            entity_type_of.insert(name.to_string(), type_id);
        }

        for f in &extraction.facts {
            let confidence = f.confidence.unwrap_or(0.7).clamp(0.0, 1.0);
            if confidence < MIN_CONFIDENCE {
                // 设计上的阈值，但用户同样无从知道"抽到了，只是不够自信"
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::LOW_CONFIDENCE,
                    &f.predicate,
                    Some(&format!("{} ({:.0}%)", f.subject, confidence * 100.0)),
                )
                .await;
                continue;
            }
            let from = f.valid_from.as_deref().and_then(utopia_extract::parse_time);
            let to = f.valid_to.as_deref().and_then(utopia_extract::parse_time);
            // **两端各记各的粒度**（见 `facts.valid_to_precision`）。从前一个精度列描述两个端点，
            // 于是「2020 年开始、2023-05-06 结束」这种只能共用一个值。
            //
            // 模型给的 valid_to = "unknown" 表示**原文说它结束了、但没说哪天**。
            // parse_time 解不出它（本来就不是日期），落在这里显式认掉——
            // 不认的话它退化成 None，那条事实就又变回"仍在持续"了
            let ended_unknown = f
                .valid_to
                .as_deref()
                .map(str::trim)
                .is_some_and(|v| v.eq_ignore_ascii_case(utopia_store::graph::ENDED_UNKNOWN));
            let validity = utopia_store::graph::Validity {
                from: from.map(|(t, _)| t),
                from_precision: from.map(|(_, p)| p),
                to: to.map(|(t, _)| t),
                to_precision: to
                    .map(|(_, p)| p)
                    .or(ended_unknown.then_some(utopia_store::graph::ENDED_UNKNOWN)),
                // 这次观察出自哪一天的文档（0022）：没起点的事实从它起成立，结束了
                // 不知哪天的到它为止。没有文档日期就是记下的此刻——账本能给的最好的
                attested_at: doc.doc_time,
            };

            // 属性事实：谓词命中属性 → 字面值通道。datatype 校验失败宁缺勿脏；
            // domain 校验（含子类上溯）挡住"把 salary 挂到 Organization"这类张冠李戴。
            // 模型偶尔照抄清单里的 "person.salary" 全限定名——剥掉类前缀再查一次
            let attr_hit = attr_meta.get(f.predicate.as_str()).or_else(|| {
                f.predicate
                    .rsplit_once('.')
                    .and_then(|(_, k)| attr_meta.get(k))
            });
            if let Some(attr) = attr_hit {
                let subject_name = f.subject.trim();
                // 主语没在 entities 里声明：类型不明，domain 无从校验，属性不落。
                // 关系路径遇到同样的缺失会兜底按 concept 消解——这里学不来，
                // 按 concept 解出来 domain 照样不匹配，只是从这里掉进下面那一档
                let bound = match f.subject_ref.as_deref().map(str::trim) {
                    Some(handle) => referenced_entity(&ref_entities, handle),
                    None => {
                        match no_ref_name_binding(&entity_ids, &handled_by_name, subject_name) {
                            NoRefNameBinding::Legacy(id) => entity_type_of
                                .get(subject_name)
                                .copied()
                                .map(|type_id| BoundEntity { id, type_id }),
                            NoRefNameBinding::Missing => None,
                            NoRefNameBinding::AmbiguousHandled => {
                                let type_id = entity_type_of.get(subject_name).copied().flatten();
                                touched_names.insert(
                                    utopia_store::resolution::normalize_name(subject_name)
                                        .to_lowercase(),
                                );
                                let id = resolve_bare(
                                    &state.pool,
                                    doc.kb_id,
                                    type_id,
                                    subject_name,
                                    ctx,
                                    Some(&chunk.text),
                                    &mut doc_cache,
                                    &handled_by_name,
                                    &mut ambiguous_bare_cache,
                                    &mut needs_adjudication,
                                    &mut human_reviews_found,
                                )
                                .await?;
                                Some(BoundEntity { id, type_id })
                            }
                        }
                    }
                };
                let Some(BoundEntity {
                    id: subject_id,
                    type_id: subject_type,
                }) = bound
                else {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        if f.subject_ref.is_some() {
                            utopia_store::extraction_drops::reason::MALFORMED_ITEM
                        } else {
                            utopia_store::extraction_drops::reason::SUBJECT_NOT_DECLARED
                        },
                        &attr.key,
                        Some(f.subject_ref.as_deref().unwrap_or(subject_name)),
                    )
                    .await;
                    continue;
                };
                // 不可达：store 层强制 attribute 必有 domain，且 domain 不可改，
                // 无 domain 的属性连提示词都进不去。留着是防御，不需要信号
                if attr.domains.is_empty() {
                    continue;
                }
                // **任一 domain 命中即可**：属性挂在多个类下时，主语属于其中之一就算数
                if !attr
                    .domains
                    .iter()
                    .any(|d| subject_type.is_some_and(|t| type_matches_domain(t, *d)))
                {
                    let subj_key = subject_type
                        .and_then(|t| type_key_by_id.get(&t).copied())
                        .unwrap_or("?");
                    let dom_key = attr
                        .domains
                        .iter()
                        .filter_map(|d| type_key_by_id.get(d).copied())
                        .collect::<Vec<_>>()
                        .join("|");
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::ATTR_DOMAIN_MISMATCH,
                        &format!("{}@{subj_key} (wants {dom_key})", attr.key),
                        Some(subject_name),
                    )
                    .await;
                    continue;
                }
                let raw = match (&f.value, &f.object) {
                    (Some(v), _) => v.clone(),
                    // 模型偶尔把值放进 object：宽容接住
                    (None, Some(o)) if !o.trim().is_empty() => {
                        serde_json::Value::String(o.trim().to_string())
                    }
                    _ => {
                        drop_signal(
                            state,
                            doc.kb_id,
                            document_id,
                            utopia_store::extraction_drops::reason::ATTR_NO_VALUE,
                            &attr.key,
                            Some(subject_name),
                        )
                        .await;
                        continue;
                    }
                };
                let datatype = attr.datatype.as_deref().unwrap_or("text");
                let Some(normalized) = utopia_extract::normalize_attr_value(datatype, &raw) else {
                    tracing::debug!(%document_id, attr = attr.key, ?raw, "属性值不合 datatype，跳过");
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::ATTR_DATATYPE,
                        &format!("{} ({datatype})", attr.key),
                        Some(&format!("{subject_name} → {raw}")),
                    )
                    .await;
                    continue;
                };
                let mut object_value = serde_json::json!({ "value": normalized });
                if let Some(u) = attr.unit.as_deref().filter(|u| !u.is_empty()) {
                    // 单位随事实落笔：类型上的单位以后改了，旧值仍按记录时的单位读
                    object_value["unit"] = serde_json::json!(u);
                }
                if await_nod {
                    if let utopia_store::pending::Outcome::Proposed(_) =
                        utopia_store::pending::propose(
                            &state.pool,
                            utopia_store::pending::Proposal {
                                kb_id: doc.kb_id,
                                subject_id,
                                predicate_id: Some(attr.id),
                                object_id: None,
                                object_value: Some(&object_value),
                                proposed_predicate: Some(f.predicate.as_str()),
                                validity,
                                confidence,
                                chunk_id: chunk.id,
                                proposed_by: proposer.user_id,
                                proposed_token: proposer.token_id,
                            },
                        )
                        .await?
                    {
                        pending_count += 1;
                    }
                    continue;
                }
                let (fact_id, created) = utopia_store::graph::insert_value_fact(
                    &state.pool,
                    doc.kb_id,
                    subject_id,
                    Some(attr.id),
                    &object_value,
                    validity,
                    confidence,
                )
                .await?;
                touched_facts.push(fact_id);
                // 属性谓词也留原词：模型偶尔照抄 "person.salary" 全限定名，
                // 命中的是剥掉前缀后的 key，原样是什么值得留着
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk.id,
                    f.quote.as_deref(),
                    Some(f.predicate.as_str()),
                )
                .await?;
                if !created {
                    continue;
                }
                fact_count += 1;
                // 单值属性 = functional：新值闭合旧值（属性历史由此而来）
                if attr.functional && attr.temporal == "state" {
                    let report = utopia_store::temporal::reconcile_new_fact(
                        &state.pool,
                        doc.kb_id,
                        fact_id,
                        subject_id,
                        attr.id,
                        None,
                        Some(&object_value),
                        utopia_store::temporal::Uniqueness::SubjectSide,
                        validity,
                        confidence,
                    )
                    .await?;
                    if report.conflicts > 0 {
                        conflicts_found = true;
                    }
                }
                continue;
            }

            // **词表外的字面值：既不丢，也不给它编一个实体。**
            //
            // 走到这里说明谓词既不是已知属性也还没查关系表。它带着字面值时
            // 有两种走法，从前两种都不好：
            //   value 有而 object 空 → 掉进下面的"宾语必填"，整条静默消失；
            //   object 里塞着字面值 → 按 concept 消解，凭空造出一个叫「2015」
            //     的实体，图里多一个假节点，事后再修还得改事实的形状。
            // 现在都存成 object_value 且没有谓词：值在图里、有证据、有时态，
            // 原词进 proposed_predicate，消解那一遍只需换谓词，形状已经是对的。
            //
            // object 里的东西算不算字面值，判据从严：**模型没把它声明成实体**，
            // 且**它本身解得出数字或日期**。"杭州"两条都不满足，"2015"都满足。
            // 文本值的属性（schema.org 里 323 个）在这一档仍会变成实体——
            // 那里没有可靠判据，猜错会吃掉真实体，不猜
            let literal = match (&f.value, f.object.as_deref().map(str::trim)) {
                (Some(v), None | Some("")) if !known_predicate(f.predicate.as_str()) => {
                    Some(v.clone())
                }
                (_, Some(o))
                    if !o.is_empty()
                        && !known_predicate(f.predicate.as_str())
                        && !entity_ids.contains_key(o)
                        && looks_literal(o) =>
                {
                    Some(serde_json::Value::String(o.to_string()))
                }
                _ => None,
            };
            if let Some(value) = literal {
                let subject_name = f.subject.trim();
                let Some(&subject_id) = entity_ids.get(subject_name) else {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::SUBJECT_NOT_DECLARED,
                        &f.predicate,
                        Some(subject_name),
                    )
                    .await;
                    continue;
                };
                let _ = utopia_store::ontology::record_miss(
                    &state.pool,
                    doc.kb_id,
                    "attribute_type",
                    &f.predicate,
                    Some(&format!("{subject_name} → {value}")),
                )
                .await;
                let literal = serde_json::json!({ "value": value });
                if await_nod {
                    if let utopia_store::pending::Outcome::Proposed(_) =
                        utopia_store::pending::propose(
                            &state.pool,
                            utopia_store::pending::Proposal {
                                kb_id: doc.kb_id,
                                subject_id,
                                predicate_id: None,
                                object_id: None,
                                object_value: Some(&literal),
                                proposed_predicate: Some(f.predicate.as_str()),
                                validity,
                                confidence,
                                chunk_id: chunk.id,
                                proposed_by: proposer.user_id,
                                proposed_token: proposer.token_id,
                            },
                        )
                        .await?
                    {
                        pending_count += 1;
                    }
                    continue;
                }
                let (fact_id, created) = utopia_store::graph::insert_value_fact(
                    &state.pool,
                    doc.kb_id,
                    subject_id,
                    None,
                    &literal,
                    validity,
                    confidence,
                )
                .await?;
                touched_facts.push(fact_id);
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk.id,
                    f.quote.as_deref(),
                    Some(f.predicate.as_str()),
                )
                .await?;
                if created {
                    fact_count += 1;
                }
                continue;
            }

            // 关系事实：宾语必填
            let Some(object_name) = f.object.as_deref().map(str::trim).filter(|s| !s.is_empty())
            else {
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::OBJECT_MISSING,
                    &f.predicate,
                    Some(f.subject.trim()),
                )
                .await;
                continue;
            };
            // **未声明的主宾也要过同一道判据。**
            //
            // 从前守卫只装在上面那条声明实体的路上，而这里绕过了它：模型把一整句话
            // 写进 `object`、那句话没出现在 entities 里，这里就转头把它造成了实体。
            // 实测（ai-timeline-ends × schema.org）421 个实体里 76 个无类型，
            // 最长的那个 111 字符——"thermal-imaging equipment used by volunteers
            // flying over the site showed at least 33 generators giving off heat"，
            // 那是一整个从句，不是一个东西。**守卫拦住了前门，后门是开的。**
            //
            // 这类实体的害处不止于此：它们永远匹配不到别处的任何提及，
            // 在图上是孤点（实测 59 个），还会拖累消解——每一个都要跟已有实体比一遍。
            if !is_entity_name(f.subject.trim()) || !is_entity_name(object_name) {
                drop_signal(
                    state,
                    doc.kb_id,
                    document_id,
                    utopia_store::extraction_drops::reason::NOT_AN_ENTITY_NAME,
                    &f.predicate,
                    Some(if is_entity_name(f.subject.trim()) {
                        object_name
                    } else {
                        f.subject.trim()
                    }),
                )
                .await;
                continue;
            }
            for side in [f.subject.trim(), object_name] {
                if let Some(signal) = clause_suspect(side) {
                    drop_signal(
                        state,
                        doc.kb_id,
                        document_id,
                        utopia_store::extraction_drops::reason::CLAUSE_SUSPECT,
                        signal,
                        Some(side),
                    )
                    .await;
                }
            }

            // 主宾未在 entities 中声明时先建出来（模型偶尔漏报）。没有 entities 那条
            // 记录就没有类型可依，留空即可——0009 之前这里只能塞 concept
            let subject_id = match f.subject_ref.as_deref().map(str::trim) {
                Some(handle) => match referenced_entity(&ref_entities, handle) {
                    Some(bound) => bound.id,
                    None => {
                        drop_signal(
                            state,
                            doc.kb_id,
                            document_id,
                            utopia_store::extraction_drops::reason::MALFORMED_ITEM,
                            &f.predicate,
                            Some(handle),
                        )
                        .await;
                        continue;
                    }
                },
                None => {
                    match no_ref_name_binding(&entity_ids, &handled_by_name, f.subject.trim()) {
                        NoRefNameBinding::Legacy(id) => id,
                        NoRefNameBinding::AmbiguousHandled | NoRefNameBinding::Missing => {
                            touched_names.insert(
                                utopia_store::resolution::normalize_name(f.subject.trim())
                                    .to_lowercase(),
                            );
                            resolve_bare(
                                &state.pool,
                                doc.kb_id,
                                None,
                                f.subject.trim(),
                                ctx,
                                Some(&chunk.text),
                                &mut doc_cache,
                                &handled_by_name,
                                &mut ambiguous_bare_cache,
                                &mut needs_adjudication,
                                &mut human_reviews_found,
                            )
                            .await?
                        }
                    }
                }
            };
            let object_id = match f.object_ref.as_deref().map(str::trim) {
                Some(handle) => match referenced_entity(&ref_entities, handle) {
                    Some(bound) => bound.id,
                    None => {
                        drop_signal(
                            state,
                            doc.kb_id,
                            document_id,
                            utopia_store::extraction_drops::reason::MALFORMED_ITEM,
                            &f.predicate,
                            Some(handle),
                        )
                        .await;
                        continue;
                    }
                },
                None => match no_ref_name_binding(&entity_ids, &handled_by_name, object_name) {
                    NoRefNameBinding::Legacy(id) => id,
                    NoRefNameBinding::AmbiguousHandled | NoRefNameBinding::Missing => {
                        touched_names.insert(
                            utopia_store::resolution::normalize_name(object_name).to_lowercase(),
                        );
                        resolve_bare(
                            &state.pool,
                            doc.kb_id,
                            None,
                            object_name,
                            ctx,
                            Some(&chunk.text),
                            &mut doc_cache,
                            &handled_by_name,
                            &mut ambiguous_bare_cache,
                            &mut needs_adjudication,
                            &mut human_reviews_found,
                        )
                        .await?
                    }
                },
            };
            if subject_id == object_id {
                continue;
            }
            // 先尽量落到本体已有的关系上（写法/时态/被动），**落不上才降级**为 related_to
            // 并记入未匹配统计。降级会把原意抹平成"有关联"——原词写进证据行的
            // proposed_predicate，是这条事实身上唯一还留着原意的地方（谓词消解据此映射回本体）
            let (predicate_id, swap) = match pred_index.lookup(f.predicate.as_str()) {
                Some((id, swap)) => (Some(id), swap),
                None => {
                    let _ = utopia_store::ontology::record_miss(
                        &state.pool,
                        doc.kb_id,
                        "relation_type",
                        &f.predicate,
                        Some(&format!("{} → {}", f.subject, object_name)),
                    )
                    .await;
                    // 本体里没有对应的关系 → **就是没有谓词**（见 `facts.predicate_id`）。
                    // 原意留在证据的 proposed_predicate 里，显示时取回。
                    // 从前这里落到 related_to 上，还要额外担心"兜底关系被删了"——
                    // 那条失败模式连同它的 continue 一起消失了
                    (None, false)
                }
            };
            // 被动说法命中的是同一条边的反向：`ChatGPT produced_by OpenAI` 与
            // `OpenAI produces ChatGPT` 是同一条边，存的时候要按本体的方向来，
            // 否则它跟已有的那 130 条 produces 各存各的，图上是两条相反的箭头
            let (subject_id, object_id) = if swap {
                (object_id, subject_id)
            } else {
                (subject_id, object_id)
            };

            // **主语违反 domain、而宾语符合时，按本体声明的方向把它掰正。**
            //
            // 先试过提示词，三轮都没赢：违反率从 57% 压到 35%，但压下去的全是
            // 类型判错那一半；**真·反向纹丝不动**（22.7% → 17.1% → 17.6%，
            // 后两个在噪声里）。模型看得见 `employee (organization → person)`，
            // 就是不照做——英语的 "X is an employee of Y" 太强。
            //
            // 这不是新原则：`produced_by` 命中 `produces` 时（见上面那个 `swap`）
            // 早就在自动翻转主宾了，区别只在触发条件是**措辞**还是**签名**。
            //
            // 当初反对自动对调的理由是实体类型不可靠——实测 Elon Musk 被判成
            // `researcher`。那个前提已经不成立：祖先地板与签名类恒在修好之后，
            // 同一批人判成了 `person`。而判据本身很窄——**主语违反且宾语符合**，
            // 两侧都要对上才动。
            //
            // **但绝不静默。** 掰正要留信号：0001 反对的是「用可能错的声明驱动
            // 自动动作」，而看得见、可复查、可反悔的动作不属于那一类。
            let (predicate_id, subject_id, object_id) = if let Some(pid) = predicate_id {
                // 类型**从库里的实体读**，不用抽取器手上那份 `entity_type_of`：
                // 那份只覆盖模型在这一块里声明过的实体，而宾语常是别处已存在的实体，
                // 这一块没重新声明它，于是查不到、判不了、掰不动。实测差别不小——
                // 用声明那份时反向只降到 7.0%，剩下的正是宾语类型查不到的那些
                //
                // 判断本身在 store（`ontology::judge_direction`），**与采纳共用**（#190）：
                // 写谓词的路不止这一条，守卫只装在一条上就等于没装。查不出来（库错）
                // 按没有判据处理，照原样落——宁可少掰一条，不能因为一次查询失败丢事实
                let fit = utopia_store::ontology::judge_direction(
                    &state.pool,
                    pid,
                    subject_id,
                    object_id,
                )
                .await
                .unwrap_or(utopia_store::ontology::Fit::Unchecked);
                match fit {
                    utopia_store::ontology::Fit::Swap => {
                        drop_signal(
                            state,
                            doc.kb_id,
                            document_id,
                            utopia_store::extraction_drops::reason::DIRECTION_CORRECTED,
                            &f.predicate,
                            Some(&format!(
                                "{} → {} 按签名掰正为 {} → {}",
                                f.subject,
                                f.object.as_deref().unwrap_or("?"),
                                f.object.as_deref().unwrap_or("?"),
                                f.subject
                            )),
                        )
                        .await;
                        (Some(pid), object_id, subject_id)
                    }
                    utopia_store::ontology::Fit::Neither => {
                        // **对调也不合法 → 退回没有谓词。**
                        //
                        // 这不是方向问题，是这个关系压根不适用：schema.org 的
                        // `affectedBy` 是医学检验用的，模型要表达「受……影响」时按名字
                        // 撞了上来；`amount` 属于融资工具而不是公司，模型没造那个中间
                        // 节点就把边挂到了公司上。
                        //
                        // 从前照原样落库，等于**用本体的名义说一件本体不同意的事**——
                        // 图上写着 "OpenAI affectedBy …"，读者会以为那是一条医学断言。
                        // 这是自信的错误，比空谓词严重得多。
                        //
                        // 退回空谓词不丢信息：原词落进 `fact_evidence.proposed_predicate`，
                        // 显示时由 `fact_surface_predicate()` 取回（0010）。主宾、时间、
                        // 证据全都留着，只是不再冒认一个本体关系。**诚实的沉默。**
                        drop_signal(
                            state,
                            doc.kb_id,
                            document_id,
                            utopia_store::extraction_drops::reason::DOMAIN_MISMATCH,
                            &f.predicate,
                            Some(&format!(
                                "{} — {} → 主宾都对不上，退回原文说法",
                                f.subject, f.predicate
                            )),
                        )
                        .await;
                        (None, subject_id, object_id)
                    }
                    utopia_store::ontology::Fit::Keep | utopia_store::ontology::Fit::Unchecked => {
                        (Some(pid), subject_id, object_id)
                    }
                }
            } else {
                (predicate_id, subject_id, object_id)
            };

            if await_nod {
                if let utopia_store::pending::Outcome::Proposed(_) = utopia_store::pending::propose(
                    &state.pool,
                    utopia_store::pending::Proposal {
                        kb_id: doc.kb_id,
                        subject_id,
                        predicate_id,
                        object_id: Some(object_id),
                        object_value: None,
                        proposed_predicate: Some(f.predicate.as_str()),
                        validity,
                        confidence,
                        chunk_id: chunk.id,
                        proposed_by: proposer.user_id,
                        proposed_token: proposer.token_id,
                    },
                )
                .await?
                {
                    pending_count += 1;
                }
                continue;
            }
            {
                let (fact_id, created) = utopia_store::graph::insert_fact(
                    &state.pool,
                    doc.kb_id,
                    subject_id,
                    predicate_id,
                    object_id,
                    validity,
                    confidence,
                )
                .await?;
                touched_facts.push(fact_id);
                // 重复观察也要挂证据：多来源相互印证，任一来源删除后事实不孤儿化。
                // 表层谓词随每次观察落笔——甲块说 "runs on"、乙块说 "optimized for"
                // 会并进同一条事实，放事实上就是先写者胜，放证据上两个都留着
                utopia_store::graph::add_evidence(
                    &state.pool,
                    fact_id,
                    chunk.id,
                    f.quote.as_deref(),
                    Some(f.predicate.as_str()),
                )
                .await?;
                if !created {
                    continue;
                }
                fact_count += 1;
                // 时态对账：带唯一性约束的状态关系落新事实即检测矛盾（纯规则点查，
                // 自动闭合走"作废+改写"，拿不准进 fact_conflicts 人裁）
                // 没有谓词就没有关系元数据，也就不参与时态对账——
                // 一条说不出是什么关系的边，本来就不可能带唯一性约束
                if let Some((pid, (func, inv_func, temporal))) =
                    predicate_id.and_then(|id| rel_meta.get(&id).map(|m| (id, m)))
                {
                    if temporal == "state" {
                        let mut directions = Vec::new();
                        if *func {
                            directions.push(utopia_store::temporal::Uniqueness::SubjectSide);
                        }
                        if *inv_func {
                            directions.push(utopia_store::temporal::Uniqueness::ObjectSide);
                        }
                        for dir in directions {
                            let report = utopia_store::temporal::reconcile_new_fact(
                                &state.pool,
                                doc.kb_id,
                                fact_id,
                                subject_id,
                                pid,
                                Some(object_id),
                                None,
                                dir,
                                validity,
                                confidence,
                            )
                            .await?;
                            if report.conflicts > 0 {
                                conflicts_found = true;
                            }
                        }
                    }
                }
            }
        }

        // 本块抽取完成即打标：更新时被认领的块携带标记跳过；中断的抽取可续跑
        // （LLM 调用/解析失败的块在上方 continue 掉，不打标，下次重试）
        utopia_store::documents::mark_chunk_extracted(&state.pool, chunk.id).await?;
    }
    // 队列里多了东西才叫醒人：Review 的计数与对话里那张确认卡都靠这一声
    if pending_count > 0 {
        tracing::info!(%document_id, pending_count, "记忆抽出的事实进了待确认队列");
        state.emit_pending(doc.kb_id);
        state.emit_review(doc.kb_id);
    }

    // 消歧后缀在实体创建时算会早于其事实写入——收尾时对本文档涉及的名字统一刷新
    touched_names.extend(doc_cache.keys().map(|(_, name)| name.clone()));
    for name in &touched_names {
        utopia_store::resolution::refresh_disambiguators(&state.pool, doc.kb_id, name).await?;
    }

    // 出口再验一次：接管可能发生在最后一个分块之后，那时循环里的检查已经跑完。
    // 漏掉这里，被顶替的任务会把 done 写在一篇 extracted_at 刚被清空的文档上——
    // 界面显示"已完成"，实则一条都没抽，要等新任务开跑才纠正回来。
    if utopia_store::documents::extract_epoch(&state.pool, document_id).await? != my_epoch {
        tracing::info!(%document_id, "抽取任务已被新一轮接管，收尾时退出");
        return Ok(());
    }

    // **有分块没抽成就不许标 done。**
    //
    // 从前这里无条件写 done：一次网络抖动让六篇文档 60 块里只抽成 12 块，
    // 六篇全部显示"抽取完成"，八成的内容没进图，而界面上没有任何东西说出来。
    // 失败只进了日志，而只进日志的错误等于没有错误。
    //
    // 返回 Err 之后这条链是完整的：`extract_document` 落 graph_failed + 原因，
    // 界面上那篇文档变成可点开看错误的 failed；任务按 30s×attempts² 退避重试，
    // 而已抽成的分块带着 extracted_at 会被跳过——所以重试很便宜，网络恢复就自愈。
    // 重试耗尽才留在 failed，那时它说的是实话。
    //
    // 判据是"全抽完"而不是某个比例：任何比例都是拍的，而这里本来就有一个
    // 不需要拍的判据——**每一块都成了才叫抽完**。
    if let Some(msg) = incomplete_reason(&unextracted, chunks.len()) {
        return Err(anyhow::anyhow!(msg));
    }
    // 刚落的事实立刻过一遍签名。写入时只掰方向（judge_direction）；掰不动的
    // ——两个方向都对不上、或宾语没类型判不了——从前要等人按 Review 里的
    // Run check 才露面，Axioms 一直是 0，图里却躺着反向事实（#222）。
    // 检查失败不影响抽取本身：事实已经在库里，下一次 Run check 仍然查得到
    if !touched_facts.is_empty() {
        match utopia_store::reasoning::signature_breaks(
            &state.pool,
            doc.kb_id,
            Some(&touched_facts),
        )
        .await
        {
            Ok(broken) if !broken.is_empty() => {
                match utopia_store::reasoning::record_signature_breaks(
                    &state.pool,
                    doc.kb_id,
                    &broken,
                )
                .await
                {
                    Ok(_) => state.emit_review(doc.kb_id),
                    Err(e) => {
                        tracing::warn!(%document_id, error = %e, "抽取后的签名违规没记进队列")
                    }
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(%document_id, error = %e, "抽取后的签名检查失败"),
        }
    }
    utopia_store::documents::set_graph_status(&state.pool, document_id, "done").await?;
    state.emit_document(doc.kb_id, document_id);

    // 灰区对进了审核队列 → 触发攒批裁决任务（独立后台跑，抽取本身到此已完成）
    if needs_adjudication {
        utopia_store::jobs::enqueue(
            &state.pool,
            "adjudicate_entities",
            serde_json::json!({ "kb_id": doc.kb_id }),
        )
        .await?;
    }
    if needs_adjudication || human_reviews_found || conflicts_found {
        state.emit_review(doc.kb_id);
    }
    // 类型消解排队自动跑（0016 C2）：开关开着就排一个库级任务，同库已排着的不重复。
    // 任务自己只看引擎没看过的实体、只自动落地子树内精化的那一档
    if kb.auto_type_resolution {
        utopia_store::jobs::enqueue_unless_queued(
            &state.pool,
            "resolve_types",
            serde_json::json!({ "kb_id": doc.kb_id }),
        )
        .await?;
    }
    // 自动扩本体：开关开着、且这一批都抽完了，由最后一篇触发。
    // 判据是显式开关而不是"本体有没有被碰过"——后者是从行为推断意图，
    // 推错的后果很荒唐（在提案上点一次 Add 就永久关掉建议），而且一旦为假
    // 就永不再真，本体会冻结在第一批文档碰巧包含的词汇上。
    // 并发下可能入队两次，任务自己会重查开关与状态
    enqueue_bootstrap(state, doc.kb_id).await?;

    tracing::info!(%document_id, facts = fact_count, "图谱抽取完成");
    Ok(())
}

/// 宾语位上的这串东西，是不是一个字面值而不是实体的名字。
///
/// **只认数字与日期。** 这是个会吃掉真实体的判断，所以宁可漏认：
/// 漏了不过是维持今天的行为（造一个 concept 实体），认错了却是把一个
/// 真实体降成一段文本，图里少一个节点。
///
/// "2015"、"2023-03"、"6" 认；"杭州"、"首席技术官"、"3M"、"V3" 不认。
/// 调用方还额外要求模型**没有**把它声明成实体——两道门一起过才算数。
fn looks_literal(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    // 纯数字（含小数与正负号）。用 f64 解而不是自己扫字符：
    // "3M"、"V3"、"２０１５"（全角）都会失败，正是想要的
    if s.parse::<f64>().is_ok() {
        return true;
    }
    // 日期：复用抽取侧那个解析器，它认 2015 / 2015-03 / 2015-03-01 等
    utopia_extract::parse_time(s).is_some()
}

/// 提示词里那三段清单：类、关系、属性。
///
/// **抽出来是为了让"全给"和"按分块检索"共用同一段排版逻辑。**两条路各排一份
/// 的话迟早分叉，而分叉在这里的后果是提示词说的与代码认的不是一回事。
struct PromptLists {
    types: Vec<(String, String, String)>,
    relations: Vec<utopia_extract::PromptRelation>,
    attributes: Vec<String>,
}

impl PromptLists {
    /// 这三段铺进提示词有多长。budget 判据用它——**量的是实际要排的那段字**，
    /// 不是另写一个估算公式（公式会跟排版分叉）。
    fn chars(&self) -> usize {
        self.types
            .iter()
            .map(|(k, l, d)| k.len() + l.len() + d.len() + 6)
            .sum::<usize>()
            + self
                .relations
                .iter()
                .map(|r| r.key.len() + r.label.len() + r.description.len() + r.signature.len() + 8)
                .sum::<usize>()
            + self.attributes.iter().map(|a| a.len() + 1).sum::<usize>()
    }
}

/// 从一个**选择集**排出三段清单。`None` = 全给（本体小于预算时的老路）。
///
/// 三处细节都是选择带来的，全给时它们不会触发：
///
/// 1. **签名只能提到选中的类**。`works_at (person → organization)` 里那两个 key
///    必须是模型看得见的——写一个没铺出去的类名，等于教它输出一个不存在的类型。
///    整侧都没选中就退回 `*`。
/// 2. **属性跟着 domain 走**。属性行是 `class.attr`，它的类没铺出去这行就没意义。
///    这也顺带解决了属性段（占提示词 28%）的裁剪，不用单独处理。
/// 3. **内置类恒在**。检索漏掉的分块仍然要有地方落脚，否则模型无类可选。
fn build_lists(
    etypes: &[utopia_core::models::EntityType],
    rtypes: &[utopia_core::models::RelationType],
    classes: Option<&HashSet<Uuid>>,
    rels: Option<&HashSet<Uuid>>,
) -> PromptLists {
    let picked_class = |id: &Uuid| classes.is_none_or(|s| s.contains(id));
    let picked_rel = |id: &Uuid| rels.is_none_or(|s| s.contains(id));
    let key_of: HashMap<Uuid, &str> = etypes
        .iter()
        .filter(|t| picked_class(&t.id))
        .map(|t| (t.id, t.key.as_str()))
        .collect();

    let types = etypes
        .iter()
        .filter(|t| picked_class(&t.id))
        .map(|t| (t.key.clone(), t.label.clone(), t.description.clone()))
        .collect();

    // 一侧的类一个都没铺出去就写 `*`：签名是导向，指向看不见的类只会误导
    let sig_of = |ids: &[Uuid]| -> String {
        let mut keys: Vec<&str> = ids
            .iter()
            .filter_map(|id| key_of.get(id).copied())
            .collect();
        if keys.is_empty() {
            return "*".into();
        }
        keys.sort_unstable();
        keys.join("|")
    };
    let relations = rtypes
        .iter()
        .filter(|r| r.kind != "attribute")
        .filter(|r| picked_rel(&r.id))
        .map(|r| {
            let signature = if r.domains.is_empty() && r.ranges.is_empty() {
                String::new()
            } else {
                format!("{} → {}", sig_of(&r.domains), sig_of(&r.ranges))
            };
            utopia_extract::PromptRelation {
                key: r.key.clone(),
                label: r.label.clone(),
                description: r.description.clone(),
                signature,
            }
        })
        .collect();

    let attributes = rtypes
        .iter()
        .filter(|r| r.kind == "attribute" && picked_rel(&r.id))
        .flat_map(|r| r.domains.iter().map(move |d| (r, d)))
        .filter_map(|(r, domain_id)| {
            let class_key = key_of.get(domain_id)?;
            let dt = r.datatype.as_deref().unwrap_or("text");
            let spec = match &r.unit {
                Some(u) if !u.is_empty() => format!("{dt}, {u}"),
                _ => dt.to_string(),
            };
            let d = r.description.trim();
            Some(if d.is_empty() {
                format!("- {class_key}.{} ({spec})", r.key)
            } else {
                format!("- {class_key}.{} ({spec}): {d}", r.key)
            })
        })
        .collect();

    PromptLists {
        types,
        relations,
        attributes,
    }
}

/// 每块检索多少个类 / 关系 / 属性。**待测**——跟预算一样，定它们要那条曲线。
const PER_CHUNK_CLASSES: i64 = 40;
const PER_CHUNK_RELATIONS: i64 = 30;
const PER_CHUNK_ATTRIBUTES: i64 = 30;

/// 按这一块的向量检索候选，排出这一块专用的三段清单。
///
/// 检索失败返回 `Ok(None)` 而不是错误：调用方会退回全量。提示词大是慢，
/// 没有类可选是抽不出东西——两者之间选前者。
async fn chunk_lists(
    state: &AppState,
    kb_id: Uuid,
    embedding: &[f32],
    etypes: &[utopia_core::models::EntityType],
    rtypes: &[utopia_core::models::RelationType],
    seed_classes: &HashSet<Uuid>,
) -> anyhow::Result<Option<PromptLists>> {
    let mut classes: HashSet<Uuid> = seed_classes.clone();
    classes.extend(
        utopia_store::ontology::nearest_entity_type_ids(
            &state.pool,
            kb_id,
            embedding,
            PER_CHUNK_CLASSES,
        )
        .await?,
    );
    // **命中什么，就把它的祖先一起铺出去。**
    //
    // 向量检索天然偏爱字面出现在正文里的叶子类。实测一个讲 Sutskever 的分块，
    // 976 个类按距离排：`researcher` 第 4、`corporation` 第 27，
    // 而 `organization` 第 177、`person` 第 359——**前 40 名里一个泛化基类都没有**。
    // 正文写的是 "a researcher at"、"the corporation"，从不写 "person"。
    //
    // 两个症状，同一个根因：
    //
    // - 实体判成 `researcher`（schema.org 里它是 `Audience` 的子类，不是人），
    //   于是 `works_for (domain=person)` 全成了违规
    // - `employee (organization → person)` 的签名**退化成 `(* → *)`**——`sig_of`
    //   只认铺出去的类，一侧没铺就写 `*`。模型根本没见过那个方向约束
    //
    // 从前这道地板由 `seed_classes`（`builtin` 的类）兜着，`build_lists` 的注释写着
    // 「内置类恒在：检索漏掉的分块仍然要有地方落脚」。种子退场后（#128）判据就悬空了——
    // 它当初碰巧等价，只因为种子类正好是那几个通用类。
    //
    // 用祖先补这道地板，比维护一张"通用类"清单好：**继承链本来就是本体自己声明的
    // 泛化关系**，谁是谁的上位不需要我们再判断一次。代价是每块多铺几层祖先。
    if !classes.is_empty() {
        let picked: Vec<Uuid> = classes.iter().copied().collect();
        classes.extend(utopia_store::ontology::ancestors_of(&state.pool, &picked).await?);
    }
    let mut rels: HashSet<Uuid> = HashSet::new();
    // 关系与属性分开检索：两段在提示词里是分开的，混在一起取会让其中一段
    // 被另一段挤空
    rels.extend(
        utopia_store::ontology::nearest_relation_type_ids(
            &state.pool,
            kb_id,
            embedding,
            PER_CHUNK_RELATIONS,
            Some("relation"),
        )
        .await?,
    );
    rels.extend(
        utopia_store::ontology::nearest_relation_type_ids(
            &state.pool,
            kb_id,
            embedding,
            PER_CHUNK_ATTRIBUTES,
            Some("attribute"),
        )
        .await?,
    );
    // **一个关系被铺出去，它签名点名的类就得跟着铺。**
    //
    // 类与关系是各自独立检索的，而签名依赖两者的交集——`sig_of` 只认铺出去的类，
    // 一侧没铺就写 `*`。于是常出现这种局面：`employee` 跟正文语义相近被捞了进来，
    // 而它的 `organization`（第 795 名）与 `person`（第 630 名）离正文字面很远，
    // 一个都没捞到，签名退化成 `(* → *)`——**方向约束整个消失**，模型按英语直觉
    // 写 `Musk --employee--> Microsoft`，而 schema.org 声明的是 organization → person。
    //
    // 上面那道祖先地板治不了这种块：它从"命中的叶子"往上长，而这里一个相关的
    // 叶子都没命中，没有叶子也就没有祖先。
    //
    // `sig_of` 的注释说「签名指向看不见的类只会误导」——顾虑是对的，但抹掉签名
    // 是拿丢失方向来换。**把类拉进来**两头都保住：模型看得见那个类，签名也排得出。
    // 顺带还对：这些类正是模型马上要用来判类型的那些，`employee` 在场就说明
    // 这一块讲的是雇佣，`organization`/`person` 本来就该在候选里——
    // 按字面相似度捞不到它们，但**本体的结构知道**。
    let sig_classes: HashSet<Uuid> = rtypes
        .iter()
        .filter(|r| rels.contains(&r.id))
        .flat_map(|r| r.domains.iter().chain(r.ranges.iter()).copied())
        .collect();
    classes.extend(sig_classes);

    // 一个候选都没检索到 = 索引还没建好，退回全量而不是给一份空清单
    if classes.len() <= seed_classes.len() && rels.is_empty() {
        return Ok(None);
    }
    Ok(Some(build_lists(
        etypes,
        rtypes,
        Some(&classes),
        Some(&rels),
    )))
}

#[cfg(test)]
mod name_tests {
    use super::{clause_suspect, is_entity_name};

    /// 样本全部取自实跑出来的库（ai-timeline-ends × schema.org），不是编的。
    #[test]
    fn a_clause_is_not_a_thing() {
        for s in [
            "thermal-imaging equipment used by volunteers flying over the site showed at least 33 generators giving off heat",
            "about the same amount of power as the Tennessee Valley Authority's large gas-fired power plant nearby",
            "removal was driven by growing discontent and distrust with Altman",
            "a risk of developing cancer at four times the national average in 2013",
            "745 of OpenAI's 770 employees",
        ] {
            assert!(!is_entity_name(s), "这是一句话，不该当成实体名：{s}");
        }
    }

    /// **真实体会长，但不带谓语。** 判据是词数 + 限定动词，不是字符数——
    /// 下面第一个 57 字符，比上面那条 65 字符的从句还短不了多少
    #[test]
    fn a_long_name_is_still_a_name() {
        for s in [
            "US District Court for the Northern District of California",
            "United States District Court for the District of Delaware",
            "OpenAI's board of directors",
            "Safe Superintelligence Inc.",
            "École Polytechnique",
            "GPT-4",
        ] {
            assert!(is_entity_name(s), "这是真实体，不该被挡：{s}");
        }
    }

    /// #193：词表之外的句子，结构信号接住——句号结尾、情态动词。样本来自第二份语料
    #[test]
    fn a_sentence_is_caught_without_its_verb_on_the_list() {
        for s in [
            "The removal could slow down the artificial intelligence industry as a whole.",
            "Shares in Microsoft fell nearly three percent following the announcement.",
            "the board should reconsider its position",
        ] {
            assert!(!is_entity_name(s), "这是一句话，不该当成实体名：{s}");
        }
        // 大写缩写的句号、兼作名词的情态词，都不误伤
        for s in [
            "Safe Superintelligence Inc.",
            "Theresa May",
            "Trash Can Museum",
        ] {
            assert!(is_entity_name(s), "这是真实体，不该被挡：{s}");
        }
    }

    /// 弱信号只记不挡：像从句的照常落库，但留下样本
    #[test]
    fn a_suspect_is_recorded_not_rejected() {
        let s = "The committee that reviewed the merger of the two companies";
        assert!(is_entity_name(s));
        assert_eq!(clause_suspect(s), Some("determiner_opens_a_long_string"));
        assert_eq!(
            clause_suspect("committee members who reviewed the merger"),
            Some("relative_or_subordinate_clause")
        );
        for s in [
            "US District Court for the Northern District of California",
            "OpenAI's board of directors",
            "The New York Times",
            "The Men Who Stare",
        ] {
            assert_eq!(clause_suspect(s), None, "{s} 不该被怀疑");
        }
    }

    /// 分词在名词短语里完全正常，列进标志词会误伤。
    #[test]
    fn a_participle_is_not_a_predicate() {
        assert!(is_entity_name("equipment used by volunteers"));
        assert!(is_entity_name("Gas-Burning Turbines"));
    }

    /// 短名字不做从句判断：`Is` 之类可能是专名的一部分。
    #[test]
    fn a_short_name_is_never_a_clause() {
        assert!(is_entity_name("Was"));
        assert!(is_entity_name("Is Elon"));
    }

    #[test]
    fn an_empty_name_is_not_a_name() {
        assert!(!is_entity_name(""));
        assert!(!is_entity_name("   "));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        incomplete_reason, looks_literal, no_ref_name_binding, referenced_entity, resolve_bare,
        resolve_handle, BoundEntity, NoRefNameBinding,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    #[test]
    fn only_numbers_and_dates_count_as_literals() {
        // 认：这些出现在宾语位上时是值，不是实体
        for yes in ["2015", "2023-03", "2024-01-15", "1200", "62.5", "-3"] {
            assert!(looks_literal(yes), "{yes} 该认成字面值");
        }
        // 不认：判错的代价是把一个真实体降成一段文本，所以宁可漏
        for no in [
            "杭州",
            "首席技术官",
            "3M",
            "V3",
            "深蓝存储",
            "",
            "   ",
            "２０１５", // 全角数字：不是我们要处理的形态，交给实体路径
        ] {
            assert!(!looks_literal(no), "{no} 不该认成字面值");
        }
    }

    #[test]
    fn no_ref_names_bypass_legacy_map_only_after_two_handle_claims() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let entity_ids = HashMap::from([("Zhang Wei".to_string(), b)]);
        let normalized = utopia_store::resolution::normalize_name("Zhang Wei").to_lowercase();

        assert_eq!(
            no_ref_name_binding(&entity_ids, &HashMap::new(), "Zhang Wei"),
            NoRefNameBinding::Legacy(b)
        );
        assert_eq!(
            no_ref_name_binding(
                &entity_ids,
                &HashMap::from([(normalized.clone(), vec![a])]),
                "Zhang Wei",
            ),
            NoRefNameBinding::Legacy(b)
        );
        assert_eq!(
            no_ref_name_binding(
                &entity_ids,
                &HashMap::from([(normalized, vec![a, b])]),
                "Zhang Wei",
            ),
            NoRefNameBinding::AmbiguousHandled
        );
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
    async fn namesake_handles_keep_fact_attribution_and_bare_mentions_get_c() -> anyhow::Result<()>
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
            let mut legacy_cache = HashMap::new();
            let mut legacy_needs_adjudication = false;
            let legacy_first = super::resolve(
                &pool,
                kb,
                Some(person),
                "Alice",
                None,
                None,
                &mut legacy_cache,
                &mut legacy_needs_adjudication,
            )
            .await?;
            let legacy_second = super::resolve(
                &pool,
                kb,
                Some(person),
                "Alice",
                None,
                None,
                &mut legacy_cache,
                &mut legacy_needs_adjudication,
            )
            .await?;
            assert_eq!(legacy_first, legacy_second, "legacy name cache is unchanged");

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

            let refs = HashMap::from([
                (
                    "e1".to_string(),
                    BoundEntity {
                        id: a,
                        type_id: Some(person),
                    },
                ),
                (
                    "e2".to_string(),
                    BoundEntity {
                        id: b,
                        type_id: Some(person),
                    },
                ),
            ]);
            assert_eq!(referenced_entity(&refs, "e1").map(|x| x.id), Some(a));
            assert_eq!(referenced_entity(&refs, "e2").map(|x| x.id), Some(b));
            assert!(referenced_entity(&refs, "missing").is_none());
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
                let subject = referenced_entity(&refs, handle).expect("valid ref").id;
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
                "SELECT subject_id, object_id FROM facts WHERE kb_id = $1 ORDER BY subject_id",
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

            let mut doc_cache = HashMap::new();
            // The legacy surface map still ends on B, but a no-ref fact must not use that
            // last-write-wins value once handles have claimed two distinct Zhang Weis.
            let entity_ids = HashMap::from([("Zhang Wei".to_string(), b)]);
            assert_eq!(entity_ids.get("Zhang Wei"), Some(&b));
            assert_eq!(
                super::no_ref_name_binding(&entity_ids, &document_claims, "Zhang Wei"),
                super::NoRefNameBinding::AmbiguousHandled
            );
            let c = resolve_bare(
                &pool,
                kb,
                Some(person),
                "Zhang Wei",
                None,
                None,
                &mut doc_cache,
                &document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;
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
            let c_again = resolve_bare(
                &pool,
                kb,
                None,
                "Zhang Wei",
                None,
                None,
                &mut doc_cache,
                &document_claims,
                &mut bare_cache,
                &mut needs_adjudication,
                &mut human_reviews,
            )
            .await?;
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
            assert_ne!(c, a);
            assert_ne!(c, b);
            assert_eq!(
                c_again, c,
                "later bare mentions must reuse document-local C"
            );
            let c_objects: Vec<Uuid> = sqlx::query_scalar(
                "SELECT object_id FROM facts WHERE kb_id = $1 AND subject_id = $2 ORDER BY object_id",
            )
            .bind(kb)
            .bind(c)
            .fetch_all(&pool)
            .await?;
            assert_eq!(c_objects.len(), 2);
            assert!(c_objects.contains(&finance));
            assert!(c_objects.contains(&platform));
            let mut later_response_claims = HashMap::new();
            let c_via_fresh_handle = resolve_handle(
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
            assert_eq!(
                c_via_fresh_handle, c,
                "a later response cannot evade A/B ambiguity by inventing a new e-handle"
            );

            let reviews = utopia_store::resolution::list_reviews(&pool, kb, 10, 0).await?;
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

    /// #270：跨文档的裸 mention 同名并列——不靠 handle，靠画像相似度。库里已有两个
    /// 「张伟」，画像质心一模一样（同一 chunk 播的种）。新 mention 对两人打出同一个分。
    /// 旧路径在最高分 ≥ SIM_ATTACH 时静默 attach 到先遇到的那个；修好之后 `resolve`
    /// 走的这条路要：新建第三个实体、两条**人工**审核对、且绝不唤醒 LLM 裁决器
    /// （否则两条几乎相同的画像会被自动并掉，正是要防的）。
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
            let mut doc_cache = HashMap::new();
            let mut needs_adjudication = false;
            let c = super::resolve(
                &pool,
                kb,
                Some(person),
                "Zhang Wei",
                Some(&ctx),
                None,
                &mut doc_cache,
                &mut needs_adjudication,
            )
            .await?;

            assert_ne!(c, a, "画像并列不该 attach 到 A——那是候选顺序掷出的硬币");
            assert_ne!(c, b, "画像并列不该 attach 到 B——那是候选顺序掷出的硬币");
            assert!(
                !needs_adjudication,
                "同名并列只能等人裁，绝不该唤醒 LLM 裁决器"
            );

            let reviews = utopia_store::resolution::list_reviews(&pool, kb, 10, 0).await?;
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
