//! 勘误（0044 决定 7，第六刀）：抽取之后按文档复审类型化图谱的那个 agent 在库里留下的东西。
//!
//! 结构先报（[`candidates`]）：主语或宾语在属性声明的类之外、文档里找不到的名字、日期属性
//! 没有日期。这几条不问模型就报得出，先看它们，其余抽样——0044 §7 的次序。
//!
//! 每一条看过的事实记一笔（keep 也记）：「没看过的」就是没行的，复审不重复；撤、改、加
//! 记成动作，带着文档的原话。动作走 0027 的闸门：撤的事实有派生靠着、主语在回答里被认
//! 过，或者写的事实会让一个只许一个值的谓词有两个值——留给人，agent 不动手。
//!
//! 撤销要站得住：物化下一轮会把活着的陈述再算成同一条类型化行。所以动作记着（陈述, 属性），
//! 物化与蕴含都跳过被勘误撤过的那一对（见 `materialize`）。别的文档说了同一件事照样算——
//! 勘误看的是这份文档，撤的是这份文档产生的那条。

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

use crate::execution_gate::{self, Impact};
use crate::graph::{FactObject, Validity};

/// 排队的任务名：一个库一份，复审有类型化行还没看过的文档
pub const JOB_KIND: &str = "errata_review";

/// 结构报出来的四种理由，与迁移里的 CHECK 同一份
pub const FLAGS: &[&str] = &["domain", "range", "name_absent", "no_date"];

/// 一条送去看的类型化事实：两端的名字与类、属性、来源陈述、结构报的理由、这份文档里的引文
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Candidate {
    pub fact_id: Uuid,
    pub statement_id: Option<Uuid>,
    pub predicate_id: Uuid,
    pub subject_id: Uuid,
    pub subject: String,
    pub subject_class: Option<String>,
    pub property: String,
    pub property_label: String,
    pub object_id: Option<Uuid>,
    pub object: String,
    pub object_class: Option<String>,
    /// 空 = 结构没报，抽样看
    pub flag: Option<String>,
    pub quote: Option<String>,
}

