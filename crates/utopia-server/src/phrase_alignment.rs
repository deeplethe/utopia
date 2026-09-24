//! 关系短语按签名绑到属性（0044 决定 3 的第二片；账本侧见 `utopia_store::phrase_bindings`，
//! 合同见 `utopia_extract::phrase_align`）。
//!
//! 签名 = 短语 × 主语的类 × 宾语的类（宾语是字面值时记「值」）。两端的类来自类别词绑定
//! 写到实体上的 `type_id`，所以这一步排在类别词对齐之后。一个库里 distinct 的签名比陈述
//! 少得多，每条只判一次：**两票一致**才绑（第二票候选倒序，防「选第一个」冒充一致），
//! 不一致记 undecided 留给审核（#725 对齐队列），没有属性对得上记 none——陈述留在开放
//! 图谱，什么都不丢，签名计入工作台的建议。绑定按属性的 `updated_at` 与库里最新的属性
//! 判过期，本体一改只重判过期的。这一片只记判定；按绑定把陈述算成类型化事实是下一片。
//!
//! 候选属性怎么来：先按声明的定义域/值域筛——签名两端的类落在属性的域/值域里的，或者
//! 属性没声明域/值域的（两个方向都算，绑定可以是反向的）。**一端没绑到类的签名，只有
//! 没声明那一端的属性才算候选**：类别词还没绑上时把声明了域的属性也给模型，NVDA 四篇上
//! 现金流量表的每一行都绑到了泛泛的 value（主语 NVIDIA 没类，value 的域是指标）——裁判
//! 判成写错的一半是它。筛完仍超过上限就不判，瞎判比不判糟。

use crate::extraction::chat_retrying_rate_limits_at;
use crate::llm_util;
use crate::state::AppState;
use std::collections::{HashMap, HashSet};
use utopia_core::models::RelationTypeView;
use utopia_extract::phrase_align::{
    build_phrase_messages, parse_phrase_response, Direction, PhraseItem, PropertyCandidate,
};
use utopia_store::phrase_bindings::{self, Decision, PhraseSignature};
use uuid::Uuid;

/// 一次问多少条签名。
const BATCH: usize = 12;
/// 筛过定义域/值域之后候选属性最多这么多，再多就不判。
const CANDIDATE_LIMIT: usize = 60;

/// 一票：这条签名选了哪个属性、哪个方向（None = 没有属性对得上）。
type Vote = Option<(String, Direction)>;

/// 一个类连同它的全部祖先。候选按它命中：属性的定义域声明在 legal_entity 上，
/// organization 是它的子类，这条属性对 organization 的签名就是候选（#807 第一条）。
type Closure = HashMap<Uuid, Vec<Uuid>>;

fn closures<'a>(classes: impl IntoIterator<Item = (Uuid, &'a [Uuid])>) -> Closure {
    let classes: Vec<(Uuid, &[Uuid])> = classes.into_iter().collect();
    let parents: HashMap<Uuid, &[Uuid]> = classes.iter().copied().collect();
    classes
        .iter()
        .map(|(id, direct)| {
            let mut seen: Vec<Uuid> = vec![*id];
            let mut stack: Vec<Uuid> = direct.to_vec();
            // 多继承与菱形：UNION 语义，同一个祖先只进一次；环不会有（编辑器不允许）
            while let Some(p) = stack.pop() {
                if seen.contains(&p) {
                    continue;
                }
                seen.push(p);
                if let Some(pp) = parents.get(&p) {
                    stack.extend_from_slice(pp);
                }
            }
            seen.sort();
            (*id, seen)
        })
        .collect()
}

/// 一条候选怎么命中的：`via` 是经继承命中的依据（空 = 直接命中或没声明）。
struct Fit {
    via: Vec<(Uuid, Uuid)>,
}

/// 声明的类里有没有一个是这一端的类或其祖先。没声明不限；这一端没绑到类时只被没声明
/// 的接受。返回命中的 (声明的类, 这一端的类) 当它不是直接命中时
fn within(
    declared: &[Uuid],
    class: Option<Uuid>,
    closure: &Closure,
) -> Option<Option<(Uuid, Uuid)>> {
    if declared.is_empty() {
        return Some(None);
    }
    let c = class?;
    if declared.contains(&c) {
        return Some(None);
    }
    let up = closure.get(&c)?;
    declared
        .iter()
        .find(|d| up.contains(d))
        .map(|d| Some((*d, c)))
}

/// 签名两端的类落在属性声明的域/值域里，经继承也算；正反两个方向都算。
fn fits(p: &RelationTypeView, sig: &PhraseSignature, closure: &Closure) -> Option<Fit> {
    let mut via = Vec::new();
    if sig.object_is_value {
        if p.kind != "attribute" {
            return None;
        }
        via.extend(within(&p.domains, sig.subject_type_id, closure)?);
        return Some(Fit { via });
    }
    if p.kind != "relation" {
        return None;
    }
    let forward = within(&p.domains, sig.subject_type_id, closure).zip(within(
        &p.ranges,
        sig.object_type_id,
        closure,
    ));
    let reverse = within(&p.domains, sig.object_type_id, closure).zip(within(
        &p.ranges,
        sig.subject_type_id,
        closure,
    ));
    let (a, b) = forward.or(reverse)?;
    via.extend(a);
    via.extend(b);
    Some(Fit { via })
}

