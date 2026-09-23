//! 蕴含规则（0044 决定 3 的第五片）：一种形状的陈述、或带某个类别词的东西，蕴含另一条
//! 属性的事实。宾语要么就是陈述的宾语，要么由一个读数从宾语的字里读出来。
//!
//! 与绑定同一套生命周期：对齐器提（`propose`，代理），工作台批（`decide_with_delivery`，人），
//! 人的判定不被代理盖；提案带指纹（0053），输入变了对齐器会再提。执行在 `materialize`
//! 里，**没有模型调用**：读数由 `read_phrases` 任务先算进 `phrase_readings`，物化只查缓存，
//! 缓存里没有的这一轮就不算，等缓存填上再来。

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 读数的种类。字符串进库、进提示词，所以是常量而不是枚举；执行它们的是模型，
/// 这里只定义「问什么」。加一种就是加一行——和描述一起给模型看
pub const READINGS: &[(&str, &str)] = &[
    (
        "country_of_nationality",
        "the country a nationality or demonym names (British → United Kingdom, 法国 → France)",
    ),
    (
        "country_of_place",
        "the country a place belongs to (Piedmont region of Virginia → United States, Lyon → France)",
    ),
    (
        "year_of_phrase",
        "the year a phrase gives, as a four-digit number (\"the summer of 1952\" → 1952)",
    ),
];

pub fn reading_is_known(reading: &str) -> bool {
    READINGS.iter().any(|(k, _)| *k == reading)
}

/// 把一个读数的输入归一：与短语、类别词同一条规矩（空白折一个、小写、去两端）
pub fn normalize(s: &str) -> String {
    crate::phrase_bindings::normalize(s)
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Rule {
    pub id: Uuid,
    pub trigger: String,
    pub phrase: String,
    pub subject_type_id: Option<Uuid>,
    pub object_type_id: Option<Uuid>,
    pub object_is_value: bool,
    pub conclude_property_id: Uuid,
    pub reading: Option<String>,
    pub status: String,
    pub votes: Option<serde_json::Value>,
    pub decided_by: String,
    pub basis: Option<String>,
    pub statement_count: i32,
    pub examples: Vec<String>,
    pub decided_at: DateTime<Utc>,
}

/// 对齐器提的一条规则。
pub struct Proposal<'a> {
    /// phrase | kind_word
    pub trigger: &'a str,
    pub phrase: &'a str,
    pub subject_type_id: Option<Uuid>,
    pub object_type_id: Option<Uuid>,
    pub object_is_value: bool,
    pub conclude_property_id: Uuid,
    pub reading: Option<&'a str>,
    /// proposed（要人批）| rejected（模型说这个形状不蕴含什么——记下来免得每轮再问）
    pub status: &'a str,
    pub votes: &'a serde_json::Value,
    pub basis: &'a str,
    pub statement_count: i64,
    pub examples: &'a [String],
}

/// 记下对齐器的提案；同一条规则已经有人判过的原样留着（返回 None）。代理自己的旧行
/// 被新提案覆盖——指纹变了对齐器才会再提，覆盖的是过期的看法
pub async fn propose(pool: &PgPool, kb_id: Uuid, p: &Proposal<'_>) -> AppResult<Option<Uuid>> {
    if !matches!(p.trigger, "phrase" | "kind_word") {
        return Err(AppError::Validation(format!(
            "unknown trigger {:?}",
            p.trigger
        )));
    }
    if !matches!(p.status, "proposed" | "rejected") {
        return Err(AppError::Validation(format!(
            "a proposal is proposed or rejected, not {:?}",
            p.status
        )));
    }
    if let Some(r) = p.reading {
        if !reading_is_known(r) {
            return Err(AppError::Validation(format!("unknown reading {r:?}")));
        }
    }
    let phrase = normalize(p.phrase);
    if phrase.is_empty() {
        return Err(AppError::Validation(
            "an empty phrase implies nothing".into(),
        ));
    }
    let id = Uuid::now_v7();
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO implication_rules
             (id, kb_id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
              conclude_property_id, reading, status, votes, decided_by, basis, statement_count, examples)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'agent', $12, $13, $14)
         ON CONFLICT (kb_id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
                      conclude_property_id, reading) DO UPDATE
            SET status = EXCLUDED.status, votes = EXCLUDED.votes, basis = EXCLUDED.basis,
                statement_count = EXCLUDED.statement_count, examples = EXCLUDED.examples,
                decided_at = now()
          WHERE implication_rules.decided_by = 'agent'
         RETURNING id",
    )
    .bind(id)
    .bind(kb_id)
    .bind(p.trigger)
    .bind(&phrase)
    .bind(p.subject_type_id)
    .bind(if p.object_is_value { None } else { p.object_type_id })
    .bind(p.object_is_value)
    .bind(p.conclude_property_id)
    .bind(p.reading)
    .bind(p.status)
    .bind(p.votes)
    .bind(p.basis)
    .bind(i32::try_from(p.statement_count).unwrap_or(i32::MAX))
    .bind(p.examples)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