/// 类的祖先闭包在 SQL 里算：属性声明在 legal_entity 上，organization 是它的子类就在域内。
/// 没有类的一端不报——「不知道是什么」不是「在类之外」
const CANDIDATES_SQL: &str = r#"
WITH RECURSIVE up AS (
    SELECT id AS child, id AS anc FROM entity_types WHERE kb_id = $1
    UNION
    SELECT up.child, p.parent_id FROM up JOIN entity_type_parents p ON p.child_id = up.anc
),
closure AS (SELECT child, array_agg(anc) AS ancs FROM up GROUP BY child),
doc AS (
    SELECT lower(string_agg(text, ' ' ORDER BY seq)) AS text
      FROM chunks WHERE document_id = $2 AND superseded_at IS NULL
),
sources AS (
    SELECT src.fact_id, src.statement_id FROM typed_fact_sources src
      JOIN fact_evidence fe ON fe.fact_id = src.statement_id AND fe.document_id = $2
    UNION
    SELECT i.fact_id, i.statement_id FROM implied_fact_sources i
      JOIN fact_evidence fe ON fe.fact_id = i.statement_id AND fe.document_id = $2
     WHERE i.statement_id IS NOT NULL
),
typed AS (SELECT DISTINCT ON (fact_id) fact_id, statement_id FROM sources ORDER BY fact_id, statement_id)
SELECT * FROM (
SELECT t.id AS fact_id, ty.statement_id, t.predicate_id, t.subject_id, t.recorded_at,
       s.canonical_name AS subject, st.key AS subject_class,
       r.key AS property, r.label AS property_label,
       t.object_id,
       COALESCE(o.canonical_name, t.object_value #>> '{value}', t.object_value::text, '') AS object,
       ot.key AS object_class,
       CASE
         WHEN s.type_id IS NOT NULL
          AND EXISTS (SELECT 1 FROM relation_type_domains d WHERE d.relation_type_id = r.id)
          AND NOT EXISTS (SELECT 1 FROM relation_type_domains d
                           WHERE d.relation_type_id = r.id AND d.entity_type_id = ANY(sc.ancs))
           THEN 'domain'
         WHEN o.type_id IS NOT NULL
          AND EXISTS (SELECT 1 FROM relation_type_ranges g WHERE g.relation_type_id = r.id)
          AND NOT EXISTS (SELECT 1 FROM relation_type_ranges g
                           WHERE g.relation_type_id = r.id AND g.entity_type_id = ANY(oc.ancs))
           THEN 'range'
         WHEN r.datatype = 'date' AND t.object_id IS NULL
          AND NOT COALESCE((t.object_value #>> '{value}') ~ '^\d{4}(-\d{2}(-\d{2})?)?', false)
           THEN 'no_date'
         WHEN position(lower(s.canonical_name) IN doc.text) = 0
           OR (o.id IS NOT NULL AND position(lower(o.canonical_name) IN doc.text) = 0)
           THEN 'name_absent'
       END AS flag,
       (SELECT fe.quote FROM fact_evidence fe
         WHERE fe.fact_id = t.id AND fe.document_id = $2 AND fe.quote IS NOT NULL
         ORDER BY fe.chunk_id LIMIT 1) AS quote
  FROM typed ty
  JOIN facts t ON t.id = ty.fact_id
  JOIN entities s ON s.id = t.subject_id
  LEFT JOIN entity_types st ON st.id = s.type_id
  LEFT JOIN closure sc ON sc.child = s.type_id
  JOIN relation_types r ON r.id = t.predicate_id
  LEFT JOIN entities o ON o.id = t.object_id
  LEFT JOIN entity_types ot ON ot.id = o.type_id
  LEFT JOIN closure oc ON oc.child = o.type_id
  CROSS JOIN doc
 WHERE t.kb_id = $1 AND t.layer = 'typed' AND t.invalidated_at IS NULL
   AND NOT EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.fact_id = t.id)
) c
ORDER BY (c.flag IS NULL), c.recorded_at, c.fact_id
"#;

/// 这份文档产生的、活着的、还没看过的类型化事实，结构报了的在前
pub async fn candidates(
    pool: &PgPool,
    kb_id: Uuid,
    document_id: Uuid,
) -> AppResult<Vec<Candidate>> {
    Ok(sqlx::query_as(CANDIDATES_SQL)
        .bind(kb_id)
        .bind(document_id)
        .fetch_all(pool)
        .await?)
}

/// 文档当前版本的正文，按分块顺序接起来
pub async fn document_text(pool: &PgPool, document_id: Uuid) -> AppResult<String> {
    let text: Option<String> = sqlx::query_scalar(
        "SELECT string_agg(text, ' ' ORDER BY seq) FROM chunks
          WHERE document_id = $1 AND superseded_at IS NULL",
    )
    .bind(document_id)
    .fetch_one(pool)
    .await?;
    Ok(text.unwrap_or_default())
}

/// 还有活着的类型化行没看过的文档，最早摄入的在前
pub async fn documents_due(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT d.id FROM documents d
          WHERE d.kb_id = $1 AND d.deleted_at IS NULL
            AND EXISTS (
              SELECT 1 FROM fact_evidence fe
                JOIN (SELECT fact_id, statement_id FROM typed_fact_sources
                      UNION ALL
                      SELECT fact_id, statement_id FROM implied_fact_sources
                       WHERE statement_id IS NOT NULL) src ON src.statement_id = fe.fact_id
                JOIN facts t ON t.id = src.fact_id
               WHERE fe.document_id = d.id AND t.invalidated_at IS NULL
                 AND NOT EXISTS (SELECT 1 FROM errata_actions ea WHERE ea.fact_id = t.id))
          ORDER BY d.created_at, d.id
          LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// 一次复审开账
pub async fn start_run(
    pool: &PgPool,
    kb_id: Uuid,
    document_id: Uuid,
    flagged: i32,
    sampled: i32,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO errata_runs (id, kb_id, document_id, flagged, sampled) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(document_id)
    .bind(flagged)
    .bind(sampled)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 结账：问了几次、端点报了多少用量（没报就空着）
pub async fn finish_run(
    pool: &PgPool,
    run_id: Uuid,
    requests: i32,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE errata_runs SET requests = $2, prompt_tokens = $3, completion_tokens = $4,
                finished_at = now() WHERE id = $1",
    )
    .bind(run_id)
    .bind(requests)
    .bind(prompt_tokens)
    .bind(completion_tokens)
    .execute(pool)
    .await?;
    Ok(())
}

/// agent 对一条事实的说法，或它想加的一条
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proposed {
    Keep,
    Retract,
    /// 换属性（键）、换宾语（名字或值），至少一样
    Revise {
        property: Option<String>,
        object: Option<String>,
    },
    Add {
        subject: String,
        property: String,
        object: String,
    },
}

impl Proposed {
    pub fn action(&self) -> &'static str {
        match self {
            Proposed::Keep => "keep",
            Proposed::Retract => "retract",
            Proposed::Revise { .. } => "revise",
            Proposed::Add { .. } => "add",
        }
    }
}

pub struct ActionInput<'a> {
    pub run_id: Uuid,
    pub document_id: Uuid,
    /// keep / retract / revise 看的那条；add 没有
    pub candidate: Option<&'a Candidate>,
    pub proposed: Proposed,
    pub reason: &'a str,
    pub quote: Option<&'a str>,
    /// 文档正文，引文按它验
    pub document_text: &'a str,
}

/// 一笔记下来的动作：落了地、留给人了、还是没过验证
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recorded {
    pub id: Uuid,
    pub status: &'static str,
    /// 留给人的理由，或没过验证的理由
    pub detail: Option<String>,
    pub new_fact_id: Option<Uuid>,
}

/// 引文是不是文档的原话：折掉大小写与空白后的连续一段。空引文不算
pub fn quote_in(document: &str, quote: &str) -> bool {
    let q = squash(quote);
    !q.is_empty() && squash(document).contains(&q)
}

fn squash(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 动作指向的那条事实：名字给队列卡片看，id 给执行用
struct Target {
    subject_id: Uuid,
    predicate_id: Uuid,
    object_id: Option<Uuid>,
    object_value: Option<Value>,
    subject: String,
    property: String,
    object: String,
}

impl Target {
    fn json(&self) -> Value {
        json!({
            "subject": self.subject, "property": self.property, "object": self.object,
            "subject_id": self.subject_id, "predicate_id": self.predicate_id,
            "object_id": self.object_id, "object_value": self.object_value,
        })
    }
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            subject_id: v.get("subject_id")?.as_str()?.parse().ok()?,
            predicate_id: v.get("predicate_id")?.as_str()?.parse().ok()?,
            object_id: v
                .get("object_id")
                .and_then(|x| x.as_str())
                .and_then(|s| s.parse().ok()),
            object_value: v.get("object_value").filter(|x| !x.is_null()).cloned(),
            subject: v["subject"].as_str().unwrap_or("").to_string(),
            property: v["property"].as_str().unwrap_or("").to_string(),
            object: v["object"].as_str().unwrap_or("").to_string(),
        })
    }
}