/// 签名的键：短语 + 两端的类 + 宾语是不是字面值（与 `PhraseSignature::key` 同形）
type SignatureKey = (String, Option<Uuid>, Option<Uuid>, bool);
/// 每条签名此刻的候选与指纹
type Considered<'a> = HashMap<SignatureKey, (Vec<&'a RelationTypeView>, String)>;

/// 每条活着的签名此刻的候选（经继承命中）与指纹（0053）。开跑时算一次决定要判谁，
/// 收尾时用重新加载的输入再算一次决定要不要再排——两次之间世界可能变了
fn consider<'a>(
    sigs: &[PhraseSignature],
    props: &'a [RelationTypeView],
    closure: &Closure,
    versions: &HashMap<Uuid, chrono::DateTime<chrono::Utc>>,
    shortlist: Option<&Shortlist>,
) -> Considered<'a> {
    let empty: Vec<Uuid> = Vec::new();
    sigs.iter()
        .map(|s| {
            let mut fitting: Vec<&RelationTypeView> = props
                .iter()
                .filter(|p| fits(p, s, closure).is_some())
                .collect();
            // 结构对得上的太多时只留最近的几条（见 `shortlist`），按相关度排：第一票先看最像的
            if let Some(keep) = shortlist.and_then(|m| m.get(&s.key())) {
                fitting.retain(|p| keep.contains(&p.id));
                fitting.sort_by_key(|p| keep.iter().position(|k| *k == p.id));
            }
            let cands: Vec<(Uuid, chrono::DateTime<chrono::Utc>)> = fitting
                .iter()
                .filter_map(|p| versions.get(&p.id).map(|at| (p.id, *at)))
                .collect();
            let up = |c: Option<Uuid>| -> &[Uuid] {
                c.and_then(|c| closure.get(&c))
                    .map(Vec::as_slice)
                    .unwrap_or(&empty)
            };
            let basis = phrase_bindings::basis_of(
                up(s.subject_type_id),
                up(s.object_type_id),
                s.object_is_value,
                &cands,
            );
            (s.key(), (fitting, basis))
        })
        .collect()
}

/// 日志里放得下的一段回复：空白折成一个空格，最多这么多字符。
const SNIPPET_CHARS: usize = 240;

fn snippet(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(SNIPPET_CHARS).collect();
    if flat.chars().count() > SNIPPET_CHARS {
        out.push('…');
    }
    out
}

/// 一条签名结构对得上的属性多过这个数，就按相关度只留这么多给模型看。第一次真跑里每条
/// 签名平均拖着几十条候选（六个粗类切不掉什么），一次请求 1.8 万 token，对齐占了一轮
/// 八成的用量（bench README，2026-09-24）；十条里若没有对的，多半是本体里就没有
const SHORTLIST: usize = 10;
/// 同时在飞的对齐批次数；模型闸门（工作区的并发上限）在下面再限一次
const PARALLEL_BATCHES: usize = 4;
/// 一次嵌入多少条签名的文本
const SHORTLIST_EMBED_BATCH: usize = 32;

/// 签名 → 留给模型看的候选 id（按相关度）。没进表的签名照旧看全部结构候选
type Shortlist = HashMap<SignatureKey, Vec<Uuid>>;

/// 按相关度给候选多的签名开短名单：签名的文本（短语加一条例句）嵌入后，与属性的向量比
/// 近（`embed_ontology` 建的那份），留最近的 [`SHORTLIST`] 条；标签里的词出现在短语里的
/// 属性无论远近都留着（"based in" 对 "based in"）。没配嵌入模型、属性还没向量、或候选本来
/// 就不多的签名不进表——那时模型看的还是全部结构候选
async fn shortlist(
    state: &AppState,
    settings: &utopia_core::models::LlmSettings,
    kb_id: Uuid,
    sigs: &[PhraseSignature],
    full: &Considered<'_>,
) -> anyhow::Result<Shortlist> {
    let mut out = Shortlist::new();
    let Some(client) = llm_util::embed_client(settings) else {
        return Ok(out);
    };
    let wide: Vec<&PhraseSignature> = sigs
        .iter()
        .filter(|s| full.get(&s.key()).is_some_and(|(f, _)| f.len() > SHORTLIST))
        .collect();
    if wide.is_empty() {
        return Ok(out);
    }
    let pool = &state.pool;
    for batch in wide.chunks(SHORTLIST_EMBED_BATCH) {
        let texts: Vec<String> = batch
            .iter()
            .map(|s| match (s.examples.first(), s.quotes.first()) {
                (Some(e), Some(q)) => format!("{} · {e} · {q}", s.phrase),
                (Some(e), None) => format!("{} · {e}", s.phrase),
                _ => s.phrase.clone(),
            })
            .collect();
        let vectors = {
            let _permit = llm_util::acquire_embed(state, settings).await;
            match client.embed(&texts).await {
                Ok(v) if v.len() == batch.len() => v,
                Ok(v) => {
                    tracing::warn!(%kb_id, sent = batch.len(), got = v.len(), "签名向量数量对不上，这一批不开短名单");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "签名向量没算出来，这一批不开短名单");
                    continue;
                }
            }
        };
        for (s, vector) in batch.iter().zip(vectors) {
            let (fitting, _) = &full[&s.key()];
            let kind = if s.object_is_value {
                "attribute"
            } else {
                "relation"
            };
            let near = utopia_store::ontology::nearest_relation_type_ids(
                pool,
                kb_id,
                &vector,
                (fitting.len() * 2) as i64,
                Some(kind),
            )
            .await?;
            if near.is_empty() {
                // 属性还没有向量：不开短名单，模型看全部
                continue;
            }
            let fitting_ids: Vec<Uuid> = fitting.iter().map(|p| p.id).collect();
            let must_keep: Vec<Uuid> = fitting
                .iter()
                .filter(|p| label_in_phrase(&p.label, &s.phrase))
                .map(|p| p.id)
                .collect();
            out.insert(
                s.key(),
                pick_shortlist(&near, &fitting_ids, &must_keep, SHORTLIST),
            );
        }
    }
    tracing::info!(%kb_id, wide = wide.len(), shortlisted = out.len(), "候选短名单开好");
    Ok(out)
}

