//! 类别词绑到类的账本侧（0044 决定 3–4 的第一片，见 0065）。
//!
//! 开放抽取只记文档自己的类别词（`entities.specific_type`），不选类。这里做三件事：
//! 数出一个库里有哪些类别词、各带什么例子与关系短语（签名）；记下每个类别词判成了
//! 什么（绑定）并判哪些过期了；把绑上的类写到该类别词下的实体上（`type_source =
//! 'aligned'`），或者在没有类对得上时按老流程提成「建议加类」。判定本身——问模型、
//! 两票一致才绑——在 server 的 `type_alignment` 里，这里不认识模型。
//!
//! 类别词在 Rust 与 SQL 两侧用同一种归一：空白折成一个空格、去两端、小写。
//! [`normalize`] 与 [`KIND_WORD_SQL`] 必须说同一件事，否则 `signatures` 数出来的词
//! `apply` 找不着。

use chrono::{DateTime, Utc};
use sqlx::{Executor, PgPool, Postgres};
use std::collections::HashMap;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// SQL 侧的归一：与 [`normalize`] 一致。`$col` 由调用处替换成列名。
const KIND_WORD_SQL: &str = "lower(btrim(regexp_replace($col, '\\s+', ' ', 'g')))";

fn kind_word_sql(col: &str) -> String {
    KIND_WORD_SQL.replace("$col", col)
}

/// 归一一个类别词：空白折成一个空格、去两端、小写。"Company " 与 "company" 是一个词。
pub fn normalize(kind_word: &str) -> String {
    kind_word
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// 一个类别词的签名：判它该绑到哪个类时给模型看的全部。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct KindWordSignature {
    /// 归一过的词
    pub kind_word: String,
    /// 文档里见过的写法（最多 3 个，按出现次数）
    pub words: Vec<String>,
    /// 这个词下活着的实体数
    pub count: i64,
    /// 例名（最多 3 个，有名字的在前，再按创建先后）
    pub examples: Vec<String>,
    /// 以这些实体为主语的开放陈述里最常见的关系短语（最多 3 个）
    pub phrases: Vec<String>,
}

/// 库里每个 distinct 的类别词：活着的（`merged_into` 空）、带类别词的实体。
pub async fn signatures(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<KindWordSignature>> {
    let sql = format!(
        "WITH live AS (
             SELECT e.id, e.canonical_name, e.description, e.created_at,
                    btrim(regexp_replace(e.specific_type, '\\s+', ' ', 'g')) AS spelling,
                    {kind} AS kind_word
             FROM entities e
             WHERE e.kb_id = $1 AND e.merged_into IS NULL
               AND e.specific_type IS NOT NULL AND btrim(e.specific_type) <> ''
         ),
         grouped AS (
             SELECT kind_word, count(*) AS count FROM live GROUP BY kind_word
         )
         SELECT g.kind_word,
                g.count,
                ARRAY(SELECT s.spelling FROM (
                          SELECT l.spelling, count(*) AS n FROM live l
                          WHERE l.kind_word = g.kind_word
                          GROUP BY l.spelling ORDER BY n DESC, l.spelling LIMIT 3) s
                ) AS words,
                ARRAY(SELECT l.canonical_name FROM live l
                      WHERE l.kind_word = g.kind_word
                      ORDER BY (l.description IS NOT NULL), l.created_at, l.id LIMIT 3
                ) AS examples,
                ARRAY(SELECT s.phrase FROM (
                          SELECT f.phrase, count(*) AS n
                          FROM facts f JOIN live l ON l.id = f.subject_id
                          WHERE f.kb_id = $1 AND f.layer = 'open' AND f.invalidated_at IS NULL
                            AND f.phrase IS NOT NULL AND l.kind_word = g.kind_word
                          GROUP BY f.phrase ORDER BY n DESC, f.phrase LIMIT 3) s
                ) AS phrases
         FROM grouped g
         ORDER BY g.count DESC, g.kind_word",
        kind = kind_word_sql("e.specific_type")
    );
    Ok(sqlx::query_as(&sql).bind(kb_id).fetch_all(pool).await?)
}

/// 一个类别词判成了什么。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Binding {
    pub kind_word: String,
    /// `status = 'bound'` 时有值
    pub type_id: Option<Uuid>,
    /// bound / none / undecided
    pub status: String,
    pub decided_at: DateTime<Utc>,
    /// agent / person
    pub decided_by: String,
}