/// 属性键在这个库里对应的行：id、是关系还是属性
async fn property_by_key(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
) -> AppResult<Option<(Uuid, String)>> {
    Ok(
        sqlx::query_as("SELECT id, kind FROM relation_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(key.trim())
            .fetch_optional(pool)
            .await?,
    )
}

/// 宾语的文本按属性的种类解：关系要库里已有的名字（勘误不造东西），属性落成值
async fn object_of(
    pool: &PgPool,
    kb_id: Uuid,
    kind: &str,
    text: &str,
) -> AppResult<Result<(Option<Uuid>, Option<Value>), String>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Err("empty object".into()));
    }
    if kind == "relation" {
        match crate::resolution::existing_by_name(pool, kb_id, text).await? {
            Some(id) => Ok(Ok((Some(id), None))),
            None => Ok(Err(format!("no thing named \"{text}\""))),
        }
    } else {
        Ok(Ok((None, Some(json!({ "value": text })))))
    }
}

/// 记一笔并（能的话）执行。keep 只记；撤、改、加先验引文，再过闸门，过了才动图。
/// 没过验证的也记（refused）：这条事实算看过了，agent 的说法留着给人看，但不再问
pub async fn record(pool: &PgPool, kb_id: Uuid, input: ActionInput<'_>) -> AppResult<Recorded> {
    let id = Uuid::now_v7();
    let action = input.proposed.action();
    let c = input.candidate;
    if !matches!(input.proposed, Proposed::Add { .. }) && c.is_none() {
        return Err(AppError::Validation(format!(
            "{action} needs the fact it is about"
        )));
    }
    let (fact_id, statement_id, flag) = match c {
        Some(c) => (Some(c.fact_id), c.statement_id, c.flag.clone()),
        None => (None, None, None),
    };
    // 动作指向的那条事实，与验证的结果
    let resolved: Result<Option<Target>, String> = match &input.proposed {
        Proposed::Keep => Ok(None),
        Proposed::Retract => {
            let c = c.unwrap();
            Ok(Some(Target {
                subject_id: c.subject_id,
                predicate_id: c.predicate_id,
                object_id: c.object_id,
                object_value: None,
                subject: c.subject.clone(),
                property: c.property.clone(),
                object: c.object.clone(),
            }))
        }
        Proposed::Revise { property, object } => {
            let c = c.unwrap();
            let prop = match property.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
                None => Ok((c.predicate_id, None)),
                Some(key) => match property_by_key(pool, kb_id, key).await? {
                    Some((id, kind)) => Ok((id, Some(kind))),
                    None => Err(format!("no property with key \"{key}\"")),
                },
            };
            match prop {
                Err(e) => Err(e),
                Ok((predicate_id, kind)) => {
                    let kind = match kind {
                        Some(k) => k,
                        None => {
                            sqlx::query_scalar("SELECT kind FROM relation_types WHERE id = $1")
                                .bind(predicate_id)
                                .fetch_one(pool)
                                .await?
                        }
                    };
                    let old: (Option<Uuid>, Option<Value>) =
                        sqlx::query_as("SELECT object_id, object_value FROM facts WHERE id = $1")
                            .bind(c.fact_id)
                            .fetch_one(pool)
                            .await?;
                    let obj = match object.as_deref() {
                        Some(text) => object_of(pool, kb_id, &kind, text).await?,
                        None => Ok(old.clone()),
                    };
                    match obj {
                        Err(e) => Err(e),
                        Ok((object_id, object_value))
                            if predicate_id == c.predicate_id
                                && object_id == old.0
                                && object_value == old.1 =>
                        {
                            Err("the revision changes nothing".into())
                        }
                        Ok((object_id, object_value)) => {
                            let property = match property {
                                Some(k) => k.trim().to_string(),
                                None => c.property.clone(),
                            };
                            Ok(Some(Target {
                                subject_id: c.subject_id,
                                predicate_id,
                                object_id,
                                object_value,
                                subject: c.subject.clone(),
                                property,
                                object: object.clone().unwrap_or_else(|| c.object.clone()),
                            }))
                        }
                    }
                }
            }
        }
        Proposed::Add {
            subject,
            property,
            object,
        } => {
            let subject_id =
                crate::resolution::existing_by_name(pool, kb_id, subject.trim()).await?;
            match (subject_id, property_by_key(pool, kb_id, property).await?) {
                (None, _) => Err(format!("no thing named \"{}\"", subject.trim())),
                (_, None) => Err(format!("no property with key \"{}\"", property.trim())),
                (Some(subject_id), Some((predicate_id, kind))) => {
                    match object_of(pool, kb_id, &kind, object).await? {
                        Err(e) => Err(e),
                        Ok((object_id, object_value)) => Ok(Some(Target {
                            subject_id,
                            predicate_id,
                            object_id,
                            object_value,
                            subject: subject.trim().to_string(),
                            property: property.trim().to_string(),
                            object: object.trim().to_string(),
                        })),
                    }
                }
            }
        }
    };
    // 撤、改、加都得引文档的原话；不是原话的说法记下来，不执行
    let resolved = match resolved {
        Ok(t) => {
            if input.proposed != Proposed::Keep
                && !input
                    .quote
                    .is_some_and(|q| quote_in(input.document_text, q))
            {
                Err("the quote is not in the document".to_string())
            } else {
                Ok(t)
            }
        }
        e => e,
    };
    let target_json = match &resolved {
        Ok(Some(t)) => Some(t.json()),
        _ => None,
    };
    // 0027 的闸门：撤的那条有什么靠着它，写的那条会不会立刻开出违规
    let hold = match &resolved {
        Ok(Some(t)) => {
            let mut impact = Impact::default();
            if let Some(fact) = fact_id.filter(|_| input.proposed != Proposed::Keep) {
                impact = execution_gate::impact_of_fact(pool, kb_id, fact).await?;
            }
            if !matches!(input.proposed, Proposed::Retract) {
                let replacing =
                    fact_id.filter(|_| matches!(input.proposed, Proposed::Revise { .. }));
                let w = execution_gate::impact_of_write(
                    pool,
                    kb_id,
                    t.subject_id,
                    t.predicate_id,
                    t.object_id,
                    replacing,
                )
                .await?;
                impact.contradictions.extend(w.contradictions);
            }
            execution_gate::hold(&impact)
        }
        _ => None,
    };
    let (status, detail): (&'static str, Option<String>) = match (&resolved, &hold) {
        (Err(e), _) => ("refused", Some(e.clone())),
        (Ok(_), Some(h)) => ("held", Some(h.to_string())),
        (Ok(_), None) => ("applied", None),
    };
    sqlx::query(
        "INSERT INTO errata_actions
             (id, kb_id, run_id, document_id, fact_id, statement_id, predicate_id, flag,
              action, reason, quote, proposed, status, detail)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(input.run_id)
    .bind(input.document_id)
    .bind(fact_id)
    .bind(statement_id)
    .bind(c.map(|c| c.predicate_id))
    .bind(&flag)
    .bind(action)
    .bind(input.reason)
    .bind(input.quote)
    .bind(&target_json)
    .bind(status)
    .bind(&detail)
    .execute(pool)
    .await?;
    let mut new_fact_id = None;
    if status == "applied" {
        if let Ok(Some(t)) = &resolved {
            new_fact_id = apply(
                pool,
                kb_id,
                action,
                fact_id,
                input.document_id,
                t,
                input.quote,
            )
            .await?;
            if new_fact_id.is_some() {
                sqlx::query("UPDATE errata_actions SET new_fact_id = $2 WHERE id = $1")
                    .bind(id)
                    .bind(new_fact_id)
                    .execute(pool)
                    .await?;
            }
        }
    }
    Ok(Recorded {
        id,
        status,
        detail,
        new_fact_id,
    })
}