/// 类别词 → 留给提规则看的属性 id（按相关度）。词的文本加几个例名嵌入，取最近的
/// [`SHORTLIST`] 条；没配嵌入模型、属性没向量的不进表
async fn shortlist_kind_words(
    state: &AppState,
    settings: &utopia_core::models::LlmSettings,
    kb_id: Uuid,
    words: &[&utopia_store::type_bindings::KindWordSignature],
) -> anyhow::Result<HashMap<String, Vec<Uuid>>> {
    let mut out = HashMap::new();
    let Some(client) = llm_util::embed_client(settings) else {
        return Ok(out);
    };
    if words.is_empty() {
        return Ok(out);
    }
    for batch in words.chunks(SHORTLIST_EMBED_BATCH) {
        let texts: Vec<String> = batch
            .iter()
            .map(|k| format!("{} · {}", k.kind_word, k.examples.join(", ")))
            .collect();
        let vectors = {
            let _permit = llm_util::acquire_embed(state, settings).await;
            match client.embed(&texts).await {
                Ok(v) if v.len() == batch.len() => v,
                Ok(_) | Err(_) => {
                    tracing::warn!(%kb_id, "类别词向量没算出来，这一批看全部候选");
                    continue;
                }
            }
        };
        for (k, vector) in batch.iter().zip(vectors) {
            let near = utopia_store::ontology::nearest_relation_type_ids(
                &state.pool,
                kb_id,
                &vector,
                SHORTLIST as i64,
                None,
            )
            .await?;
            if !near.is_empty() {
                out.insert(k.kind_word.clone(), near);
            }
        }
    }
    Ok(out)
}

/// 标签里有一个像样的词（四个字母以上）出现在短语里
fn label_in_phrase(label: &str, phrase: &str) -> bool {
    let phrase = phrase.to_lowercase();
    label
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| w.len() >= 4 && phrase.contains(w))
}

/// 短名单：先是标签对上的（无论远近），再按向量距离补到上限；都是结构对得上的
fn pick_shortlist(near: &[Uuid], fitting: &[Uuid], must_keep: &[Uuid], limit: usize) -> Vec<Uuid> {
    let mut out: Vec<Uuid> = must_keep
        .iter()
        .copied()
        .filter(|id| fitting.contains(id))
        .collect();
    for id in near {
        if out.len() >= limit {
            break;
        }
        if fitting.contains(id) && !out.contains(id) {
            out.push(*id);
        }
    }
    out
}

/// 一轮里没判完的（调用失败、回复读不出、模型漏答）自己再排几次；超过这个数就等
/// 下一篇文档或本体的改动再问。不设上限的话，温度为零下一段每次都读不出的回复会让
/// 任务每隔几十秒把同一段提示词再送一遍，没有尽头（同类别词对齐）
pub(crate) const MAX_REASK: u32 = 3;

