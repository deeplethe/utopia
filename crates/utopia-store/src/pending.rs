//! 等人点头的事实（见 `docs/decisions/0015`，表在 `migrations/0018`）。
//!
//! 一句记忆抽出来的三元组先落这里，**不进 `facts`**：不上图、不参与检索、
//! 不进推理。人在对话里或 Review 页看见「原句在上、三元组在下」之后点头，
//! 它才经与抽取相同的那条路（`insert_fact` + 证据 + 时态对账）进账本。
//!
//! **为什么另一张表而不是 `facts` 加一列**：五十多处查询按 `invalidated_at IS NULL`
//! 捞活事实，逐个补过滤漏一处就有一条没人点头的事实混进图里——而这张表存在的
//! 全部理由就是防这件事。分开之后忘了读它的后果是「看不见待确认队列」，
//! 方向对了。0013 的 `derived_facts` 是同一个判断。
//!
//! **只拦交互式的单条写入。** 批量摄入仍旧乐观写入 + 事后审阅——那条路不经这里。
//!
//! **两种形状**（0044 第一刀，迁移 0062）：`phrase` 为空是老的带本体形状（谓词、算出来的
//! `valid_*`），点头走 `insert_fact` + 时态对账；`phrase` 非空是一条开放陈述——文档自己
//! 的关系短语、按角色词记的属性、照抄的时间词、引文在块里的位置——点头走
//! `insert_open_statement`，不问本体、不碰世界轴。

use sqlx::PgPool;
use utopia_core::models::PendingFactView;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

use crate::graph::{FactObject, Validity};

/// 抽取器交过来的一条提议。字段与 `insert_fact` / `insert_value_fact` 对齐，
/// 多出 `proposed_predicate`（模型原话）、`chunk_id`（那句记忆）、`proposed_by`（谁说的）。
pub struct Proposal<'a> {
    pub kb_id: Uuid,
    pub subject_id: Uuid,
    /// None = 本体里没有对应的关系（0010）。**人要看见的正是这个空**
    pub predicate_id: Option<Uuid>,
    pub object_id: Option<Uuid>,
    pub object_value: Option<&'a serde_json::Value>,
    pub proposed_predicate: Option<&'a str>,
    pub validity: Validity<'a>,
    pub confidence: f32,
    pub chunk_id: Uuid,
    pub proposed_by: Option<Uuid>,
    /// 经 MCP 提的话，是哪一枚令牌说的（0026）。网页端对话为空
    pub proposed_token: Option<Uuid>,
    /// Some = 一条开放陈述（0044）：文档自己的关系短语，照写。点头后落成
    /// `facts.layer = 'open'`，没有谓词、不写 `valid_*`。None = 老的带本体形状
    pub phrase: Option<&'a str>,
    /// 按文档角色词记的属性：`[{"role": "amount", "value": "$2 million"} |
    /// {"role": "to", "entity_id": "<uuid>"}]`。只在 `phrase` 非空时有意义
    pub qualifiers: Option<&'a serde_json::Value>,
    /// 陈述提到的时间词，照抄、永远不是日期：`[{"text": "March 4, 2011", "char_start": 143}]`，
    /// 偏移是 `chunks.text` 里的字符偏移。只在 `phrase` 非空时有意义
    pub time_words: Option<&'a serde_json::Value>,
    /// 引文在 `chunks.text` 里的字符偏移 `(start, end)`，服务端搜文本算出、从不取模型
    /// 报的数；没定位到传 None。只在 `phrase` 非空时有意义
    pub quote_span: Option<(i32, i32)>,
}

/// 提议的去向。三种「没提」都不是错误——重抽一句记忆会再次算出同样的三元组，
/// 不挡住就等于每抽一次都把人的决定抹掉一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Proposed(Uuid),
    /// 图上已经有一条一样的活事实——不必再问
    AlreadyAsserted,
    /// 已经在队列里等着
    AlreadyPending,
    /// 人拒绝过这个三元组（`rejected_facts`）
    Rejected,
}