/// 动图：撤是作废（`reject_fact`，证据不动，台账留痕）；改是撤旧写新，新行接旧行的时间；
/// 加是写一行，锚在文档的日期上。新行的证据是引文所在的分块。
/// 已经不在的行（别处先撤了）不算错——要的结果已经在了
async fn apply(
    pool: &PgPool,
    kb_id: Uuid,
    action: &str,
    fact_id: Option<Uuid>,
    document_id: Uuid,
    target: &Target,
    quote: Option<&str>,
) -> AppResult<Option<Uuid>> {
    let mut validity_row: Option<ValidityRow> = None;
    if matches!(action, "retract" | "revise") {
        let fact = fact_id.ok_or_else(|| AppError::Validation("no fact to retract".into()))?;
        validity_row = sqlx::query_as(
            "SELECT valid_from, valid_from_precision, valid_from_grade, valid_to, valid_to_precision,
                    attested_from FROM facts WHERE id = $1",
        )
        .bind(fact)
        .fetch_optional(pool)
        .await?;
        match crate::graph::reject_fact(pool, kb_id, fact).await {
            Ok(()) | Err(AppError::NotFound) => {}
            Err(e) => return Err(e),
        }
    }
    if action == "retract" {
        return Ok(None);
    }
    let doc_time: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT doc_time FROM documents WHERE id = $1")
            .bind(document_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let v = validity_row.unwrap_or_default();
    let validity = Validity {
        from: v.valid_from,
        from_precision: v.valid_from_precision.as_deref(),
        from_grade: v.valid_from_grade.as_deref(),
        to: v.valid_to,
        to_precision: v.valid_to_precision.as_deref(),
        attested_at: v.attested_from.or(doc_time),
    };
    let object = match (target.object_id, &target.object_value) {
        (Some(o), _) => FactObject::Entity(o),
        (None, Some(val)) => FactObject::Value(val),
        _ => return Err(AppError::Validation("a fact needs an object".into())),
    };
    let mut conn = pool.acquire().await?;
    let (new, _) = crate::graph::insert_fact_on(
        &mut conn,
        kb_id,
        target.subject_id,
        Some(target.predicate_id),
        object,
        validity,
        0.9,
    )
    .await?;
    // 证据：引文所在的分块；引文跨块时退到文档的第一块
    let placed = sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         SELECT $1, c.id, $3, c.document_id, c.doc_version FROM chunks c
          WHERE c.document_id = $2 AND c.superseded_at IS NULL
            AND position(lower($3) IN lower(c.text)) > 0
          ORDER BY c.seq LIMIT 1
         ON CONFLICT DO NOTHING",
    )
    .bind(new)
    .bind(document_id)
    .bind(quote.unwrap_or(""))
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if placed == 0 {
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
             SELECT $1, c.id, $3, c.document_id, c.doc_version FROM chunks c
              WHERE c.document_id = $2 AND c.superseded_at IS NULL
              ORDER BY c.seq LIMIT 1
             ON CONFLICT DO NOTHING",
        )
        .bind(new)
        .bind(document_id)
        .bind(quote.unwrap_or(""))
        .execute(&mut *conn)
        .await?;
    }
    Ok(Some(new))
}