/// 对一个库跑一遍：新出现的和过期的签名各判一次。
/// `reask` 是这份任务已经是第几次自己排的（文档、本体、类别词对齐排的是 0）。
pub async fn align_phrases_reasking(
    state: &AppState,
    kb_id: Uuid,
    reask: u32,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let kb = utopia_store::kbs::get(pool, kb_id).await?;
    let settings = utopia_store::settings::get(pool, kb.workspace_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align phrases"))?;
    // 对齐是判断题：让模型按端点默认的强度想，不用工作区给抽取设的 minimal
    let client = llm_util::chat_client_thinking(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align phrases"))?;
    // 一个库同时只跑一份，理由同类别词对齐（并行跑会把端点打出 502）
    let mut guard = pool.acquire().await?;
    let locked: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtext('align_phrases'), hashtext($1))")
            .bind(kb_id.to_string())
            .fetch_one(&mut *guard)
            .await?;
    if !locked {
        // 正在跑的那份结束时会自己看一眼有没有新东西（见 align_phrases_locked 末尾）；这里
        // 不排——排回去会和跑着的那份互相踢成死循环
        tracing::info!(%kb_id, "短语对齐已有一份在跑，这次跳过");
        return Ok(());
    }
    let result = align_phrases_locked(state, kb_id, reask, &settings, &client).await;
    let _ = sqlx::query("SELECT pg_advisory_unlock(hashtext('align_phrases'), hashtext($1))")
        .bind(kb_id.to_string())
        .execute(&mut *guard)
        .await;
    result
}

async fn align_phrases_locked(
    state: &AppState,
    kb_id: Uuid,
    reask: u32,
    settings: &utopia_core::models::LlmSettings,
    client: &utopia_llm::LlmClient,
) -> anyhow::Result<()> {
    let pool = &state.pool;
    let props = utopia_store::ontology::relation_type_views(pool, kb_id).await?;
    let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
    let class_key: HashMap<Uuid, &str> = classes.iter().map(|c| (c.id, c.key.as_str())).collect();
    let by_key: HashMap<&str, &RelationTypeView> =
        props.iter().map(|p| (p.key.as_str(), p)).collect();
    let closure = closures(classes.iter().map(|c| (c.id, c.parents.as_slice())));
    let versions = phrase_bindings::property_versions(pool, kb_id).await?;
    let sigs = phrase_bindings::signatures(pool, kb_id).await?;
    let existing: HashMap<_, _> = phrase_bindings::bindings(pool, kb_id)
        .await?
        .into_iter()
        .map(|b| (b.key(), b))
        .collect();
    // 每条活着的签名此刻的候选与指纹。候选按继承命中，多了再按相关度开短名单；指纹是判定
    // 看到的全部输入（0053），短名单也算在内——名单变了就再问
    let full = consider(&sigs, &props, &closure, &versions, None);
    let short = shortlist(state, settings, kb_id, &sigs, &full).await?;
    let considered = consider(&sigs, &props, &closure, &versions, Some(&short));
    // 过期 = 存下的指纹和此刻的不一样（没有指纹的是这一列出现前判的，各重判一次）。
    // 不再按时间戳：父边的增删、请求途中的编辑（#795）时间戳看不见。人的判定不重判
    let todo: Vec<&PhraseSignature> = sigs
        .iter()
        .filter(|s| match existing.get(&s.key()) {
            None => true,
            Some(b) => {
                b.decided_by != "person"
                    && b.basis.as_deref() != Some(considered[&s.key()].1.as_str())
            }
        })
        .collect();
    let attempted: HashSet<_> = todo.iter().map(|s| s.key()).collect();
    tracing::info!(%kb_id, signatures = sigs.len(), to_decide = todo.len(), properties = props.len(), "短语对齐开始");

    // 没有属性可绑：每条都是「没有」；属性出现后指纹变了，它们会再交回来
    if props.is_empty() {
        for s in &todo {
            phrase_bindings::decide(
                pool,
                kb_id,
                s,
                Decision {
                    relation_type_id: None,
                    direction: None,
                    status: "none",
                    votes: &serde_json::json!({ "reason": "no_properties" }),
                    decided_by: "agent",
                    basis: Some(&considered[&s.key()].1),
                },
            )
            .await?;
        }
        return Ok(());
    }

    let keys_of = |ids: &[Uuid]| -> Vec<&str> {
        ids.iter()
            .filter_map(|id| class_key.get(id).copied())
            .collect()
    };
    let (mut bound, mut none, mut undecided, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    // 调用或解析失败的批次：这轮跳过，结束时自己再排一次
    let mut failed = 0usize;
    // 问了、模型也答了、却没答到的签名：两票缺一票就不下结论
    let mut unanswered = 0usize;
    // 批与批并行（[`PARALLEL_BATCHES`] 个在飞，模型闸门再限一次）：一批两票串行要等模型
    // 想两回，串着跑 22 批就是半小时，其中一次卡住的调用能把整轮拖住 18 分钟（bench README，
    // 2026-09-24）。每批各记各的数，回来再加
    {
        use futures_util::StreamExt;
        // 只把引用搬进各批的 future
        let (considered, full, closure, class_key, by_key) =
            (&considered, &full, &closure, &class_key, &by_key);
        // 先把每批的 future 造出来再排队：直接在 map 里返回 async 块会让借用的生命周期
        // 满足不了 tokio::spawn 要的 Send
        let futures: Vec<_> = todo
            .chunks(BATCH)
            .map(|batch| async move {
                let (mut bound, mut none, mut undecided, mut skipped, mut failed, mut unanswered) =
                    (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        // 候选超过上限的不问模型：记成 undecided 交给人，指纹照记——属性少下去指纹就变，
        // 到时再问。从前超限和无候选一样静默跳过，签名永远排着又永远不可执行（#807）
        let cands: Vec<Vec<&RelationTypeView>> = batch
            .iter()
            .map(|s| {
                let fitting = &considered[&s.key()].0;
                if fitting.len() > CANDIDATE_LIMIT {
                    Vec::new()
                } else {
                    fitting.clone()
                }
            })
            .collect();
        let mut votes: Vec<(Vote, Vote)> = vec![(None, None); batch.len()];
        let mut answered = vec![(false, false); batch.len()];
        for pass in 0..2 {
            let items: Vec<PhraseItem<'_>> = batch
                .iter()
                .enumerate()
                .filter(|(i, _)| !cands[*i].is_empty())
                .map(|(i, s)| {
                    let mut list: Vec<&RelationTypeView> = cands[i].clone();
                    if pass == 1 {
                        list.reverse();
                    }
                    PhraseItem {
                        id: i as i64,
                        phrase: &s.phrase,
                        subject_class: s.subject_type_key.as_deref(),
                        object_class: s.object_type_key.as_deref(),
                        object_is_value: s.object_is_value,
                        statement_count: s.count,
                        examples: &s.examples,
                        quotes: &s.quotes,
                        candidates: list
                            .iter()
                            .map(|p| PropertyCandidate {
                                key: &p.key,
                                label: &p.label,
                                description: &p.description,
                                kind: &p.kind,
                                domains: keys_of(&p.domains),
                                ranges: keys_of(&p.ranges),
                                via: fits(p, s, closure)
                                    .map(|f| {
                                        f.via
                                            .iter()
                                            .map(|(declared, class)| {
                                                format!(
                                                    "{} is a subclass of {}",
                                                    class_key.get(class).copied().unwrap_or("?"),
                                                    class_key.get(declared).copied().unwrap_or("?"),
                                                )
                                            })
                                            .collect()
                                    })
                                    .unwrap_or_default(),
                            })
                            .collect(),
                        // 结构对得上却没进短名单的键：模型若从批里的属性表选了它，算票
                        also_allowed: full[&s.key()]
                            .0
                            .iter()
                            .filter(|p| !cands[i].iter().any(|c| c.id == p.id))
                            .map(|p| p.key.as_str())
                            .collect(),
                    }
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            let messages = build_phrase_messages(&items);
            let reply =
                match chat_retrying_rate_limits_at(state, settings, client, &messages, Some(0.0))
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!(%kb_id, error = %e, "短语对齐调用失败，这一批留到下次");
                        failed += 1;
                        continue;
                    }
                };
            let (choices, malformed) = match parse_phrase_response(&reply.text, &items) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(%kb_id, error = %e, "短语对齐回复解析失败，这一批留到下次");
                    failed += 1;
                    continue;
                }
            };
            skipped += malformed;
            if choices.is_empty() {
                // 解出来了却一条都没读到：回复的形状不是我们认得的。这和解析失败是一回事，
                // 按失败算、留到下次。从前这里什么都不说，每一条签名都当「有一票没答到」
                // 静静跳过，日志里只有一串「完成 bound=0」——回复的开头要进日志，下次才
                // 知道它长什么样（同类别词对齐）
                tracing::warn!(
                    %kb_id,
                    pass,
                    items = items.len(),
                    malformed,
                    finish_reason = ?reply.finish_reason,
                    chars = reply.text.chars().count(),
                    reply = %snippet(&reply.text),
                    "短语对齐回复读不出一条，这一批留到下次"
                );
                failed += 1;
                continue;
            if malformed > 0 {
                // 坏票长什么样得看得见：第一次真跑里一半签名被判坏票，查了一天才知道模型答的是标签
                tracing::info!(%kb_id, malformed, reply = %snippet(&reply.text), "短语对齐的回复里有坏票");
            }
            }
            for c in choices {
                let Ok(i) = usize::try_from(c.id) else {
                    continue;
                };
                if let Some(slot) = votes.get_mut(i) {
                    if pass == 0 {
                        slot.0 = c.property;
                        answered[i].0 = true;
                    } else {
                        slot.1 = c.property;
                        answered[i].1 = true;
                    }
                }
            }
        }
        for (i, s) in batch.iter().enumerate() {
            let basis = considered[&s.key()].1.as_str();
            if cands[i].is_empty() {
                let fitting = considered[&s.key()].0.len();
                // 两种「没问模型」各自落库，投影才退得掉、队列才收得住：
                // 没有一条属性对得上 → none（绑过的签名失去支撑，类型化行随物化作废）；
                // 对得上的太多 → undecided 交给人，不再每轮重排
                let (status, votes) = if fitting == 0 {
                    ("none", serde_json::json!({ "reason": "no_candidates" }))
                } else {
                    (
                        "undecided",
                        serde_json::json!({ "first": null, "second": null,
                                            "reason": "too_many_candidates", "candidates": fitting }),
                    )
                };
                if phrase_bindings::decide(
                    pool,
                    kb_id,
                    s,
                    Decision {
                        relation_type_id: None,
                        direction: None,
                        status,
                        votes: &votes,
                        decided_by: "agent",
                        basis: Some(basis),
                    },
                )
                .await?
                {
                    if status == "none" {
                        none += 1;
                    } else {
                        undecided += 1;
                    }
                }
                skipped += 1;
                continue;
            }
            let (a, b) = &votes[i];
            let (ans_a, ans_b) = answered[i];
            if !ans_a || !ans_b {
                // 有一票没答到：不下结论，下次再问
                unanswered += 1;
                continue;
            }
            let show = |v: &Vote| {
                v.as_ref()
                    .map(|(k, d)| serde_json::json!({ "property": k, "direction": d.as_str() }))
                    .unwrap_or(serde_json::Value::Null)
            };
            let record = serde_json::json!({ "first": show(a), "second": show(b) });
            if a != b {
                phrase_bindings::decide(
                    pool,
                    kb_id,
                    s,
                    Decision {
                        relation_type_id: None,
                        direction: None,
                        status: "undecided",
                        votes: &record,
                        decided_by: "agent",
                        basis: Some(basis),
                    },
                )
                .await?;
                undecided += 1;
                continue;
            }
            match a
                .as_ref()
                .and_then(|(k, d)| by_key.get(k.as_str()).map(|p| (p, *d)))
            {
                Some((p, d)) => {
                    if phrase_bindings::decide(
                        pool,
                        kb_id,
                        s,
                        Decision {
                            relation_type_id: Some(p.id),
                            direction: Some(d.as_str()),
                            status: "bound",
                            votes: &record,
                            decided_by: "agent",
                            basis: Some(basis),
                        },
                    )
                    .await?
                    {
                        bound += 1;
                    }
                }
                None => {
                    if phrase_bindings::decide(
                        pool,
                        kb_id,
                        s,
                        Decision {
                            relation_type_id: None,
                            direction: None,
                            status: "none",
                            votes: &record,
                            decided_by: "agent",
                            basis: Some(basis),
                        },
                    )
                    .await?
                    {
                        none += 1;
                    }
                }
            }
        }

                Ok::<_, anyhow::Error>((bound, none, undecided, skipped, failed, unanswered))
            })
            .collect();
        let mut results = futures_util::stream::iter(futures).buffer_unordered(PARALLEL_BATCHES);
        while let Some(r) = results.next().await {
            let (b, n, u, sk, f, un) = r?;
            bound += b;
            none += n;
            undecided += u;
            skipped += sk;
            failed += f;
            unanswered += un;
        }
    }
    tracing::info!(%kb_id, bound, none, undecided, skipped, failed, unanswered, "短语对齐完成");
    if unanswered > 0 {
        tracing::warn!(%kb_id, unanswered, "短语对齐有签名模型没答到，这些签名这轮没有结论");
    }
    // 提规则（0044 决定 3 第五片）：本轮刚判过的签名，和带类别词的东西，问模型「这种形状
    // 还蕴含什么」。只问本轮判过的：指纹没变的形状上一轮已经问过，答案（提案或代理驳回）
    // 还在 implication_rules 里；指纹变了它就在 todo 里，自然再问
    {
        let decided_now: HashMap<_, _> = phrase_bindings::bindings(pool, kb_id)
            .await?
            .into_iter()
            .map(|b| (b.key(), b))
            .collect();
        let kind_words = utopia_store::type_bindings::signatures(pool, kb_id).await?;
        let existing_rules = utopia_store::implication_rules::list(pool, kb_id, None).await?;
        let asked_kind: HashSet<&str> = existing_rules
            .iter()
            .filter(|r| r.trigger == "kind_word")
            .map(|r| r.phrase.as_str())
            .collect();
        let mut asks: Vec<crate::implication::RuleAsk<'_>> = Vec::new();
        for s in &todo {
            let Some(b) = decided_now.get(&s.key()) else {
                continue;
            };
            // 只问绑上的签名：判「无」的形状一轮 1121 条问下来提了不到 1% 的规则，却占了
            // 提规则一半以上的调用（bench README，2026-09-24）；拿不定的等人先定
            if b.status != "bound" {
                continue;
            }
            let (fitting, basis) = &considered[&s.key()];
            let bound_to = b
                .relation_type_id
                .and_then(|id| props.iter().find(|p| p.id == id))
                .map(|p| p.key.as_str());
            asks.push(crate::implication::RuleAsk {
                phrase: Some(s),
                kind_word: None,
                bound_to,
                candidates: fitting
                    .iter()
                    .copied()
                    .filter(|p| Some(p.key.as_str()) != bound_to)
                    .collect(),
                basis,
            });
        }
        // 类别词：每个词问一次；候选是主语能落在它绑到的类（或没声明）的关系属性
        let kind_basis: Vec<String> = kind_words
            .iter()
            .map(|k| {
                phrase_bindings::basis_of(
                    &[],
                    &[],
                    false,
                    &versions
                        .iter()
                        .map(|(id, at)| (*id, *at))
                        .collect::<Vec<_>>(),
                ) + ":"
                    + &k.kind_word
            })
            .collect();
        // 类别词的候选也开短名单：词加例名嵌入后取最近的属性；没有向量时看全部
        let fresh: Vec<&utopia_store::type_bindings::KindWordSignature> = kind_words
            .iter()
            .filter(|k| !asked_kind.contains(k.kind_word.as_str()))
            .collect();
        let kind_short = shortlist_kind_words(state, settings, kb_id, &fresh).await?;
        for (k, basis) in kind_words.iter().zip(kind_basis.iter()) {
            if asked_kind.contains(k.kind_word.as_str()) {
                continue;
            }
            let mut candidates: Vec<&RelationTypeView> = props
                .iter()
                .filter(|p| p.kind == "relation" || p.kind == "attribute")
                .collect();
            if let Some(keep) = kind_short.get(&k.kind_word) {
                candidates.retain(|p| keep.contains(&p.id));
                candidates.sort_by_key(|p| keep.iter().position(|id| *id == p.id));
            }
            asks.push(crate::implication::RuleAsk {
                phrase: None,
                kind_word: Some(k),
                bound_to: None,
                candidates,
                basis,
            });
        }
        if !asks.is_empty() {
            match crate::implication::propose_rules(
                state, kb_id, settings, client, &asks, &class_key, &by_key,
            )
            .await
            {
                Ok((proposed, rule_failed)) => {
                    tracing::info!(%kb_id, asked = asks.len(), proposed, failed = rule_failed, "提规则完成");
                    if proposed > 0 {
                        state.emit_review(kb_id);
                    }
                }
                Err(e) => tracing::warn!(%kb_id, error = %e, "提规则失败，下一轮再提"),
            }
        }
    }
    // 绑定定了，视图跟着算：绑上的签名下的陈述成类型化行，绑定变了的行作废（0067）
    let typed = utopia_store::materialize::materialize(pool, kb_id).await?;
    tracing::info!(%kb_id, added = typed.added, merged = typed.merged, retired = typed.retired, "类型化事实按绑定算完");
    if typed.added > 0 || typed.merged > 0 || typed.retired > 0 {
        state.emit_graph(kb_id);
    }
    // 这一轮跑着的时候世界没停：新文档带来新签名，改了的属性、动了的父边让刚判的绑定
    // 过期，本轮没排上的触发也都落在这里。有没试过的新签名、有本轮判完指纹又变了的绑定
    // （请求途中的编辑，#795），就再排一次（从头算一份，新签名换了提示词）。
    // 只看**活着的**签名：端点的类换了，旧签名的行没有陈述可判，它永远「过期」却永远
    // 不可执行——从前 `stale` 把这种孤儿每轮交回来，一条孤儿排一次 job，三轮三次（#807）
    let changed = {
        // **重新加载**，不是拿开跑时的快照比：快照就是判定写下的那份指纹，跟它比永远
        // 相等。模型答着的时候改了定义（#795）、加了父边、来了新文档，只有再读一遍才看得见
        let props = utopia_store::ontology::relation_type_views(pool, kb_id).await?;
        let classes = utopia_store::graph::entity_types(pool, kb_id).await?;
        let closure = closures(classes.iter().map(|c| (c.id, c.parents.as_slice())));
        let versions = phrase_bindings::property_versions(pool, kb_id).await?;
        let sigs = phrase_bindings::signatures(pool, kb_id).await?;
        // 短名单沿用开跑时算的那份：向量没变，名单就没变；变了的签名本轮之后自然再问
        let now_considered = consider(&sigs, &props, &closure, &versions, Some(&short));
        let now: HashMap<_, _> = phrase_bindings::bindings(pool, kb_id)
            .await?
            .into_iter()
            .map(|b| (b.key(), b))
            .collect();
        sigs.iter().any(|s| match now.get(&s.key()) {
            None => !attempted.contains(&s.key()),
            Some(b) => {
                b.decided_by != "person"
                    && b.basis.as_deref() != Some(now_considered[&s.key()].1.as_str())
            }
        })
    };
    // 本轮没判完的（调用失败、回复读不出、模型漏答了几条）自己再排，最多 MAX_REASK 次，
    // 每次多等一会。从前只有失败的批次会再排，读不出的回复解成「零条、零坏」不算失败，
    // 漏答的签名就只能等下一篇文档来排——最后一篇之后没有下一篇，它们就永远没有结论；
    // 而漏答不写任何行，审核队列也看不见（同类别词对齐）
    let unfinished = failed > 0 || unanswered > 0;
    if changed {
        utopia_store::jobs::enqueue_unless_queued(
            pool,
            "align_phrases",
            serde_json::json!({ "kb_id": kb_id }),
        )
        .await?;
    } else if unfinished && reask < MAX_REASK {
        let delay = std::time::Duration::from_secs(20 * u64::from(reask + 1));
        tracing::info!(%kb_id, failed, unanswered, reask = reask + 1, delay_secs = delay.as_secs(), "短语对齐没判完，稍后再问");
        utopia_store::jobs::enqueue_unless_pending(
            pool,
            "align_phrases",
            serde_json::json!({ "kb_id": kb_id, "reask": reask + 1 }),
            delay,
        )
        .await?;
    } else if unfinished {
        tracing::warn!(%kb_id, failed, unanswered, reask, "短语对齐问了几轮仍没判完，等下一篇文档或本体改动再问");
    }
    Ok(())
}

#[cfg(test)]
#[path = "phrase_alignment_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
mod tests {
    #[test]
    fn a_shortlist_keeps_label_matches_and_fills_by_distance_within_the_fitting_set() {
        let ids: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
        // near 按距离：ids[3] 最近，但不在结构候选里；ids[5] 标签对上，排在最后也留
        let near = vec![ids[3], ids[0], ids[1], ids[2], ids[4], ids[5]];
        let fitting = vec![ids[0], ids[1], ids[2], ids[4], ids[5]];
        let picked = pick_shortlist(&near, &fitting, &[ids[5]], 3);
        assert_eq!(picked, vec![ids[5], ids[0], ids[1]]);
        assert!(label_in_phrase(
            "headquarters location",
            "has its headquarters in"
        ));
        assert!(!label_in_phrase("country", "is based in"));
        assert!(
            !label_in_phrase("in", "is based in"),
            "short words do not count"
        );
    }

    use super::*;

    fn view(kind: &str, domains: Vec<Uuid>, ranges: Vec<Uuid>) -> RelationTypeView {
        RelationTypeView {
            id: Uuid::now_v7(),
            key: "p".into(),
            label: "p".into(),
            temporal: "state".into(),
            functional: false,
            inverse_functional: false,
            is_transitive: false,
            is_symmetric: false,
            is_asymmetric: false,
            is_irreflexive: false,
            inverse_of: None,
            sub_property_of: None,
            builtin: false,
            description: String::new(),
            kind: kind.into(),
            domains,
            ranges,
            datatype: None,
            unit: None,
            qualifiers: Vec::new(),
            usage: 0,
        }
    }

    fn sig(subject: Option<Uuid>, object: Option<Uuid>, value: bool) -> PhraseSignature {
        PhraseSignature {
            phrase: "x".into(),
            subject_type_id: subject,
            subject_type_key: None,
            object_type_id: object,
            object_type_key: None,
            object_is_value: value,
            count: 1,
            examples: Vec::new(),
            quotes: Vec::new(),
        }
    }

    /// 域/值域筛候选：声明了的要落在里面（正反都算），没声明的不限，类为空的一端不限
    #[test]
    fn a_property_fits_a_signature_by_its_declared_ends_in_either_direction() {
        let (org, place, person) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        // 没有父边时闭包为空：每个类只等于它自己，行为与从前一样
        let ok = |p: &RelationTypeView, s: &PhraseSignature| fits(p, s, &Closure::new()).is_some();
        let hq = view("relation", vec![org], vec![place]);
        assert!(ok(&hq, &sig(Some(org), Some(place), false)));
        assert!(ok(&hq, &sig(Some(place), Some(org), false)), "反向也算");
        assert!(!ok(&hq, &sig(Some(person), Some(place), false)));
        assert!(
            !ok(&hq, &sig(None, Some(place), false)),
            "没绑到类的一端不算落在声明的域里"
        );
        let any_to_place = view("relation", vec![], vec![place]);
        assert!(
            ok(&any_to_place, &sig(None, Some(place), false)),
            "没声明的一端接受没绑到类的"
        );
        assert!(!ok(&hq, &sig(Some(org), None, true)), "关系不接字面值");
        let open = view("relation", vec![], vec![]);
        assert!(
            ok(&open, &sig(Some(person), Some(person), false)),
            "没声明就不限"
        );
        let revenue = view("attribute", vec![org], vec![]);
        assert!(ok(&revenue, &sig(Some(org), None, true)));
        assert!(!ok(&revenue, &sig(Some(person), None, true)));
        assert!(
            !ok(&revenue, &sig(Some(org), Some(place), false)),
            "属性只接字面值"
        );
    }
    /// 声明在祖先上的属性经继承命中子类的签名（#807 第一条）；依据要能说给模型听
    #[test]
    fn a_property_declared_on_an_ancestor_fits_a_subclass_by_inheritance() {
        let (legal_entity, org, place) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let mut closure = Closure::new();
        closure.insert(org, {
            let mut v = vec![org, legal_entity];
            v.sort();
            v
        });
        closure.insert(legal_entity, vec![legal_entity]);
        closure.insert(place, vec![place]);
        let hq = view("relation", vec![legal_entity], vec![place]);
        let fit =
            fits(&hq, &sig(Some(org), Some(place), false), &closure).expect("fits via parent");
        assert_eq!(
            fit.via,
            vec![(legal_entity, org)],
            "the basis names the declared ancestor and the class"
        );
        let direct =
            fits(&hq, &sig(Some(legal_entity), Some(place), false), &closure).expect("direct");
        assert!(direct.via.is_empty(), "a direct hit needs no explanation");
        assert!(
            fits(&hq, &sig(Some(place), Some(org), false), &closure).is_some(),
            "reverse direction walks the hierarchy too"
        );
        assert!(
            fits(&hq, &sig(Some(org), Some(place), false), &Closure::new()).is_none(),
            "without the parent edge the property is not a candidate"
        );
    }

    /// 闭包：多继承与菱形，每个祖先只出现一次，且含自己
    #[test]
    fn closures_walk_the_hierarchy_once_per_ancestor() {
        let (thing, agent, legal, org) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // org ⊂ agent ⊂ thing 且 org ⊂ legal ⊂ thing：菱形
        let (a, l, o) = ([thing], [thing], [agent, legal]);
        let c = closures([
            (thing, &[][..]),
            (agent, &a[..]),
            (legal, &l[..]),
            (org, &o[..]),
        ]);
        let mut expect = vec![org, agent, legal, thing];
        expect.sort();
        assert_eq!(c[&org], expect);
        assert_eq!(c[&thing], vec![thing]);
    }
}