/// 库里全部绑定，按词序。
pub async fn bindings(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<Binding>> {
    Ok(sqlx::query_as(
        "SELECT kind_word, type_id, status, decided_at, decided_by
         FROM type_bindings WHERE kb_id = $1 ORDER BY kind_word",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 不再成立的绑定：绑到的类在判定之后改过；或判成 none / undecided 之后库里有类
/// 新建或修改。负向判定没有选中的类，已有类的新定义也可能让它对得上。
/// 绑到的类被删了的，行已随级联消失，这里不会出现。
pub async fn stale(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT b.kind_word
         FROM type_bindings b
         LEFT JOIN entity_types t ON t.id = b.type_id
         WHERE b.kb_id = $1
           AND ((b.status = 'bound' AND t.updated_at > b.decided_at)
                OR (b.status IN ('none', 'undecided')
                    AND b.decided_at < (SELECT max(updated_at) FROM entity_types
                                        WHERE kb_id = $1)))
         ORDER BY b.kind_word",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 记下一个类别词的判定（有则改）。返回是否写入了。
///
/// **人的判定不被代理覆盖**：已有行是人判的而这次是代理，原样留着、返回 false。
/// 反过来人可以改代理的。`words` 传空时保留已有的写法——人在界面上拍板时手里
/// 未必有签名。
#[allow(clippy::too_many_arguments)]
pub async fn decide<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
    kind_word: &str,
    words: &[String],
    type_id: Option<Uuid>,
    status: &str,
    votes: &serde_json::Value,
    decided_by: &str,
) -> AppResult<bool> {
    if !matches!(status, "bound" | "none" | "undecided") {
        return Err(AppError::Validation(format!(
            "unknown binding status {status:?}"
        )));
    }
    if (status == "bound") != type_id.is_some() {
        return Err(AppError::Validation(
            "a bound kind word needs a class and an unbound one must not have one".into(),
        ));
    }
    if !matches!(decided_by, "agent" | "person") {
        return Err(AppError::Validation(format!(
            "unknown decider {decided_by:?}"
        )));
    }
    let kind_word = normalize(kind_word);
    if kind_word.is_empty() {
        return Err(AppError::Validation(
            "an empty kind word binds nothing".into(),
        ));
    }
    let res = sqlx::query(
        "INSERT INTO type_bindings
             (id, kb_id, kind_word, words, type_id, status, votes, decided_at, decided_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8)
         ON CONFLICT (kb_id, kind_word) DO UPDATE
            SET words = CASE WHEN cardinality(EXCLUDED.words) = 0
                             THEN type_bindings.words ELSE EXCLUDED.words END,
                type_id = EXCLUDED.type_id,
                status = EXCLUDED.status,
                votes = EXCLUDED.votes,
                decided_at = now(),
                decided_by = EXCLUDED.decided_by
          WHERE NOT (type_bindings.decided_by = 'person' AND EXCLUDED.decided_by = 'agent')",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(&kind_word)
    .bind(words)
    .bind(type_id)
    .bind(status)
    .bind(votes)
    .bind(decided_by)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Store a decision and its entity projection in one transaction.
/// The binding row stays locked until the projection is written, so an older
/// agent cannot apply its class after a person's newer decision has committed.
/// A rejected agent decision changes neither the binding nor the entities.
#[allow(clippy::too_many_arguments)]
pub async fn decide_and_apply(
    pool: &PgPool,
    kb_id: Uuid,
    kind_word: &str,
    words: &[String],
    type_id: Option<Uuid>,
    status: &str,
    votes: &serde_json::Value,
    decided_by: &str,
) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    let written = write_decision_and_projection(
        &mut tx, kb_id, kind_word, words, type_id, status, votes, decided_by,
    )
    .await?;
    tx.commit().await?;
    Ok(written)
}

/// The review request may wait briefly for a concurrent writer, but must not
/// pin a connection indefinitely. This is per lock acquisition, not a request
/// deadline, and does not change the background aligner's waiting policy.
pub async fn decide_and_apply_human(
    pool: &PgPool,
    kb_id: Uuid,
    kind_word: &str,
    type_id: Option<Uuid>,
    votes: &serde_json::Value,
) -> AppResult<bool> {
    let mut tx = pool.begin().await?;
    let result = async {
        sqlx::query("SET LOCAL lock_timeout = '2s'")
            .execute(&mut *tx)
            .await?;
        write_decision_and_projection(
            &mut tx,
            kb_id,
            kind_word,
            &[],
            type_id,
            if type_id.is_some() { "bound" } else { "none" },
            votes,
            "person",
        )
        .await
    }
    .await;
    match result {
        Ok(written) => {
            tx.commit().await?;
            Ok(written)
        }
        Err(error) => {
            // Finish rollback before returning a retryable response or reusing
            // the connection. Preserve both errors if cleanup itself fails.
            if let Err(rollback) = tx.rollback().await {
                return Err(AppError::Other(anyhow::Error::new(error).context(format!(
                    "rolling back human kind-word decision: {rollback}"
                ))));
            }
            if matches!(&error, AppError::Db(sqlx::Error::Database(e))
                if e.code().as_deref() == Some("55P03"))
            {
                return Err(AppError::CodedConflict {
                    code: "alignment_busy",
                    message: "This kind word is being updated by another operation. Please try again shortly."
                        .into(),
                });
            }
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn write_decision_and_projection(
    connection: &mut sqlx::PgConnection,
    kb_id: Uuid,
    kind_word: &str,
    words: &[String],
    type_id: Option<Uuid>,
    status: &str,
    votes: &serde_json::Value,
    decided_by: &str,
) -> AppResult<bool> {
    let written = decide(
        &mut *connection,
        kb_id,
        kind_word,
        words,
        type_id,
        status,
        votes,
        decided_by,
    )
    .await?;
    if written {
        match type_id {
            Some(id) => {
                apply(&mut *connection, kb_id, kind_word, id).await?;
            }
            None => {
                unapply(&mut *connection, kb_id, kind_word).await?;
            }
        }
    }
    Ok(written)
}

/// 把绑上的类写到这个类别词下每个活着的、人没定过类的实体上。返回改动数。
///
/// 已在这个类上的不算改动（`IS DISTINCT FROM`：`type_id` 可能是 NULL）。有了类，
/// 「建议加类」就不再是建议，`proposed_type` 一并清掉——否则本体页会继续为一个
/// 已经有类的词喊着要建类。
pub async fn apply<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
    kind_word: &str,
    type_id: Uuid,
) -> AppResult<u64> {
    let sql = format!(
        "UPDATE entities
            SET type_id = $3, type_source = 'aligned', proposed_type = NULL, updated_at = now()
          WHERE kb_id = $1 AND merged_into IS NULL AND type_source <> 'human'
            AND specific_type IS NOT NULL AND {kind} = $2
            AND type_id IS DISTINCT FROM $3",
        kind = kind_word_sql("specific_type")
    );
    let res = sqlx::query(&sql)
        .bind(kb_id)
        .bind(normalize(kind_word))
        .bind(type_id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// 绑定变成 none 或失效时：按它对齐上去的实体失去那个类。只动 `aligned` 的行——
/// 抽取判的、引擎猜的、人拍的都不是这条绑定给的。返回改动数。
pub async fn unapply<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
    kind_word: &str,
) -> AppResult<u64> {
    let sql = format!(
        "UPDATE entities
            SET type_id = NULL, type_source = 'extracted', updated_at = now()
          WHERE kb_id = $1 AND merged_into IS NULL AND type_source = 'aligned'
            AND specific_type IS NOT NULL AND {kind} = $2",
        kind = kind_word_sql("specific_type")
    );
    let res = sqlx::query(&sql)
        .bind(kb_id)
        .bind(normalize(kind_word))
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// 抽取时用的绑定表：归一过的类别词 → 类 id，只有绑上的。
pub async fn bound_map(pool: &PgPool, kb_id: Uuid) -> AppResult<HashMap<String, Uuid>> {
    let rows: Vec<(String, Uuid)> = sqlx::query_as(
        "SELECT kind_word, type_id FROM type_bindings
         WHERE kb_id = $1 AND status = 'bound' AND type_id IS NOT NULL",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// 没有类对得上的类别词：按老流程提到本体页（`proposed_type` → `adopt_proposed_types`）。
///
/// 守 `set_proposed_type` 的约：只写第一次、最长 60 字；只提活着的、还没类的、人没
/// 定过的实体——采纳时人定过的不会被认领，数进「将重新归类 N 个」里就是虚的。
/// 返回写上的行数。
pub async fn propose(
    pool: &PgPool,
    kb_id: Uuid,
    kind_word: &str,
    spelling: &str,
) -> AppResult<u64> {
    let spelling = spelling.trim();
    if spelling.is_empty() {
        return Err(AppError::Validation(
            "a proposal needs the document's spelling".into(),
        ));
    }
    let sql = format!(
        "UPDATE entities SET proposed_type = left($3, 60)
          WHERE kb_id = $1 AND merged_into IS NULL AND type_id IS NULL
            AND type_source <> 'human' AND proposed_type IS NULL
            AND specific_type IS NOT NULL AND {kind} = $2",
        kind = kind_word_sql("specific_type")
    );
    let res = sqlx::query(&sql)
        .bind(kb_id)
        .bind(normalize(kind_word))
        .bind(spelling)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn a_kind_word_is_one_word_however_spaced_or_cased() {
        assert_eq!(
            normalize("  Stockholder   Proposal "),
            "stockholder proposal"
        );
        assert_eq!(normalize("Company"), "company");
        assert_eq!(normalize("指标"), "指标");
        assert_eq!(normalize("   "), "");
    }
}