pub async fn propose(pool: &PgPool, p: Proposal<'_>) -> AppResult<Outcome> {
    // 开放陈述的短语照写但去掉首尾空白——`insert_open_statement` 也这么做，空短语在这里
    // 就挡下，别让一条点头时才会失败的行进队列
    let phrase = p.phrase.map(str::trim);
    if phrase == Some("") {
        return Err(AppError::invalid(
            "phrase_missing",
            "an open statement keeps the document's relation phrase",
        ));
    }
    // 「图上已经有」：开放陈述在活着的开放行里按 (主语, 短语, 宾语) 找；带本体的形状
    // 照旧按 (主语, 谓词, 宾语)
    let asserted: Option<(i32,)> = match phrase {
        Some(phrase) => {
            sqlx::query_as(
                "SELECT 1 FROM facts
                  WHERE kb_id = $1 AND subject_id = $2
                    AND layer = 'open' AND phrase = $3
                    AND object_id IS NOT DISTINCT FROM $4
                    AND object_value IS NOT DISTINCT FROM $5
                    AND invalidated_at IS NULL
                  LIMIT 1",
            )
            .bind(p.kb_id)
            .bind(p.subject_id)
            .bind(phrase)
            .bind(p.object_id)
            .bind(p.object_value)
            .fetch_optional(pool)
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT 1 FROM facts
                  WHERE kb_id = $1 AND subject_id = $2
                    AND predicate_id IS NOT DISTINCT FROM $3
                    AND object_id IS NOT DISTINCT FROM $4
                    AND object_value IS NOT DISTINCT FROM $5
                    AND invalidated_at IS NULL
                  LIMIT 1",
            )
            .bind(p.kb_id)
            .bind(p.subject_id)
            .bind(p.predicate_id)
            .bind(p.object_id)
            .bind(p.object_value)
            .fetch_optional(pool)
            .await?
        }
    };
    if asserted.is_some() {
        return Ok(Outcome::AlreadyAsserted);
    }
    let pending: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM pending_facts
          WHERE kb_id = $1 AND subject_id = $2
            AND predicate_id IS NOT DISTINCT FROM $3
            AND object_id IS NOT DISTINCT FROM $4
            AND object_value IS NOT DISTINCT FROM $5
            AND phrase IS NOT DISTINCT FROM $6
          LIMIT 1",
    )
    .bind(p.kb_id)
    .bind(p.subject_id)
    .bind(p.predicate_id)
    .bind(p.object_id)
    .bind(p.object_value)
    .bind(phrase)
    .fetch_optional(pool)
    .await?;
    if pending.is_some() {
        return Ok(Outcome::AlreadyPending);
    }
    // 拒绝记录只按 (主语, 谓词 / 短语, 宾语实体) 查。**字面值事实不查**：`rejected_facts`
    // 没有 object_value 列，按 (主语, 谓词) 挡会把「薪水 28000 被拒」扩大成
    // 「薪水这个属性永远不再提」。宁可多问一次，不替人拒掉一个新值。
    // 短语进了键：同一对实体之间换一个短语是另一句话，不被上一句的拒绝连累
    if let Some(object_id) = p.object_id {
        let rejected: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM rejected_facts
              WHERE kb_id = $1 AND subject_id = $2
                AND predicate_id IS NOT DISTINCT FROM $3
                AND object_id = $4
                AND phrase IS NOT DISTINCT FROM $5
              LIMIT 1",
        )
        .bind(p.kb_id)
        .bind(p.subject_id)
        .bind(p.predicate_id)
        .bind(object_id)
        .bind(phrase)
        .fetch_optional(pool)
        .await?;
        if rejected.is_some() {
            return Ok(Outcome::Rejected);
        }
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO pending_facts
             (id, kb_id, subject_id, predicate_id, object_id, object_value, proposed_predicate,
              valid_from, valid_from_precision, valid_to, valid_to_precision,
              confidence, chunk_id, proposed_by, proposed_token,
              phrase, qualifiers, time_words, quote_start, quote_end)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                 $16, $17, $18, $19, $20)",
    )
    .bind(id)
    .bind(p.kb_id)
    .bind(p.subject_id)
    .bind(p.predicate_id)
    .bind(p.object_id)
    .bind(p.object_value)
    .bind(p.proposed_predicate)
    .bind(p.validity.from)
    .bind(p.validity.from_precision)
    .bind(p.validity.to)
    .bind(p.validity.to_precision)
    .bind(p.confidence)
    .bind(p.chunk_id)
    .bind(p.proposed_by)
    .bind(p.proposed_token)
    .bind(phrase)
    .bind(p.qualifiers)
    .bind(p.time_words)
    .bind(p.quote_span.map(|(start, _)| start))
    .bind(p.quote_span.map(|(_, end)| end))
    .execute(pool)
    .await?;
    Ok(Outcome::Proposed(id))
}

