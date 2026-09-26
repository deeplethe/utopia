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
//!
//! 代理的判定记下它看到的输入（`basis`，0053 的类别词那一半，#795）：给模型看的每个
//! 候选类的 id、`updated_at` 与祖先闭包（[`ClassSnapshot::basis`]）。过期 = 按现在的类
//! 重算出来的指纹对不上，不再比时刻——判定写在模型答完之后，答题期间改的定义时间戳
//! 看不见，父边的增删也不碰 `updated_at`。写判定时在同一事务里再算一遍，对不上的回复
//! 不收（[`decide_and_apply_if_current`]）。

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
pub async fn signatures<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
) -> AppResult<Vec<KindWordSignature>> {
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
    /// 代理判定时输入的指纹（[`ClassSnapshot::basis`]）。人的判定不带——人不按指纹重判；
    /// 这一列出现之前的代理判定也为空，各重判一次
    pub basis: Option<String>,
}

/// 库里全部绑定，按词序。
pub async fn bindings<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
) -> AppResult<Vec<Binding>> {
    Ok(sqlx::query_as(
        "SELECT kind_word, type_id, status, decided_at, decided_by, basis
         FROM type_bindings WHERE kb_id = $1 ORDER BY kind_word",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 一个库此刻的类：每个类的 `updated_at` 与直接父类。判定的指纹从它算——开跑时在调模型
/// 之前的快照事务里读一份，写判定时在写的事务里再读一份，两份算出来不一样就是答题
/// 期间候选类变了
#[derive(Debug, Clone, Default)]
pub struct ClassSnapshot {
    versions: HashMap<Uuid, DateTime<Utc>>,
    parents: HashMap<Uuid, Vec<Uuid>>,
}

/// 读一个库的 [`ClassSnapshot`]。
pub async fn class_snapshot<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
) -> AppResult<ClassSnapshot> {
    let rows: Vec<(Uuid, DateTime<Utc>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT t.id, t.updated_at,
                ARRAY(SELECT p.parent_id FROM entity_type_parents p WHERE p.child_id = t.id)
         FROM entity_types t WHERE t.kb_id = $1",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    let mut snapshot = ClassSnapshot::default();
    for (id, at, parents) in rows {
        snapshot.versions.insert(id, at);
        snapshot.parents.insert(id, parents);
    }
    Ok(snapshot)
}

impl ClassSnapshot {
    /// 一个类别词判定的指纹：给模型看的每个候选类的 id、`updated_at` 与祖先闭包。
    /// 候选的先后不算（第二票本来就倒着给）；候选已经不在了记成 gone，同样算变了。
    /// 父边的增删不碰 `updated_at`，所以闭包要单独算进来。
    pub fn basis(&self, candidates: &[Uuid]) -> String {
        let mut parts: Vec<String> = candidates
            .iter()
            .map(|id| match self.versions.get(id) {
                Some(at) => {
                    let up: Vec<String> = self.ancestors(*id).iter().map(Uuid::to_string).collect();
                    format!("{id}@{}^{}", at.to_rfc3339(), up.join(","))
                }
                None => format!("{id}@gone"),
            })
            .collect();
        parts.sort();
        parts.dedup();
        fingerprint(&parts.join(";"))
    }

    /// 一个类的全部祖先（不含自己），排好序。多继承与菱形按并集走，同一个祖先只进一次；
    /// 编辑器不允许环，见过就不再走，万一有环也走得完
    fn ancestors(&self, id: Uuid) -> Vec<Uuid> {
        let mut seen: Vec<Uuid> = Vec::new();
        let mut stack: Vec<Uuid> = self.parents.get(&id).cloned().unwrap_or_default();
        while let Some(p) = stack.pop() {
            if p == id || seen.contains(&p) {
                continue;
            }
            seen.push(p);
            if let Some(up) = self.parents.get(&p) {
                stack.extend_from_slice(up);
            }
        }
        seen.sort();
        seen
    }
}

/// 与 `phrase_bindings::basis_of` 同一个哈希：FNV-1a 64 位。只是缓存失效的键，不是安全
/// 用途，不值得为它拉一个哈希依赖；也不去动那边——那边的值一变，全部短语判定都得重判
fn fingerprint(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 不再成立的绑定：绑到的类在判定之后改过；或判成 none / undecided 之后库里有类
/// 新建或修改。负向判定没有选中的类，已有类的新定义也可能让它对得上。
/// 绑到的类被删了的，行已随级联消失，这里不会出现。
///
/// 对齐的 worker 已不读它：时间戳看不见模型答题期间的编辑（#795），过期改按指纹判
/// （[`ClassSnapshot::basis`]）。留着给只要粗信号的调用方。
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
/// 未必有签名。这里写下的判定不带指纹：代理的判定走 [`decide_and_apply_if_current`]。
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
    decide_with_basis(
        pool, kb_id, kind_word, words, type_id, status, votes, decided_by, None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn decide_with_basis<'e>(
    pool: impl Executor<'e, Database = Postgres>,
    kb_id: Uuid,
    kind_word: &str,
    words: &[String],
    type_id: Option<Uuid>,
    status: &str,
    votes: &serde_json::Value,
    decided_by: &str,
    basis: Option<&str>,
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
             (id, kb_id, kind_word, words, type_id, status, votes, decided_at, decided_by, basis)
         VALUES ($1, $2, $3, $4, $5, $6, $7, now(), $8, $9)
         ON CONFLICT (kb_id, kind_word) DO UPDATE
            SET words = CASE WHEN cardinality(EXCLUDED.words) = 0
                             THEN type_bindings.words ELSE EXCLUDED.words END,
                type_id = EXCLUDED.type_id,
                status = EXCLUDED.status,
                votes = EXCLUDED.votes,
                decided_at = now(),
                decided_by = EXCLUDED.decided_by,
                basis = EXCLUDED.basis
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
    .bind(basis)
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
        &mut tx, kb_id, kind_word, words, type_id, status, votes, decided_by, None,
    )
    .await?;
    tx.commit().await?;
    Ok(written)
}

/// What became of an agent decision offered to [`decide_and_apply_if_current`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// The decision and its projection are written, with the basis.
    Written,
    /// A person decided this kind word; the agent's answer does not replace it.
    KeptPerson,
    /// The candidate classes changed while the model was answering. The answer
    /// is about inputs that no longer exist, so nothing is written.
    Moved,
}

/// Accept an agent decision only for the inputs it actually read (#795).
///
/// `basis` is what the run computed from its snapshot before calling the model.
/// The candidate classes are locked `FOR SHARE` before the binding row is written:
/// a class delete takes the class row first and then cascades to the binding, so
/// taking them in the same order cannot deadlock with it. The basis is then
/// recomputed from the rows as they are now; a mismatch means the model answered
/// about a definition or hierarchy that has since changed, and nothing is written.
///
/// A parent edge or a new class committed after this check is not blocked. The
/// stored basis then no longer matches the current inputs, so the next run finds
/// the decision stale and asks again.
#[allow(clippy::too_many_arguments)]
pub async fn decide_and_apply_if_current(
    pool: &PgPool,
    kb_id: Uuid,
    kind_word: &str,
    words: &[String],
    type_id: Option<Uuid>,
    status: &str,
    votes: &serde_json::Value,
    basis: &str,
    candidates: &[Uuid],
) -> AppResult<Acceptance> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT id FROM entity_types WHERE kb_id = $1 AND id = ANY($2) ORDER BY id FOR SHARE",
    )
    .bind(kb_id)
    .bind(candidates)
    .fetch_all(&mut *tx)
    .await?;
    if class_snapshot(&mut *tx, kb_id).await?.basis(candidates) != basis {
        tx.rollback().await?;
        return Ok(Acceptance::Moved);
    }
    let written = write_decision_and_projection(
        &mut tx,
        kb_id,
        kind_word,
        words,
        type_id,
        status,
        votes,
        "agent",
        Some(basis),
    )
    .await?;
    tx.commit().await?;
    Ok(if written {
        Acceptance::Written
    } else {
        Acceptance::KeptPerson
    })
}

