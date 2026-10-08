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
    build_phrase_messages, parse_phrase_response, Direction, Marks, PhraseItem, PropertyCandidate,
};
use utopia_store::phrase_bindings::{self, Decision, PhraseSignature};
use uuid::Uuid;

/// 一次问多少条签名。
const BATCH: usize = 12;
/// 筛过定义域/值域之后候选属性最多这么多，再多就不判。
const CANDIDATE_LIMIT: usize = 60;

/// 一票：这条签名选了哪个属性、哪个方向，属性是状态时还有一刻标哪一端（#966）。
/// None = 没有属性对得上
type Vote = Option<(String, Direction, Option<Marks>)>;

/// 代理绑到状态属性、却从没问过一刻标哪一端的绑定（这一列之前的判定，#966）：指纹没变也问
/// 一次，只问这一格，不重判绑定（0053 修订 2026-09-27）。问过的不再问，没得到值的列进对齐
/// 队列等人。只进开跑时的 todo，不进收尾的「变了没有」
fn awaits_marks(b: &phrase_bindings::Binding, props: &[RelationTypeView]) -> bool {
    b.status == "bound"
        && b.marks.is_none()
        && b.marks_asked_at.is_none()
        && b.relation_type_id
            .and_then(|id| props.iter().find(|p| p.id == id))
            .is_some_and(|p| p.temporal == "state")
}

/// 一个类连同它的全部祖先。候选按它命中：属性的定义域声明在 legal_entity 上，
/// organization 是它的子类，这条属性对 organization 的签名就是候选（#807 第一条）。
pub(crate) type Closure = HashMap<Uuid, Vec<Uuid>>;