pub async fn list(pool: &PgPool, kb_id: Uuid, status: Option<&str>) -> AppResult<Vec<Rule>> {
    Ok(sqlx::query_as(
        "SELECT id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
                conclude_property_id, reading, status, votes, decided_by, basis,
                statement_count, examples, decided_at
           FROM implication_rules
          WHERE kb_id = $1 AND ($2::text IS NULL OR status = $2)
          ORDER BY decided_at, phrase",
    )
    .bind(kb_id)
    .bind(status)
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<Option<Rule>> {
    Ok(sqlx::query_as(
        "SELECT id, trigger, phrase, subject_type_id, object_type_id, object_is_value,
                conclude_property_id, reading, status, votes, decided_by, basis,
                statement_count, examples, decided_at
           FROM implication_rules WHERE kb_id = $1 AND id = $2",
    )
    .bind(kb_id)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// 人批或驳一条规则，与它的后续工作**同一事务**提交（0051 的同一条规矩）。
/// 批准且要读数的：先排 `read_phrases` 把缓存填上（它跑完自己会排物化）；
/// 不要读数的、或驳回的：直接排物化——驳回也要重算，隐含行得退掉
pub async fn decide_with_delivery(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    approve: bool,
    votes: &serde_json::Value,
) -> AppResult<Option<i64>> {
    let mut tx = pool.begin().await?;
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "UPDATE implication_rules
            SET status = $3, votes = $4, decided_by = 'person', decided_at = now()
          WHERE kb_id = $1 AND id = $2
      RETURNING reading",
    )
    .bind(kb_id)
    .bind(id)
    .bind(if approve { "approved" } else { "rejected" })
    .bind(votes)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((reading,)) = row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let kind = if approve && reading.is_some() {
        READ_KIND
    } else {
        crate::phrase_bindings::MATERIALIZE_KIND
    };
    let job = crate::jobs::enqueue_with_max_attempts_tx(
        &mut tx,
        kind,
        serde_json::json!({ "kb_id": kb_id }),
        3,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

/// 填读数缓存的任务的种类。
pub const READ_KIND: &str = "read_phrases";

/// 一条待读的字：哪种读数、读什么。
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct PendingReading {
    pub reading: String,
    pub phrase: String,
}

/// 已批准的规则里，还没有缓存的 (读数, 字)。短语规则读的是陈述的宾语（实体名或字面值），
/// 类别词规则读的是类别词自己。答过「读不出来」的也算缓存过，不再列
pub async fn pending_readings(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<PendingReading>> {
    let sql = format!(
        "SELECT DISTINCT r.reading, {phrase} AS phrase
           FROM implication_rules r
           JOIN facts s ON s.kb_id = r.kb_id AND s.layer = 'open' AND s.invalidated_at IS NULL
           JOIN entities se ON se.id = s.subject_id
      LEFT JOIN entities oe ON oe.id = s.object_id
          WHERE r.kb_id = $1 AND r.status = 'approved' AND r.trigger = 'phrase' AND r.reading IS NOT NULL
            AND {rule_match}
            AND NOT EXISTS (SELECT 1 FROM phrase_readings pr
                             WHERE pr.kb_id = r.kb_id AND pr.reading = r.reading AND pr.phrase = {phrase})
         UNION
         SELECT DISTINCT r.reading, r.phrase
           FROM implication_rules r
          WHERE r.kb_id = $1 AND r.status = 'approved' AND r.trigger = 'kind_word' AND r.reading IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM phrase_readings pr
                             WHERE pr.kb_id = r.kb_id AND pr.reading = r.reading AND pr.phrase = r.phrase)",
        phrase = object_text_sql(),
        rule_match = RULE_MATCH,
    );
    Ok(sqlx::query_as(&sql).bind(kb_id).fetch_all(pool).await?)
}

/// 陈述宾语的字，归一：实体名，或字面值的 value。给读数用
pub(crate) fn object_text_sql() -> &'static str {
    "lower(btrim(regexp_replace(coalesce(oe.canonical_name, s.object_value ->> 'value', s.object_value #>> '{}', ''), '\\s+', ' ', 'g')))"
}

/// 短语规则与陈述的签名匹配（同 materialize 里绑定的 MATCH，把 b 换成 r）
pub(crate) const RULE_MATCH: &str =
    "r.phrase = lower(btrim(regexp_replace(s.phrase, '\\s+', ' ', 'g')))
       AND r.subject_type_id IS NOT DISTINCT FROM se.type_id
       AND r.object_is_value = (s.object_id IS NULL)
       AND (s.object_id IS NULL OR r.object_type_id IS NOT DISTINCT FROM oe.type_id)";

/// 记一条读数的答案：库里的一样东西、一个字面值，或两者都空（读不出来，也记，别再问）。
pub async fn record_reading(
    pool: &PgPool,
    kb_id: Uuid,
    reading: &str,
    phrase: &str,
    entity_id: Option<Uuid>,
    value: Option<&serde_json::Value>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO phrase_readings (kb_id, reading, phrase, entity_id, value)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (kb_id, reading, phrase) DO UPDATE
            SET entity_id = EXCLUDED.entity_id, value = EXCLUDED.value, answered_at = now()",
    )
    .bind(kb_id)
    .bind(reading)
    .bind(normalize(phrase))
    .bind(entity_id)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// 读数读出了一个名字：库里有这样东西就是它，没有就建一个有名字的。
/// 名字事实照 0041 记（`names::record`）——这是召回的桥，读数读出的国家下次就能被认出来
pub async fn resolve_or_create_named(pool: &PgPool, kb_id: Uuid, name: &str) -> AppResult<Uuid> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("an empty name names nothing".into()));
    }
    if let Some(id) = crate::resolution::existing_by_name(pool, kb_id, name).await? {
        return Ok(id);
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(kb_id)
        .bind(name)
        .execute(pool)
        .await?;
    crate::names::record(pool, kb_id, id, name, None, None).await?;
    Ok(id)
}

#[cfg(test)]
#[path = "implication_rules_tests.rs"]
mod tests;