/// The review request may wait briefly for a concurrent writer, but must not
/// pin a connection indefinitely. This is per lock acquisition, not a request
/// deadline, and does not change the background aligner's waiting policy.
///
/// The decision, its projection and the phrase alignment it makes necessary
/// commit together: a changed class moves the phrase signatures of every entity
/// under this kind word, and a job queued after the commit could be lost with
/// the process in between.
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
        let written = write_decision_and_projection(
            &mut tx,
            kb_id,
            kind_word,
            &[],
            type_id,
            if type_id.is_some() { "bound" } else { "none" },
            votes,
            "person",
            None,
        )
        .await?;
        if written {
            crate::jobs::enqueue_unless_queued_tx(
                &mut tx,
                "align_phrases",
                serde_json::json!({ "kb_id": kb_id }),
            )
            .await?;
        }
        Ok::<bool, AppError>(written)
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
    basis: Option<&str>,
) -> AppResult<bool> {
    let written = decide_with_basis(
        &mut *connection,
        kb_id,
        kind_word,
        words,
        type_id,
        status,
        votes,
        decided_by,
        basis,
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
    use super::{normalize, ClassSnapshot};
    use chrono::{DateTime, Utc};
    use uuid::Uuid;

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

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().expect("timestamp")
    }

    /// organization ⊂ legal_entity；agent 暂时没有父类
    fn snapshot() -> (ClassSnapshot, [Uuid; 3]) {
        let ids = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        let [org, legal, _] = ids;
        let mut s = ClassSnapshot::default();
        for id in ids {
            s.versions.insert(id, at("2026-09-26T00:00:00Z"));
            s.parents.insert(id, Vec::new());
        }
        s.parents.insert(org, vec![legal]);
        (s, ids)
    }

    #[test]
    fn a_basis_names_what_was_shown_not_the_order_it_was_shown_in() {
        let (s, [org, legal, _]) = snapshot();
        assert_eq!(s.basis(&[org, legal]), s.basis(&[legal, org]));
        assert_eq!(s.basis(&[org, legal]), s.basis(&[org, legal, org]));
        assert_ne!(s.basis(&[org, legal]), s.basis(&[org]));
    }

    #[test]
    fn an_edit_a_grandparent_edge_or_a_deleted_candidate_changes_the_basis() {
        let (s, [org, legal, agent]) = snapshot();
        let before = s.basis(&[org]);

        let mut edited = s.clone();
        edited.versions.insert(org, at("2026-09-26T00:00:01Z"));
        assert_ne!(before, edited.basis(&[org]), "an edit moves updated_at");

        // 父边的增删不碰 updated_at：org 的祖父变了，org 的指纹也得变
        let mut rooted = s.clone();
        rooted.parents.insert(legal, vec![agent]);
        assert_ne!(
            before,
            rooted.basis(&[org]),
            "a grandparent is in the closure"
        );

        let mut gone = s.clone();
        gone.versions.remove(&org);
        gone.parents.remove(&org);
        assert_ne!(
            before,
            gone.basis(&[org]),
            "a deleted candidate is a change"
        );
    }

    #[test]
    fn a_diamond_or_a_cycle_still_gives_one_closure() {
        let (mut s, [org, legal, agent]) = snapshot();
        // 菱形：org → legal、org → agent，legal → agent
        s.parents.insert(org, vec![legal, agent]);
        s.parents.insert(legal, vec![agent]);
        assert_eq!(s.ancestors(org), {
            let mut v = vec![legal, agent];
            v.sort();
            v
        });
        // 编辑器不许环；万一有，走得完且不把自己算进祖先
        s.parents.insert(agent, vec![org]);
        assert!(!s.ancestors(org).contains(&org));
        let _ = s.basis(&[org, legal, agent]);
    }
}