pub(crate) fn closures<'a>(classes: impl IntoIterator<Item = (Uuid, &'a [Uuid])>) -> Closure {
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

/// 结构上对得上（本体代理给每批裁词表时用的判据，同对齐的候选）
pub(crate) fn structurally_fits(
    p: &RelationTypeView,
    sig: &PhraseSignature,
    closure: &Closure,
) -> bool {
    fits(p, sig, closure).is_some()
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

/// 第二票要不要投。第二票防的是「选第一个」冒充一致，要两票一致的是**绑定**；第一票
/// 说没有一条对得上的签名，第二票怎么答都绑不上：答没有是「无」，答了一条是「拿不定」。
/// 测量库一轮 864 条签名里第一票说没有的 317 条，没有一条最后绑上（bench README，
/// 2026-09-28），这一票省下。第一票没答到的也不投：少一票本来就不下结论。
/// 补问 marks 的照旧投两票：它要两票说同一个值
fn second_vote_is_due(first: &Vote, first_answered: bool, marks_only: bool) -> bool {
    marks_only || (first_answered && first.is_some())
}

/// 类别词规则的宾语是从类别词自己的字里读出来的（「british film」读出英国），只有中心词
/// 的类别词（「state」「ship」）没有可读的字：一轮 159 个类别词里 127 个是单个词，提出
/// 的 8 条规则没有一条读得出宾语（bench README，2026-09-28）。这种不问。
///
/// 只认得出用空格分词的写法：一串 ASCII 字母是一个词。带连字符、数字的（「1968」、
/// 「british-governed」）和不分词的文字（「英国电影」）看不出有没有修饰语，照旧问
fn has_modifier(kind_word: &str) -> bool {
    !kind_word.trim().chars().all(|c| c.is_ascii_alphabetic())
}

/// 把签名分成批，候选相近的放在一起。返回每批里签名的下标。
///
/// 属性定义表在一批里只写一遍，占一次调用输入的将近一半：按到来的次序每十二条切一批时，
/// 同批的签名互不相干（一条讲出生地、一条讲导演、一条讲所属球队），各自十条候选几乎不
/// 重叠，合起来就是本体的一半（测量库上九十六条属性里约五十条）。这里只改排法：每条签名
/// 看到的候选一条不少，判断一字不变。
///
/// 贪心：每批从还没排的第一条起，每次加进让这一批的候选并集长得最少的那一条，并列时取
/// 靠前的——结果只由输入决定，同样的签名每次排成同样的批。没有候选的签名不问模型，
/// 排在最后，不占有候选的批里的位置
fn batch_by_candidates<K: Copy + Eq + std::hash::Hash>(
    sets: &[Vec<K>],
    size: usize,
) -> Vec<Vec<usize>> {
    let size = size.max(1);
    let mut left: Vec<usize> = (0..sets.len()).filter(|i| !sets[*i].is_empty()).collect();
    let empty: Vec<usize> = (0..sets.len()).filter(|i| sets[*i].is_empty()).collect();
    let mut out: Vec<Vec<usize>> = Vec::new();
    while !left.is_empty() {
        let first = left.remove(0);
        let mut batch = vec![first];
        let mut union: HashSet<K> = sets[first].iter().copied().collect();
        while batch.len() < size && !left.is_empty() {
            let (at, _) = left
                .iter()
                .enumerate()
                .map(|(at, i)| (at, sets[*i].iter().filter(|k| !union.contains(k)).count()))
                .min_by_key(|(at, added)| (*added, *at))
                .expect("left is not empty");
            let i = left.remove(at);
            union.extend(sets[i].iter().copied());
            batch.push(i);
        }
        out.push(batch);
    }
    out.extend(empty.chunks(size).map(<[usize]>::to_vec));
    out
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
    // 先问这一类属性有没有向量，没有的签名不嵌：嵌了最近邻也是空，照样看全部。一个没跑过
    // `embed_ontology` 的库从前每轮把一万七千条签名送去嵌入、短名单一条没开（#1097）。
    // 一类问一句，至多两句
    let mut has_vectors = HashMap::new();
    for kind in ["relation", "attribute"] {
        if wide.iter().any(|s| property_kind(s) == kind) {
            let has =
                utopia_store::ontology::has_relation_type_vectors(pool, kb_id, Some(kind)).await?;
            has_vectors.insert(kind, has);
        }
    }
    let usable: Vec<&PhraseSignature> = wide
        .iter()
        .copied()
        .filter(|s| has_vectors.get(property_kind(s)) == Some(&true))
        .collect();
    for batch in usable.chunks(SHORTLIST_EMBED_BATCH) {
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
            let near = utopia_store::ontology::nearest_relation_type_ids(
                pool,
                kb_id,
                &vector,
                (fitting.len() * 2) as i64,
                Some(property_kind(s)),
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
    tracing::info!(
        %kb_id,
        wide = wide.len(),
        skipped = wide.len() - usable.len(),
        shortlisted = out.len(),
        "候选短名单开好"
    );
    Ok(out)
}

/// 签名能绑的是哪一类属性：宾语是字面值的绑属性，否则绑关系
fn property_kind(s: &PhraseSignature) -> &'static str {
    if s.object_is_value {
        "attribute"
    } else {
        "relation"
    }
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
    // 一条带向量的属性都没有，嵌了也是空手而回（理由同 `shortlist`）
    if words.is_empty()
        || !utopia_store::ontology::has_relation_type_vectors(&state.pool, kb_id, None).await?
    {
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
    let client = llm_util::chat_client(&settings)
        .ok_or_else(|| anyhow::anyhow!("Chat model not configured; cannot align phrases"))?;
    // 类别词对齐还排着或跑着：两端的类还没定，现在判的签名指纹马上就变，判了也是重判。
    // 等它收尾再来——排一份半分钟后的，排着的至多一份。别的入口（本体页、审核页、每篇文档
    // 抽完）排的短语对齐也从这里过，所以这一道守住就够。
    //
    // 本体向量还在补也等：编辑器建了属性先排 `embed_ontology` 再排这个任务，没向量的属性
    // 进不了短名单，形状的指纹不变，现在判了它照样不在候选里（`bordered_by` 第一次真跑
    // 就是这么漏的）。补齐任务几秒到几分钟，等得起
    let waiting_on = if utopia_store::jobs::pending_for_kb(pool, "align_types", kb_id).await? {
        Some("类别词对齐")
    } else if utopia_store::jobs::pending_for_kb(pool, "embed_ontology", kb_id).await? {
        Some("本体向量补齐")
    } else {
        None
    };
    if let Some(what) = waiting_on {
        tracing::info!(%kb_id, "短语对齐：{what}还没收尾，半分钟后再看");
        let payload = if reask == 0 {
            serde_json::json!({ "kb_id": kb_id })
        } else {
            serde_json::json!({ "kb_id": kb_id, "reask": reask })
        };
        utopia_store::jobs::enqueue_unless_queued_after(
            pool,
            "align_phrases",
            payload,
            std::time::Duration::from_secs(30),
        )
        .await?;
        return Ok(());
    }
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
    // 不再按时间戳：父边的增删、请求途中的编辑（#795）时间戳看不见。人的判定不重判。
    // 绑到状态属性、从没问过一刻标哪一端的代理判定也问（`awaits_marks`，#966）
    let todo: Vec<&PhraseSignature> = sigs
        .iter()
        .filter(|s| match existing.get(&s.key()) {
            None => true,
            Some(b) => {
                b.decided_by != "person"
                    && (b.basis.as_deref() != Some(considered[&s.key()].1.as_str())
                        || awaits_marks(b, &props))
            }
        })
        .collect();
    let attempted: HashSet<_> = todo.iter().map(|s| s.key()).collect();
    // 其中指纹没变、只为补问 marks 进来的：这一问只能写 marks，绑定原样（0053 修订 2026-09-27）。
    // 从前整条重判，两票没选属性就把绑上的签名判成 none，它的类型化行跟着作废
    let marks_only: HashMap<_, &phrase_bindings::Binding> = todo
        .iter()
        .filter_map(|s| {
            let b = existing.get(&s.key())?;
            (b.basis.as_deref() == Some(considered[&s.key()].1.as_str()) && awaits_marks(b, &props))
                .then_some((s.key(), b))
        })
        .collect();
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
                    marks: None,
                    marks_asked: false,
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
    // marks：补问写下了值的；问过却没得到两票同一个值、列进对齐队列的
    let (mut marked, mut unsettled) = (0usize, 0usize);
    // 批与批并行（[`PARALLEL_BATCHES`] 个在飞，模型闸门再限一次）：一批两票串行要等模型
    // 想两回，串着跑 22 批就是半小时，其中一次卡住的调用能把整轮拖住 18 分钟（bench README，
    // 2026-09-24）。每批各记各的数，回来再加
    {
        use futures_util::StreamExt;
        // 只把引用搬进各批的 future
        let (considered, full, closure, class_key, by_key, marks_only) = (
            &considered,
            &full,
            &closure,
            &class_key,
            &by_key,
            &marks_only,
        );
        // 先把每批的 future 造出来再排队：直接在 map 里返回 async 块会让借用的生命周期
        // 满足不了 tokio::spawn 要的 Send
        // 候选相近的签名排进同一批（见 `batch_by_candidates`）：属性表一批只写一遍，同批的
        // 签名候选重叠得越多，这张表越短
        let sets: Vec<Vec<Uuid>> = todo
            .iter()
            .map(|s| {
                let fitting = &considered[&s.key()].0;
                if fitting.len() > CANDIDATE_LIMIT {
                    Vec::new()
                } else {
                    fitting.iter().map(|p| p.id).collect()
                }
            })
            .collect();
        let batches: Vec<Vec<&PhraseSignature>> = batch_by_candidates(&sets, BATCH)
            .into_iter()
            .map(|group| group.into_iter().map(|i| todo[i]).collect())
            .collect();
        let futures: Vec<_> = batches
            .iter()
            .map(|batch| batch.as_slice())
            .map(|batch| async move {
                let (mut bound, mut none, mut undecided, mut skipped, mut failed, mut unanswered) =
                    (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
                let (mut marked, mut unsettled) = (0usize, 0usize);
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
        // 读得出回复的遍数：补问 marks 时，两遍的回复都回来了才算问过
        let mut replied = 0usize;
        for pass in 0..2 {
            let items: Vec<PhraseItem<'_>> = batch
                .iter()
                .enumerate()
                .filter(|(i, _)| !cands[*i].is_empty())
                .filter(|(i, s)| pass == 0 || second_vote_is_due(&votes[*i].0, answered[*i].0, marks_only.contains_key(&s.key())))
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
                                temporal: &p.temporal,
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
            }
            replied += 1;
            if malformed > 0 {
                // 坏票长什么样得看得见：第一次真跑里一半签名被判坏票，查了一天才知道模型答的是标签
                tracing::info!(%kb_id, malformed, reply = %snippet(&reply.text), "短语对齐的回复里有坏票");
            }
            for c in choices {
                let Ok(i) = usize::try_from(c.id) else {
                    continue;
                };
                if let Some(slot) = votes.get_mut(i) {
                    let vote = c.property.map(|(key, direction)| (key, direction, c.marks));
                    if pass == 0 {
                        slot.0 = vote;
                        answered[i].0 = true;
                    } else {
                        slot.1 = vote;
                        answered[i].1 = true;
                    }
                }
            }
        }
        for (i, s) in batch.iter().enumerate() {
            let basis = considered[&s.key()].1.as_str();
            if let Some(asked) = marks_only.get(&s.key()) {
                // 补问 marks（0053 修订 2026-09-27）：两票都认这条绑定的属性与方向、又说了同一个
                // 值，写下它；别的答案——没选属性、选了别的、没说或说得不一样——都让绑定原样。
                // 两种都记下问过：不再问，没有值的列进对齐队列等人。回复没回来不算问过，
                // 下次再问（同没答到的签名）；问不了的（候选超限）直接交给人
                if !cands[i].is_empty() && replied < 2 {
                    unanswered += 1;
                    continue;
                }
                // 这一票认的是这条绑定，又说了值：认了就是它说的值
                let value_of = |v: &Vote| match v {
                    Some((k, d, m)) => m.filter(|_| {
                        by_key.get(k.as_str()).map(|p| p.id) == asked.relation_type_id
                            && asked.direction.as_deref() == Some(d.as_str())
                    }),
                    None => None,
                };
                let (a, b) = &votes[i];
                let value = value_of(a).filter(|m| value_of(b) == Some(*m));
                if phrase_bindings::record_marks(pool, kb_id, asked, value.map(Marks::as_str))
                    .await?
                {
                    if value.is_some() {
                        marked += 1;
                    } else {
                        unsettled += 1;
                    }
                }
                continue;
            }
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
                        marks: None,
                        marks_asked: false,
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
            // 第一票说没有的不投第二票（见 `second_vote_is_due`）：结论就是没有
            let asked_twice = second_vote_is_due(a, ans_a, false);
            if !ans_a || (asked_twice && !ans_b) {
                // 有一票没答到：不下结论，下次再问
                unanswered += 1;
                continue;
            }
            let show = |v: &Vote| {
                v.as_ref()
                    .map(|(k, d, m)| {
                        serde_json::json!({ "property": k, "direction": d.as_str(),
                                            "marks": m.map(Marks::as_str) })
                    })
                    .unwrap_or(serde_json::Value::Null)
            };
            let record = if asked_twice {
                serde_json::json!({ "first": show(a), "second": show(b) })
            } else {
                serde_json::json!({ "first": null, "reason": "first_vote_none" })
            };
            // 两票选的属性与方向（或都说没有）是否一致
            let agree = match (a, b) {
                (Some((ka, da, _)), Some((kb, db, _))) => ka == kb && da == db,
                (None, None) => true,
                _ => false,
            };
            if !agree {
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
                        marks: None,
                        marks_asked: false,
                    },
                )
                .await?;
                undecided += 1;
                continue;
            }
            // 属性是状态时，两票还说一刻标哪一端（#966）：说了同一个值才写下它。没说、或说得
            // 不一样，绑定照绑、这一格空着，记下问过——不再问，列进对齐队列等人（0053 修订
            // 2026-09-27）。从前没说的算没答到，模型一直只答三格时每轮重问、永远绑不上。
            // 事件与恒常不问这一格
            let state = a
                .as_ref()
                .and_then(|(k, _, _)| by_key.get(k.as_str()))
                .is_some_and(|p| p.temporal == "state");
            let marks = match (a, b) {
                (Some((_, _, ma)), Some((_, _, mb))) if state && ma == mb => *ma,
                _ => None,
            };
            match a
                .as_ref()
                .and_then(|(k, d, _)| by_key.get(k.as_str()).map(|p| (p, *d)))
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
                            marks: marks.map(Marks::as_str),
                            marks_asked: state,
                        },
                    )
                    .await?
                    {
                        bound += 1;
                        if state && marks.is_none() {
                            unsettled += 1;
                        }
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
                            marks: None,
                            marks_asked: false,
                        },
                    )
                    .await?
                    {
                        none += 1;
                    }
                }
            }
        }

                Ok::<_, anyhow::Error>((
                    (bound, none, undecided, skipped, failed, unanswered),
                    (marked, unsettled),
                ))
            })
            .collect();
        let mut results = futures_util::stream::iter(futures).buffer_unordered(PARALLEL_BATCHES);
        while let Some(r) = results.next().await {
            let ((b, n, u, sk, f, un), (m, us)) = r?;
            bound += b;
            none += n;
            undecided += u;
            skipped += sk;
            failed += f;
            unanswered += un;
            marked += m;
            unsettled += us;
        }
    }
    tracing::info!(%kb_id, bound, none, undecided, skipped, failed, unanswered, marked, unsettled, "短语对齐完成");
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
            // 只补问了 marks 的没有重判：指纹没变，它的规则上一轮问过
            if marks_only.contains_key(&s.key()) {
                continue;
            }
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
            .filter(|k| !asked_kind.contains(k.kind_word.as_str()) && has_modifier(&k.kind_word))
            .collect();
        let kind_short = shortlist_kind_words(state, settings, kb_id, &fresh).await?;
        for (k, basis) in kind_words.iter().zip(kind_basis.iter()) {
            if asked_kind.contains(k.kind_word.as_str()) || !has_modifier(&k.kind_word) {
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
    // 对齐收尾：没绑上的形状够多，叫本体代理来看（0061 决定 2）。代理自己跳过提过的，
    // 所以这里只数不筛；一分钟的去抖让连着几篇文档只叫一次
    if !changed && !unfinished {
        let unbound = phrase_bindings::bindings(pool, kb_id)
            .await?
            .iter()
            .filter(|b| matches!(b.status.as_str(), "none" | "undecided"))
            .count();
        if unbound >= crate::ontology_agent::TRIGGER_UNBOUND {
            utopia_store::jobs::enqueue_unless_pending(
                pool,
                "propose_ontology",
                serde_json::json!({ "kb_id": kb_id }),
                std::time::Duration::from_secs(60),
            )
            .await?;
        }
    }
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
    fn a_kind_word_that_is_only_a_head_word_has_nothing_to_read() {
        for bare in ["state", "ship", " settlement "] {
            assert!(!has_modifier(bare), "{bare}");
        }
        for readable in [
            "british film",
            "1968",
            "british-governed",
            "英国电影",
            "wine area",
        ] {
            assert!(has_modifier(readable), "{readable}");
        }
    }

    #[test]
    fn signatures_with_the_same_candidates_share_a_batch() {
        // 六条签名、三种候选集，交错着来；每批两条
        let sets: Vec<Vec<u32>> = vec![
            vec![1, 2],
            vec![7, 8],
            vec![],
            vec![2, 1],
            vec![8, 9],
            vec![1, 2, 3],
        ];
        let batches = batch_by_candidates(&sets, 2);
        // 每条签名恰好出现一次
        let mut all: Vec<usize> = batches.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3, 4, 5]);
        // 候选一样的排在一起，没有候选的在最后
        assert_eq!(batches, vec![vec![0, 3], vec![1, 4], vec![5], vec![2]]);
        // 同样的输入排成同样的批
        assert_eq!(batch_by_candidates(&sets, 2), batches);
        // 一批的候选并集比按次序切的小
        let union = |b: &Vec<usize>| {
            b.iter()
                .flat_map(|i| sets[*i].iter())
                .collect::<HashSet<_>>()
                .len()
        };
        let in_order: usize = [vec![0, 1], vec![2, 3], vec![4, 5]].iter().map(union).sum();
        let grouped: usize = batches.iter().map(union).sum();
        assert!(grouped < in_order, "{grouped} < {in_order}");
    }

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
