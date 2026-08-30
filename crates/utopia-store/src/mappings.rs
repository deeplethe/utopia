//! 语义层的「业务概念 → 数据资产」映射（见 `docs/decisions/0011`）。
//!
//! 它从前是一条 `mapped_to` 事实，宾语是塞在 `object_value` 里的一份 JSON。
//! 搬出来的理由写在迁移 0019 里，一句话：**它不是关于世界的断言，是配置**。
//!
//! 与账本的区别在这里就能看见：这张表**允许原地改**。`confirm` 改的是
//! 「这条配置生效了没有」，不是「我们对世界的认知变了」——所以它不需要
//! append-only，改之前的那一版进 `concept_mapping_revisions` 留痕即可。

use sqlx::PgPool;
use utopia_core::models::ConceptMapping;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 探索任务提议一条映射。
///
/// **同一个 (概念, 源) 只有一条**，由主键管——从前这条唯一性藏在 `object_value`
/// 内部，数据库看不见，只能靠确认流程显式闭合。
///
/// 已经有人表过态的不覆盖：重跑探索会再次算出被拒绝过的那条，不加这一句
/// 它就被刷回待看，等于每跑一次都把人的否决抹掉一次（`ontology_proposals`
/// 那边踩过同一个坑，见 `ontology_proposals`）。
#[allow(clippy::too_many_arguments)]
pub async fn propose(
    pool: &PgPool,
    kb_id: Uuid,
    concept_id: Uuid,
    source: &str,
    table_name: Option<&str>,
    expr: Option<&str>,
    sql: Option<&str>,
    unit: Option<&str>,
    summary: Option<&str>,
    derived: bool,
) -> AppResult<Uuid> {
    // **`DO UPDATE ... WHERE` 不满足时 `RETURNING` 一行都不返回。**
    //
    // 这是 Postgres 的实情而不是直觉：条件挡住更新，那一行就不算被这条语句
    // 动过，于是也不出现在 RETURNING 里。测试当场撞上——第二次提议一条已被
    // 拒绝的映射，`fetch_one` 报「no rows returned」。
    //
    // 所以把 id 单独查出来：更新与否是一回事，「这条映射是哪一行」是另一回事。
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM concept_mappings
          WHERE kb_id = $1 AND concept_id = $2 AND source = $3",
    )
    .bind(kb_id)
    .bind(concept_id)
    .bind(source)
    .fetch_optional(pool)
    .await?;
    let id = existing.map(|(i,)| i).unwrap_or_else(Uuid::now_v7);
    sqlx::query(
        "INSERT INTO concept_mappings
             (id, kb_id, concept_id, source, table_name, expr, sql, unit, summary, derived)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (kb_id, concept_id, source) DO UPDATE
           SET table_name = EXCLUDED.table_name, expr = EXCLUDED.expr,
               sql = EXCLUDED.sql, unit = EXCLUDED.unit,
               summary = EXCLUDED.summary, derived = EXCLUDED.derived,
               updated_at = now()
           WHERE concept_mappings.status = 'proposed'",
    )
    .bind(id)
    .bind(kb_id)
    .bind(concept_id)
    .bind(source)
    .bind(table_name)
    .bind(expr)
    .bind(sql)
    .bind(unit)
    .bind(summary)
    .bind(derived)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 还等着人表态的。Review 页读它。
pub async fn proposed(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<ConceptMapping>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.concept_id, e.canonical_name AS concept_name, m.source,
                m.table_name, m.expr, m.sql, m.unit, m.summary, m.derived, m.status
         FROM concept_mappings m
         JOIN entities e ON e.id = m.concept_id
         WHERE m.kb_id = $1 AND m.status = 'proposed'
         ORDER BY e.canonical_name, m.source
         LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// 人确认过的。问数把它们注进 system prompt——**只用确认过的口径**，
/// 而不是每次从 schema 猜。
pub async fn confirmed(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<ConceptMapping>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.concept_id, e.canonical_name AS concept_name, m.source,
                m.table_name, m.expr, m.sql, m.unit, m.summary, m.derived, m.status
         FROM concept_mappings m
         JOIN entities e ON e.id = m.concept_id
         WHERE m.kb_id = $1 AND m.status = 'confirmed'
         ORDER BY e.canonical_name, m.source
         LIMIT $2",
    )
    .bind(kb_id)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// 有人表态了。
///
/// **改状态不删行**：确认发生过、拒绝也发生过。而拒绝留痕还有当下就用得着的
/// 作用——`propose` 的 `WHERE status = 'proposed'` 据此不把它刷回待看。
pub async fn decide(
    pool: &PgPool,
    kb_id: Uuid,
    mapping_id: Uuid,
    status: &str,
    actor: Uuid,
) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE concept_mappings
            SET status = $3, decided_by = $4, decided_at = now(), updated_at = now()
          WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(mapping_id)
    .bind(status)
    .bind(actor)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 改一条确认过的口径。**改之前那一版先进 revisions**——问数回溯历史报表时
/// 要答得出「上季度这个数是怎么算的」。
///
/// 存整版快照而不是差异：读的时候要的就是「当时是什么」，而差异得从头重放
/// 才能回答这个问题。
#[allow(clippy::too_many_arguments)]
pub async fn revise(
    pool: &PgPool,
    kb_id: Uuid,
    mapping_id: Uuid,
    table_name: Option<&str>,
    expr: Option<&str>,
    sql: Option<&str>,
    unit: Option<&str>,
    summary: Option<&str>,
    derived: bool,
    actor: Uuid,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    let before: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT to_jsonb(m) - 'id' - 'kb_id' FROM concept_mappings m
          WHERE m.id = $2 AND m.kb_id = $1",
    )
    .bind(kb_id)
    .bind(mapping_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((before,)) = before else {
        tx.rollback().await?;
        return Err(AppError::NotFound);
    };
    sqlx::query(
        "INSERT INTO concept_mapping_revisions (id, mapping_id, before, changed_by)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(mapping_id)
    .bind(before)
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE concept_mappings
            SET table_name = $3, expr = $4, sql = $5, unit = $6, summary = $7,
                derived = $8, updated_at = now()
          WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(mapping_id)
    .bind(table_name)
    .bind(expr)
    .bind(sql)
    .bind(unit)
    .bind(summary)
    .bind(derived)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