/// 给人看的一条待确认项：`utopia_core::models::PendingFactView`。
const VIEW_SELECT: &str = "\
    SELECT p.id, p.subject_id, s.canonical_name AS subject_name,
           p.predicate_id, r.label AS predicate_label, p.proposed_predicate,
           p.object_id, o.canonical_name AS object_name, p.object_value,
           p.valid_from, p.valid_from_precision, p.valid_to, p.valid_to_precision,
           p.confidence, p.chunk_id, c.text AS quote,
           p.proposed_by, u.display_name AS proposed_by_name,
           t.name AS proposed_token_name, p.created_at,
           p.phrase, p.qualifiers, p.time_words, p.quote_start, p.quote_end
      FROM pending_facts p
      JOIN entities s ON s.id = p.subject_id
      LEFT JOIN relation_types r ON r.id = p.predicate_id
      LEFT JOIN entities o ON o.id = p.object_id
      JOIN chunks c ON c.id = p.chunk_id
      LEFT JOIN users u ON u.id = p.proposed_by
      LEFT JOIN personal_tokens t ON t.id = p.proposed_token";

pub async fn list(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<PendingFactView>> {
    Ok(sqlx::query_as(&format!(
        "{VIEW_SELECT} WHERE p.kb_id = $1 ORDER BY p.created_at DESC, p.id DESC LIMIT $2 OFFSET $3"
    ))
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// 一句记忆抽出的全部待确认项——对话里那张卡片按这个取。
pub async fn for_chunk(
    pool: &PgPool,
    kb_id: Uuid,
    chunk_id: Uuid,
) -> AppResult<Vec<PendingFactView>> {
    Ok(sqlx::query_as(&format!(
        "{VIEW_SELECT} WHERE p.kb_id = $1 AND p.chunk_id = $2 ORDER BY p.created_at"
    ))
    .bind(kb_id)
    .bind(chunk_id)
    .fetch_all(pool)
    .await?)
}

async fn get(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<PendingFactView> {
    sqlx::query_as(&format!("{VIEW_SELECT} WHERE p.kb_id = $1 AND p.id = $2"))
        .bind(kb_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// 决策台账要的自包含快照：行删了之后台账上还读得出当时确认或拒绝的是什么。
fn snapshot(v: &PendingFactView) -> serde_json::Value {
    serde_json::json!({
        "subject": v.subject_name,
        "predicate": v.predicate_label,
        "proposed_predicate": v.proposed_predicate,
        "phrase": v.phrase,
        "object": v.object_name,
        "object_value": v.object_value,
        "qualifiers": v.qualifiers,
        "time_words": v.time_words,
        "valid_from": v.valid_from,
        "valid_to": v.valid_to,
        "confidence": v.confidence,
        "quote": v.quote,
        "proposed_by": v.proposed_by_name,
    })
}

pub struct Confirmed {
    pub fact_id: Uuid,
    /// 那句记忆所在的文档：点头之后按它重排时间解析（0045）
    pub document_id: Uuid,
    /// false = 图上已有同一条活事实，这次只补了证据
    pub created: bool,
    /// 时态对账拿不准、进了 `fact_conflicts` 的条数。开放陈述不对账，恒为 0
    pub conflicts: u32,
    pub snapshot: serde_json::Value,
}

/// 人点头：按抽取那条路进账本——落事实、挂证据、时态对账——然后从队列里拿掉。
///
/// **置信度不动。** 人的态度不用浮点数表达（0011 的教训），它在审计台账里。
///
/// 不包事务：`insert_fact` 按 (主语, 谓词, 宾语) 对活事实去重，中途断掉再点一次
/// 只会补证据、不会落第二条，删行在最后——幂等靡有余悸。
pub async fn confirm(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<Confirmed> {
    let v = get(pool, kb_id, id).await?;
    if let Some(phrase) = v.phrase.clone() {
        return confirm_open(pool, kb_id, &v, &phrase).await;
    }
    // 锚点是那句记忆所在文档的日期（0022）——与抽取那条路同一个来源。没起点的
    // 事实从有证据的那一刻起成立，而证据就是这句话
    let attested_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT d.doc_time FROM chunks c JOIN documents d ON d.id = c.document_id
          WHERE c.id = $1",
    )
    .bind(v.chunk_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let validity = Validity {
        from: v.valid_from,
        from_precision: v.valid_from_precision.as_deref(),
        to: v.valid_to,
        to_precision: v.valid_to_precision.as_deref(),
        attested_at,
        // 待定的事实是人或代理提的，日期是直接给的，没经过时间解析
        from_grade: None,
    };
    let (fact_id, created) = match (v.object_id, v.object_value.as_ref()) {
        (Some(object_id), _) => {
            crate::graph::insert_fact(
                pool,
                kb_id,
                v.subject_id,
                v.predicate_id,
                object_id,
                validity,
                v.confidence,
            )
            .await?
        }
        (None, Some(value)) => {
            crate::graph::insert_value_fact(
                pool,
                kb_id,
                v.subject_id,
                v.predicate_id,
                value,
                validity,
                v.confidence,
            )
            .await?
        }
        (None, None) => return Err(no_object()),
    };
    // 证据指回那句记忆。引句就是整句——一条 episode 本来就只有一句
    crate::graph::add_evidence(
        pool,
        fact_id,
        v.chunk_id,
        Some(&v.quote),
        v.proposed_predicate.as_deref(),
    )
    .await?;

    // 与抽取相同的时态对账：带唯一性约束的状态关系，新事实闭合旧事实。
    // 这正是记忆该有的行为——「Mira 交给 Devin 了」说完，Mira 那条就该闭合。
    // 并进已有断言的也对：多了一次观察，时间线的形状可能跟着变（#679）
    let mut conflicts = 0u32;
    if let Some(pid) = v.predicate_id {
        let meta: Option<(bool, bool, String)> = sqlx::query_as(
            "SELECT functional, inverse_functional, temporal FROM relation_types WHERE id = $1",
        )
        .bind(pid)
        .fetch_optional(pool)
        .await?;
        if let Some((functional, inverse_functional, temporal)) = meta {
            if temporal == "state" {
                let mut directions = Vec::new();
                if functional {
                    directions.push(crate::temporal::Uniqueness::SubjectSide);
                }
                // 宾语侧唯一只对实体宾语有意义；字面值没有「谁被指着」
                if inverse_functional && v.object_id.is_some() {
                    directions.push(crate::temporal::Uniqueness::ObjectSide);
                }
                for dir in directions {
                    let report = crate::temporal::reconcile_new_fact(
                        pool,
                        kb_id,
                        fact_id,
                        v.subject_id,
                        pid,
                        v.object_id,
                        v.object_value.as_ref(),
                        dir,
                        validity,
                        v.confidence,
                    )
                    .await?;
                    conflicts += report.conflicts;
                }
            }
        }
    }
    sqlx::query("DELETE FROM pending_facts WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(Confirmed {
        fact_id,
        document_id: document_of(pool, v.chunk_id).await?,
        created,
        conflicts,
        snapshot: snapshot(&v),
    })
}

/// 人对一条开放陈述点头（0044）：按开放抽取那条路进账本——`insert_open_statement`、
/// 带偏移的证据、角色词属性、时间词——然后从队列里拿掉。
///
/// **不问本体、不做时态对账。** 开放陈述没有谓词，`relation_types` 上没有它的
/// functional / temporal 可查；它提到的时间是照抄的字，在世界轴上还没有位置，
/// 没有 `valid_*` 可以拿去闭合谁。把字读成日期、再对账，是 0045 后面几刀的事。
///
/// 与带本体的那条路一样不包事务：`insert_open_statement` 按 (主语, 短语, 宾语) 在活着的
/// 开放行里去重，证据、属性、时间词都是 ON CONFLICT 幂等的，中途断掉再点一次不会落第二行。
async fn confirm_open(
    pool: &PgPool,
    kb_id: Uuid,
    v: &PendingFactView,
    phrase: &str,
) -> AppResult<Confirmed> {
    // 记录轴的锚点只认文档自己说的日期（#714）：`doc_time_source` 是 content / source
    // 才取 `doc_time`，上传时间与文件 mtime 不算。记忆文档是 'none'，于是这里为空，
    // `insert_open_statement` 落成此刻——人此刻说的话，人就是证据
    let attested_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT d.doc_time FROM chunks c JOIN documents d ON d.id = c.document_id
          WHERE c.id = $1 AND d.doc_time_source IN ('content', 'source')",
    )
    .bind(v.chunk_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let object = match (v.object_id, v.object_value.as_ref()) {
        (Some(object_id), _) => FactObject::Entity(object_id),
        (None, Some(value)) => FactObject::Value(value),
        (None, None) => return Err(no_object()),
    };
    let (fact_id, created) = crate::graph::insert_open_statement(
        pool,
        kb_id,
        v.subject_id,
        phrase,
        object,
        attested_at,
        v.confidence,
    )
    .await?;
    // 证据指回那句记忆，带着引文在块里的位置；`proposed_predicate = phrase`，于是所有
    // 已经容得下空谓词的读路径不改一字就按短语显示它。
    // 引文是**那句话**，不是整块：待确认项上的 quote 是块的全文（带时间戳前缀），
    // 有偏移时按偏移截出这一句，偏移才和引文说同一件事（证据表的 quote 只留 120 字）
    let span = v.quote_start.zip(v.quote_end);
    let sentence: String = match span {
        Some((start, end)) if start >= 0 && end > start => v
            .quote
            .chars()
            .skip(start as usize)
            .take((end - start) as usize)
            .collect(),
        _ => v.quote.clone(),
    };
    crate::graph::add_evidence_located(
        pool,
        fact_id,
        v.chunk_id,
        Some(&sentence),
        Some(phrase),
        span,
    )
    .await?;
    for q in json_items(v.qualifiers.as_ref()) {
        let role = q
            .get("role")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                AppError::invalid(
                    "qualifier_role",
                    "a statement qualifier needs the document's role word",
                )
            })?;
        // 带 entity_id 的指向一个实体；否则 value 就是文档里的那个值，原样落成 JSON
        let entity_id = q
            .get("entity_id")
            .and_then(serde_json::Value::as_str)
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|_| {
                AppError::invalid(
                    "qualifier_entity",
                    "a statement qualifier's entity_id must be a uuid",
                )
            })?;
        let value = if entity_id.is_some() {
            None
        } else {
            q.get("value")
        };
        crate::graph::add_statement_qualifier(pool, fact_id, role, value, entity_id).await?;
    }
    for t in json_items(v.time_words.as_ref()) {
        let (Some(text), Some(char_start)) = (
            t.get("text").and_then(serde_json::Value::as_str),
            t.get("char_start")
                .and_then(serde_json::Value::as_i64)
                .and_then(|n| i32::try_from(n).ok()),
        ) else {
            return Err(AppError::invalid(
                "time_word_shape",
                "a time word is the document's text and its char offset in the chunk",
            ));
        };
        // 来自陈述的哪个槽；0062 之前记下的项没有这一字段，它们都是 when
        let role = t
            .get("role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("when");
        crate::time_mentions::record(pool, kb_id, fact_id, v.chunk_id, text, char_start, role)
            .await?;
    }
    sqlx::query("DELETE FROM pending_facts WHERE id = $1")
        .bind(v.id)
        .execute(pool)
        .await?;
    Ok(Confirmed {
        fact_id,
        document_id: document_of(pool, v.chunk_id).await?,
        created,
        conflicts: 0,
        snapshot: snapshot(v),
    })
}

fn no_object() -> AppError {
    AppError::invalid(
        "pending_fact_has_no_object",
        "a pending fact must have an object entity or a literal value",
    )
}

/// 一列 JSON 对象；不是数组（或没有）就是空
fn json_items(v: Option<&serde_json::Value>) -> impl Iterator<Item = &serde_json::Value> {
    v.and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
}

/// 一块所在的文档。
async fn document_of(pool: &PgPool, chunk_id: Uuid) -> AppResult<Uuid> {
    Ok(
        sqlx::query_scalar("SELECT document_id FROM chunks WHERE id = $1")
            .bind(chunk_id)
            .fetch_one(pool)
            .await?,
    )
}

/// 人拒绝：记进 `rejected_facts`（下一轮重抽先查它），从队列里拿掉。
/// 那句记忆本身留着——人确实说过那句话，只是没抽出可用的事实。
/// 开放陈述连短语一起记：拒的是「这句话」，不是这一对实体之间的一切
pub async fn reject(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    rejected_by: Option<Uuid>,
) -> AppResult<serde_json::Value> {
    let v = get(pool, kb_id, id).await?;
    sqlx::query(
        "INSERT INTO rejected_facts (kb_id, subject_id, predicate_id, object_id, rejected_by, phrase)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(kb_id)
    .bind(v.subject_id)
    .bind(v.predicate_id)
    .bind(v.object_id)
    .bind(rejected_by)
    .bind(&v.phrase)
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM pending_facts WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(snapshot(&v))
}