#[derive(Debug, Default, sqlx::FromRow)]
struct ValidityRow {
    valid_from: Option<DateTime<Utc>>,
    valid_from_precision: Option<String>,
    valid_from_grade: Option<String>,
    valid_to: Option<DateTime<Utc>>,
    valid_to_precision: Option<String>,
    attested_from: Option<DateTime<Utc>>,
}

/// 闸门留给人的一笔：看的是哪份文档的哪条事实、agent 想怎么办、凭哪句原话、为什么留下
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct HeldItem {
    pub id: Uuid,
    pub document_id: Uuid,
    pub document: String,
    pub action: String,
    pub flag: Option<String>,
    pub fact_id: Option<Uuid>,
    pub proposed: Option<Value>,
    pub reason: String,
    pub quote: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn held(pool: &PgPool, kb_id: Uuid, limit: i64, offset: i64) -> AppResult<Vec<HeldItem>> {
    Ok(sqlx::query_as(
        "SELECT ea.id, ea.document_id, d.filename AS document, ea.action, ea.flag, ea.fact_id,
                ea.proposed, ea.reason, ea.quote, ea.detail, ea.created_at
           FROM errata_actions ea JOIN documents d ON d.id = ea.document_id
          WHERE ea.kb_id = $1 AND ea.status = 'held'
          ORDER BY ea.created_at, ea.id
          LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// 等人的几笔，最老的从什么时候起等
pub async fn waiting(pool: &PgPool, kb_id: Uuid) -> AppResult<(i64, Option<DateTime<Utc>>)> {
    Ok(sqlx::query_as(
        "SELECT count(*), min(created_at) FROM errata_actions WHERE kb_id = $1 AND status = 'held'",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?)
}

#[derive(sqlx::FromRow)]
struct HeldRow {
    action: String,
    fact_id: Option<Uuid>,
    document_id: Uuid,
    proposed: Option<Value>,
    quote: Option<String>,
}

/// 人答留给人的一笔：批了就执行，否了就记 rejected。不是 held 的（已经答过、别的库的）回 false
pub async fn decide_held(
    pool: &PgPool,
    kb_id: Uuid,
    action_id: Uuid,
    approve: bool,
    actor: Uuid,
) -> AppResult<bool> {
    let row: Option<HeldRow> = sqlx::query_as(
        "SELECT action, fact_id, document_id, proposed, quote FROM errata_actions
          WHERE id = $1 AND kb_id = $2 AND status = 'held'",
    )
    .bind(action_id)
    .bind(kb_id)
    .fetch_optional(pool)
    .await?;
    let Some(HeldRow {
        action,
        fact_id,
        document_id,
        proposed,
        quote,
    }) = row
    else {
        return Ok(false);
    };
    let mut new_fact_id = None;
    if approve {
        let target = proposed
            .as_ref()
            .and_then(Target::from_json)
            .ok_or_else(|| AppError::Validation("the held action has no target".into()))?;
        new_fact_id = apply(
            pool,
            kb_id,
            &action,
            fact_id,
            document_id,
            &target,
            quote.as_deref(),
        )
        .await?;
    }
    sqlx::query(
        "UPDATE errata_actions SET status = $3, new_fact_id = $4, decided_by = $5, decided_at = now()
          WHERE id = $1 AND kb_id = $2 AND status = 'held'",
    )
    .bind(action_id)
    .bind(kb_id)
    .bind(if approve { "applied" } else { "rejected" })
    .bind(new_fact_id)
    .bind(actor)
    .execute(pool)
    .await?;
    Ok(true)
}

#[cfg(test)]
mod quote_tests {
    use super::quote_in;

    #[test]
    fn a_quote_is_the_documents_words_up_to_case_and_spacing() {
        let doc = "Acme is based in London.\n  Jane Roe runs Acme.";
        assert!(quote_in(doc, "based in London"));
        assert!(quote_in(doc, "jane roe   runs acme"));
        assert!(!quote_in(doc, "based in Paris"));
        assert!(!quote_in(doc, ""));
        assert!(!quote_in(doc, "   "));
    }
}

#[cfg(test)]
#[path = "errata_tests.rs"]
mod tests;
