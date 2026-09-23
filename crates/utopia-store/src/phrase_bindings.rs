//! 关系短语按签名绑到属性的账本侧（0044 决定 3 的第二片，见 0066）。
//!
//! 开放陈述的关系是文档自己的短语（`facts.phrase`），不选属性。这里做三件事：数出一个
//! 库里有哪些签名（短语 × 主语的类 × 宾语的类或「值」），各带几条例句；记下每个签名判成
//! 了什么（绑定）并判哪些过期了；给下一片（物化）一张「签名 → 属性、方向」的表。判定
//! 本身——问模型、两票一致才绑——在 server 的 `phrase_alignment` 里，这里不认识模型。
//!
//! 短语的归一同类别词：空白折成一个空格、去两端、小写；[`normalize`] 与 [`PHRASE_SQL`]
//! 必须说同一件事。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::HashMap;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// SQL 侧的归一：与 [`normalize`] 一致。`$col` 由调用处替换成列名。
const PHRASE_SQL: &str = "lower(btrim(regexp_replace($col, '\\s+', ' ', 'g')))";

fn phrase_sql(col: &str) -> String {
    PHRASE_SQL.replace("$col", col)
}

/// 归一一个短语：空白折成一个空格、去两端、小写。"Acquired " 与 "acquired" 是一个短语。
pub fn normalize(phrase: &str) -> String {
    phrase
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 一条签名：判它该绑到哪个属性时给模型看的全部。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PhraseSignature {
    /// 归一过的短语
    pub phrase: String,
    /// 主语的类（类别词还没绑到类时为空）
    pub subject_type_id: Option<Uuid>,
    pub subject_type_key: Option<String>,
    /// 宾语的类；宾语是字面值时为空且 `object_is_value` 为真
    pub object_type_id: Option<Uuid>,
    pub object_type_key: Option<String>,
    pub object_is_value: bool,
    /// 这个签名下活着的开放陈述数
    pub count: i64,
    /// 例句：最多 3 条「主语 —短语→ 宾语」，每条跟着它自己的引文
    pub examples: Vec<String>,
    pub quotes: Vec<String>,
}

/// 库里每条 distinct 的签名：活着的开放陈述，按短语、两端的类、宾语是不是字面值分组。
pub async fn signatures(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<PhraseSignature>> {
    // 一条陈述可以有多条证据。计数、取例句前先为每条陈述选一条稳定的引用，
    // 优先有完整位置的证据，避免证据多的陈述挤掉其他陈述。
    let sql = format!(
        "WITH live AS (
             SELECT f.id, {phrase} AS phrase,
                    s.type_id AS subject_type_id, o.type_id AS object_type_id,
                    (f.object_id IS NULL) AS object_is_value,
                    s.canonical_name AS subject_name,
                    coalesce(o.canonical_name, f.object_value #>> '{{value}}', '') AS object_name,
                    f.phrase AS spelling, fe.chunk_id, fe.quote_start, fe.quote_end, f.recorded_at
             FROM facts f
             JOIN entities s ON s.id = f.subject_id
             LEFT JOIN entities o ON o.id = f.object_id
             LEFT JOIN LATERAL (
                 SELECT chunk_id, quote_start, quote_end FROM fact_evidence
                 WHERE fact_id = f.id
                 ORDER BY (quote_start IS NULL OR quote_end IS NULL), chunk_id
                 LIMIT 1
             ) fe ON true
             WHERE f.kb_id = $1 AND f.layer = 'open' AND f.invalidated_at IS NULL
               AND f.phrase IS NOT NULL AND btrim(f.phrase) <> ''
         ),
         grouped AS (
             SELECT phrase, subject_type_id, object_type_id, object_is_value, count(*) AS count
             FROM live GROUP BY phrase, subject_type_id, object_type_id, object_is_value
         ),
         picked AS (
             SELECT g.phrase, g.subject_type_id, g.object_type_id, g.object_is_value,
                    l.subject_name || ' —' || l.spelling || '→ ' || l.object_name AS example,
                    coalesce(substr(c.text, l.quote_start + 1, l.quote_end - l.quote_start), '') AS quote,
                    row_number() OVER (PARTITION BY g.phrase, g.subject_type_id, g.object_type_id, g.object_is_value
                                       ORDER BY l.recorded_at, l.id) AS rn
             FROM grouped g
             JOIN live l ON l.phrase = g.phrase
                        AND l.subject_type_id IS NOT DISTINCT FROM g.subject_type_id
                        AND l.object_type_id IS NOT DISTINCT FROM g.object_type_id
                        AND l.object_is_value = g.object_is_value
             LEFT JOIN chunks c ON c.id = l.chunk_id
         )
         SELECT g.phrase, g.subject_type_id, st.key AS subject_type_key,
                g.object_type_id, ot.key AS object_type_key, g.object_is_value, g.count,
                ARRAY(SELECT p.example FROM picked p
                      WHERE p.phrase = g.phrase AND p.subject_type_id IS NOT DISTINCT FROM g.subject_type_id
                        AND p.object_type_id IS NOT DISTINCT FROM g.object_type_id
                        AND p.object_is_value = g.object_is_value AND p.rn <= 3 ORDER BY p.rn) AS examples,
                ARRAY(SELECT p.quote FROM picked p
                      WHERE p.phrase = g.phrase AND p.subject_type_id IS NOT DISTINCT FROM g.subject_type_id
                        AND p.object_type_id IS NOT DISTINCT FROM g.object_type_id
                        AND p.object_is_value = g.object_is_value AND p.rn <= 3 ORDER BY p.rn) AS quotes
         FROM grouped g
         LEFT JOIN entity_types st ON st.id = g.subject_type_id
         LEFT JOIN entity_types ot ON ot.id = g.object_type_id
         ORDER BY g.count DESC, g.phrase",
        phrase = phrase_sql("f.phrase")
    );
    Ok(sqlx::query_as(&sql).bind(kb_id).fetch_all(pool).await?)
}

/// 按 id 取一条绑定的签名（人在队列里定它时用）：短语、两端的类、宾语是不是字面值，
/// 例句照判定时记下的，陈述数同。
pub async fn signature_of(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
) -> AppResult<Option<PhraseSignature>> {
    Ok(sqlx::query_as(
        "SELECT b.phrase, b.subject_type_id, st.key AS subject_type_key,
                b.object_type_id, ot.key AS object_type_key, b.object_is_value,
                b.statement_count::bigint AS count, b.examples, '{}'::text[] AS quotes
           FROM phrase_bindings b
      LEFT JOIN entity_types st ON st.id = b.subject_type_id
      LEFT JOIN entity_types ot ON ot.id = b.object_type_id
          WHERE b.kb_id = $1 AND b.id = $2",
    )
    .bind(kb_id)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// 一条签名判成了什么。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Binding {
    pub phrase: String,
    pub subject_type_id: Option<Uuid>,
    pub object_type_id: Option<Uuid>,
    pub object_is_value: bool,
    /// `status = 'bound'` 时有值
    pub relation_type_id: Option<Uuid>,
    /// forward / reverse，`status = 'bound'` 时有值
    pub direction: Option<String>,
    /// bound / none / undecided
    pub status: String,
    pub decided_at: DateTime<Utc>,
    /// agent / person
    pub decided_by: String,
    /// 判定时输入的指纹；NULL = 这一列出现之前的判定
    pub basis: Option<String>,
}

impl Binding {
    /// 和签名对上的键：短语 + 两端的类 + 宾语是不是字面值
    pub fn key(&self) -> (String, Option<Uuid>, Option<Uuid>, bool) {
        (
            self.phrase.clone(),
            self.subject_type_id,
            self.object_type_id,
            self.object_is_value,
        )
    }
}

impl PhraseSignature {
    pub fn key(&self) -> (String, Option<Uuid>, Option<Uuid>, bool) {
        (
            self.phrase.clone(),
            self.subject_type_id,
            self.object_type_id,
            self.object_is_value,
        )
    }
}

/// 库里全部绑定。
pub async fn bindings(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Binding>> {
    Ok(sqlx::query_as(
        "SELECT phrase, subject_type_id, object_type_id, object_is_value,
                relation_type_id, direction, status, decided_at, decided_by, basis
         FROM phrase_bindings WHERE kb_id = $1 ORDER BY phrase",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 一条判定看到的输入的指纹（0053）：两端类的祖先闭包（含自己）、宾语是不是字面值、
/// 按继承命中的候选属性与各自的 `updated_at`。worker 每轮对活着的签名重算，与存下的
/// 不一致就是过期——比的是**现在的输入**，不是时刻，于是父边的增删、模型请求途中的
/// 编辑（#795）都看得见，时间戳看不见。
///
/// 只是缓存失效的键，不是安全用途：FNV-1a 64 位够用，也不用为它拉一个哈希依赖。
pub fn basis_of(
    subject_closure: &[Uuid],
    object_closure: &[Uuid],
    object_is_value: bool,
    candidates: &[(Uuid, DateTime<Utc>)],
) -> String {
    let sorted = |ids: &[Uuid]| {
        let mut v: Vec<String> = ids.iter().map(|u| u.to_string()).collect();
        v.sort();
        v.join(",")
    };
    let mut cands: Vec<String> = candidates
        .iter()
        .map(|(id, at)| format!("{id}@{}", at.to_rfc3339()))
        .collect();
    cands.sort();
    let text = format!(
        "s={};o={};v={};c={}",
        sorted(subject_closure),
        sorted(object_closure),
        object_is_value,
        cands.join(",")
    );
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 库里每条属性最后一次改动的时刻，给指纹用。一轮读一次，不进视图——视图是给页面的，
/// 页面不需要这个数
pub async fn property_versions(
    pool: &PgPool,
    kb_id: Uuid,
) -> AppResult<HashMap<Uuid, DateTime<Utc>>> {
    let rows: Vec<(Uuid, DateTime<Utc>)> =
        sqlx::query_as("SELECT id, updated_at FROM relation_types WHERE kb_id = $1")
            .bind(kb_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

/// 不再成立的绑定：绑到的属性在判定之后改过；或判成 none / undecided 之后库里有属性
/// 新建或修改。负向判定没有选中的属性，已有属性的新定义也可能让它对得上。
/// 属性或类被删了的，行已随级联消失。
pub async fn stale(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Binding>> {
    Ok(sqlx::query_as(
        "SELECT b.phrase, b.subject_type_id, b.object_type_id, b.object_is_value,
                b.relation_type_id, b.direction, b.status, b.decided_at, b.decided_by, b.basis
         FROM phrase_bindings b
         LEFT JOIN relation_types r ON r.id = b.relation_type_id
         WHERE b.kb_id = $1
           AND ((b.status = 'bound' AND r.updated_at > b.decided_at)
                OR (b.status IN ('none', 'undecided')
                    AND b.decided_at < (SELECT max(updated_at) FROM relation_types
                                        WHERE kb_id = $1)))
         ORDER BY b.phrase",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 一次判定要写的东西。
pub struct Decision<'a> {
    pub relation_type_id: Option<Uuid>,
    /// forward / reverse
    pub direction: Option<&'a str>,
    /// bound / none / undecided
    pub status: &'a str,
    pub votes: &'a serde_json::Value,
    /// agent / person
    pub decided_by: &'a str,
    /// 判定时输入的指纹（[`basis_of`]）：代理的判定必带，人的判定不带——人不按指纹重判
    pub basis: Option<&'a str>,
}

/// 记下一条签名的判定（有则改）。返回是否写入了。
///
/// **人的判定不被代理覆盖**：已有行是人判的而这次是代理，原样留着、返回 false。
/// 反过来人可以改代理的。
pub async fn decide(
    pool: &PgPool,
    kb_id: Uuid,
    sig: &PhraseSignature,
    d: Decision<'_>,
) -> AppResult<bool> {
    validate_decision(sig, &d)?;
    let mut connection = pool.acquire().await?;
    decide_on(&mut connection, kb_id, sig, d).await
}

fn validate_decision(sig: &PhraseSignature, d: &Decision<'_>) -> AppResult<String> {
    if !matches!(d.status, "bound" | "none" | "undecided") {
        return Err(AppError::Validation(format!(
            "unknown binding status {:?}",
            d.status
        )));
    }
    let bound = d.status == "bound";
    if bound != d.relation_type_id.is_some() || bound != d.direction.is_some() {
        return Err(AppError::Validation(
            "a bound signature needs a property and a direction and an unbound one must not have them".into(),
        ));
    }
    if let Some(dir) = d.direction {
        if !matches!(dir, "forward" | "reverse") {
            return Err(AppError::Validation(format!("unknown direction {dir:?}")));
        }
    }
    if !matches!(d.decided_by, "agent" | "person") {
        return Err(AppError::Validation(format!(
            "unknown decider {:?}",
            d.decided_by
        )));
    }
    let phrase = normalize(&sig.phrase);
    if phrase.is_empty() {
        return Err(AppError::Validation("an empty phrase binds nothing".into()));
    }
    Ok(phrase)
}

/// 人的判定落库时随手排下的重算任务的种类。`main` 按它分发；载荷只有 `kb_id`——
/// job 读的是**当前**的绑定，不回放判定时的属性（0051：回放旧载荷会盖掉后来的人）。
pub const MATERIALIZE_KIND: &str = "materialize_typed";

/// 人的判定与它自己的重算任务**同一事务**提交（0051）。
///
/// 为什么不是「判定落库，然后看有没有 worker 在跑」：正在跑的那次对齐可能已经做完
/// 最后一次读，这条判定就没有任何人替它算类型化行——它被接受了，却永远不投影。
/// 一条判定配一个自己的 job，job 只在判定提交后可见，被谁先处理都读到最新的绑定，
/// 于是最后一次判定总会被算到。代价是 N 次判定 N 次重算，后面的多半是空跑（0051
/// 量过：100 次判定总收敛 593 ms）；「有一个在跑就不排」省下的正是那条会丢的投影。
///
/// 返回 job id；代理不能盖人（`decide_on` 的规则）时什么都没写，返回 `None`。
pub async fn decide_with_delivery(
    pool: &PgPool,
    kb_id: Uuid,
    sig: &PhraseSignature,
    d: Decision<'_>,
) -> AppResult<Option<i64>> {
    decide_with_delivery_budget(pool, kb_id, sig, d, 3).await
}

/// 预算单独成参只为了测「排队失败要连判定一起回滚」：0 会被 `enqueue` 拒掉，
/// 那正是一次发生在判定写入之后的真实失败。生产入口固定给 3。
async fn decide_with_delivery_budget(
    pool: &PgPool,
    kb_id: Uuid,
    sig: &PhraseSignature,
    d: Decision<'_>,
    max_attempts: i32,
) -> AppResult<Option<i64>> {
    let mut tx = pool.begin().await?;
    if !decide_on(&mut tx, kb_id, sig, d).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let id = crate::jobs::enqueue_with_max_attempts_tx(
        &mut tx,
        MATERIALIZE_KIND,
        serde_json::json!({ "kb_id": kb_id }),
        max_attempts,
    )
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// Write on the caller's connection, so related durable work can share its transaction.
pub async fn decide_on(
    connection: &mut sqlx::PgConnection,
    kb_id: Uuid,
    sig: &PhraseSignature,
    d: Decision<'_>,
) -> AppResult<bool> {
    let phrase = validate_decision(sig, &d)?;
    let res = sqlx::query(
        "INSERT INTO phrase_bindings
             (id, kb_id, phrase, subject_type_id, object_type_id, object_is_value,
              relation_type_id, direction, status, votes, statement_count, examples,
              decided_at, decided_by, basis)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), $13, $14)
         ON CONFLICT (kb_id, phrase, subject_type_id, object_type_id, object_is_value) DO UPDATE
            SET relation_type_id = EXCLUDED.relation_type_id,
                direction = EXCLUDED.direction,
                status = EXCLUDED.status,
                votes = EXCLUDED.votes,
                statement_count = EXCLUDED.statement_count,
                examples = EXCLUDED.examples,
                decided_at = now(),
                decided_by = EXCLUDED.decided_by,
                basis = EXCLUDED.basis
          WHERE NOT (phrase_bindings.decided_by = 'person' AND EXCLUDED.decided_by = 'agent')",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(&phrase)
    .bind(sig.subject_type_id)
    .bind(if sig.object_is_value {
        None
    } else {
        sig.object_type_id
    })
    .bind(sig.object_is_value)
    .bind(d.relation_type_id)
    .bind(d.direction)
    .bind(d.status)
    .bind(d.votes)
    .bind(i32::try_from(sig.count).unwrap_or(i32::MAX))
    .bind(&sig.examples)
    .bind(d.decided_by)
    .bind(d.basis)
    .execute(connection)
    .await?;
    Ok(res.rows_affected() > 0)
}

#[cfg(test)]
#[path = "phrase_bindings_delivery_tests.rs"]
mod delivery_tests;

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn a_phrase_is_one_phrase_however_spaced_or_cased() {
        assert_eq!(normalize("  Was   Designed By "), "was designed by");
        assert_eq!(normalize("Revenue"), "revenue");
        assert_eq!(normalize("细化解读"), "细化解读");
        assert_eq!(normalize("   "), "");
    }
}
