//! 语义层的「业务概念 → 数据资产」映射（见 `docs/decisions/0011`）。
//!
//! 它从前是一条 `mapped_to` 事实，宾语是塞在 `object_value` 里的一份 JSON。
//! 搬出来的理由写在 `concept_mappings` 的建表注释里，一句话：**它不是关于世界的断言，是配置**。
//!
//! 与账本的区别在这里就能看见：这张表**允许原地改**。`confirm` 改的是
//! 「这条配置生效了没有」，不是「我们对世界的认知变了」——所以它不需要
//! append-only，改之前的那一版进 `concept_mapping_revisions` 留痕即可。

use sqlx::PgPool;
use utopia_core::models::{ConceptMapping, MappingRevision};
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
) -> AppResult<(Uuid, bool)> {
    propose_with_evidence(
        pool, kb_id, concept_id, source, table_name, expr, sql, unit, summary, derived, None, None,
    )
    .await
}

/// Re-evaluate the opt-in on every exploration write. An automatic definition can improve;
/// a human decision (even an edit of a pending proposal) cannot be replaced by exploration.
#[allow(clippy::too_many_arguments)]
pub async fn propose_with_evidence(
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
    confidence: Option<f64>,
    rationale: Option<&str>,
) -> AppResult<(Uuid, bool)> {
    let confidence = confidence.filter(|c| c.is_finite() && (0.0..=1.0).contains(c));
    let rationale = rationale.map(str::trim).filter(|s| !s.is_empty());
    let mut tx = pool.begin().await?;
    // A switch changed during a long model call must apply to its eventual write. SHARE
    // also orders this write against a concurrent opt-out without serializing explorations.
    let enabled: bool = sqlx::query_scalar(
        "SELECT auto_confirm_mappings FROM knowledge_bases WHERE id = $1 FOR SHARE",
    )
    .bind(kb_id)
    .fetch_one(&mut *tx)
    .await?;
    let automatic = enabled
        && confidence.is_some_and(|c| c >= 0.8)
        && rationale.is_some()
        && [expr, sql]
            .into_iter()
            .flatten()
            .any(|s| !s.trim().is_empty());
    let status = if automatic { "confirmed" } else { "proposed" };
    let written: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO concept_mappings
             (id, kb_id, concept_id, source, table_name, expr, sql, unit, summary, derived,
              confidence, rationale, status, decided_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                 CASE WHEN $13 = 'confirmed' THEN now() END)
         ON CONFLICT (kb_id, concept_id, source) DO UPDATE
           SET table_name = EXCLUDED.table_name, expr = EXCLUDED.expr,
               sql = EXCLUDED.sql, unit = EXCLUDED.unit,
               summary = EXCLUDED.summary, derived = EXCLUDED.derived,
               confidence = EXCLUDED.confidence, rationale = EXCLUDED.rationale,
               status = EXCLUDED.status, decided_at = EXCLUDED.decided_at, updated_at = now()
           WHERE concept_mappings.decided_by IS NULL
             AND concept_mappings.status IN ('proposed', 'confirmed')
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(concept_id)
    .bind(source)
    .bind(table_name)
    .bind(expr)
    .bind(sql)
    .bind(unit)
    .bind(summary)
    .bind(derived)
    .bind(confidence)
    .bind(rationale)
    .bind(status)
    .fetch_optional(&mut *tx)
    .await?;
    // RETURNING supplies the actual row id after concurrent inserts. A protected row
    // returns nothing, so read its id separately without reporting a successful write.
    let id = match written {
        Some(id) => id,
        None => sqlx::query_scalar(
            "SELECT id FROM concept_mappings WHERE kb_id = $1 AND concept_id = $2 AND source = $3",
        )
        .bind(kb_id)
        .bind(concept_id)
        .bind(source)
        .fetch_one(&mut *tx)
        .await?,
    };
    if written.is_some() && automatic {
        // Automation changes the effective definition: losing this event must roll back
        // that change, rather than leave an effective definition nobody can account for.
        sqlx::query(
            "INSERT INTO audit_events (id, kb_id, actor_id, action, target_kind, target_id, detail)
             VALUES ($1, $2, NULL, 'mapping.auto_confirmed', 'mapping', $3, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(id)
        .bind(serde_json::json!({
            "concept_id": concept_id, "source": source, "confidence": confidence,
            "rationale": rationale, "threshold": 0.8,
            "definition": { "table": table_name, "expr": expr, "sql": sql,
                "unit": unit, "summary": summary, "derived": derived }
        }))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok((id, written.is_some()))
}

/// 人从零写一条口径（#562）。**落下来就是确认的**：写的人就是表态的人，
/// 不需要再过一遍审。
///
/// 从前这张表只有探索一条来路。一个数据团队手上有自己的指标口径文档，却没有
/// 地方把它填进去——而口径进了问数的提示词，宽表语料从 1/18 到 17/18（#520）。
///
/// 同一个 (概念, 源) 已经有一条时报冲突，而不是悄悄覆盖：那一条可能是探索提的、
/// 人已经确认过的，覆盖等于抹掉一次表态。要改用 `revise`。
/// 探索反过来也盖不掉这条：`propose` 只刷新没有人工决定的行。
#[allow(clippy::too_many_arguments)]
pub async fn create(
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
    actor: Uuid,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO concept_mappings
             (id, kb_id, concept_id, source, table_name, expr, sql, unit, summary, derived,
              status, decided_by, decided_at, written_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'confirmed', $11, now(), $11)",
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
    .bind(actor)
    .execute(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => AppError::Conflict(
            "A definition for this concept on this source already exists; revise it instead".into(),
        ),
        _ => AppError::Db(e),
    })?;
    Ok(id)
}

/// 还等着人表态的。Review 页读它。
pub async fn proposed(
    pool: &PgPool,
    kb_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<ConceptMapping>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.concept_id, e.canonical_name AS concept_name, m.source,
                m.table_name, m.expr, m.sql, m.unit, m.summary, m.derived, m.status, m.written_by,
                m.decided_by, m.confidence, m.rationale
         FROM concept_mappings m
         JOIN entities e ON e.id = m.concept_id
         WHERE m.kb_id = $1 AND m.status = 'proposed'
         ORDER BY e.canonical_name, m.source, m.id
         LIMIT $2 OFFSET $3",
    )
    .bind(kb_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// 已生效的（人工或自动确认）。问数把它们注进 system prompt——**只用确认过的口径**，
/// 而不是每次从 schema 猜。
pub async fn confirmed(pool: &PgPool, kb_id: Uuid, limit: i64) -> AppResult<Vec<ConceptMapping>> {
    Ok(sqlx::query_as(
        "SELECT m.id, m.concept_id, e.canonical_name AS concept_name, m.source,
                m.table_name, m.expr, m.sql, m.unit, m.summary, m.derived, m.status, m.written_by,
                m.decided_by, m.confidence, m.rationale
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
          WHERE m.id = $2 AND m.kb_id = $1 FOR UPDATE",
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
                derived = $8, decided_by = $9, decided_at = now(),
                confidence = NULL, rationale = NULL, updated_at = now()
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
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 数据映射页读的：一页口径，可按状态与关键词筛。
///
/// **`proposed` 与 `confirmed` 那两条查询都不够用**——前者只捞待表态的，
/// 后者是给问数拼 prompt 的（无分页、无筛选、限 30 条）。人要看的是全部，
/// 包括被自己拒绝过的那些：拒绝留了痕，就该看得见，否则「为什么这个概念
/// 没被映射」永远答不上来。
pub async fn page(
    pool: &PgPool,
    kb_id: Uuid,
    status: Option<&str>,
    q: Option<&str>,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<ConceptMapping>, i64)> {
    // 概念名与数据源名都可搜：人记得住的是「GMV」，也可能是「那个接了 orders 的」
    const WHERE: &str = "WHERE m.kb_id = $1
           AND ($2::text IS NULL OR m.status = $2)
           AND ($3::text IS NULL
                OR e.canonical_name ILIKE '%' || $3 || '%'
                OR m.source ILIKE '%' || $3 || '%'
                OR m.table_name ILIKE '%' || $3 || '%')";
    let rows: Vec<ConceptMapping> = sqlx::query_as(&format!(
        "SELECT m.id, m.concept_id, e.canonical_name AS concept_name, m.source,
                m.table_name, m.expr, m.sql, m.unit, m.summary, m.derived, m.status, m.written_by,
                m.decided_by, m.confidence, m.rationale
         FROM concept_mappings m
         JOIN entities e ON e.id = m.concept_id
         {WHERE}
         ORDER BY e.canonical_name, m.source, m.id
         LIMIT $4 OFFSET $5"
    ))
    .bind(kb_id)
    .bind(status)
    .bind(q)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) = sqlx::query_as(&format!(
        "SELECT count(*) FROM concept_mappings m
         JOIN entities e ON e.id = m.concept_id {WHERE}"
    ))
    .bind(kb_id)
    .bind(status)
    .bind(q)
    .fetch_one(pool)
    .await?;
    Ok((rows, total))
}

/// 每种状态各多少。页面的筛选条要显示计数，分三次查是三次全表扫。
pub async fn status_counts(pool: &PgPool, kb_id: Uuid) -> AppResult<(i64, i64, i64)> {
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'proposed'),
                count(*) FILTER (WHERE status = 'confirmed'),
                count(*) FILTER (WHERE status = 'rejected')
           FROM concept_mappings WHERE kb_id = $1",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// 一条口径给检索用的两段文字：嵌入的一段（名字 + 说明 + 单位——问题跟这些对得上），
/// 词面匹配的一段（再加表名与表达式——问题里偶尔直接说列名）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MappingText {
    pub id: Uuid,
    pub concept_name: String,
    pub summary: Option<String>,
    pub unit: Option<String>,
    pub table_name: Option<String>,
    pub expr: Option<String>,
}

impl MappingText {
    pub fn embed_text(&self) -> String {
        let mut s = self.concept_name.clone();
        if let Some(x) = self.summary.as_deref().filter(|x| !x.trim().is_empty()) {
            s.push_str(": ");
            s.push_str(x.trim());
        }
        if let Some(u) = self.unit.as_deref().filter(|u| !u.trim().is_empty()) {
            s.push_str(" (");
            s.push_str(u.trim());
            s.push(')');
        }
        s
    }
    pub fn lexical_text(&self) -> String {
        [
            Some(self.concept_name.as_str()),
            self.summary.as_deref(),
            self.table_name.as_deref(),
            self.expr.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
    }
}

/// 确认口径里还没嵌、或嵌的时候文本 / 模型跟现在不一样的（#574），最多 `limit` 条。
/// 与 `ontology::types_needing_embedding` 同一条规则：改了文本或换了模型就重嵌。
/// 有上限是因为补嵌跑在问数的请求路径上：换了嵌入模型之后第一问不该把几百条
/// 一口气嵌完才开口，剩下的下一问接着补
pub async fn needing_embedding(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
    limit: i64,
) -> AppResult<Vec<MappingText>> {
    let rows: Vec<MappingText> = sqlx::query_as(
        "SELECT m.id, e.canonical_name AS concept_name, m.summary, m.unit, m.table_name, m.expr
           FROM concept_mappings m
           JOIN entities e ON e.id = m.concept_id
          WHERE m.kb_id = $1 AND m.status = 'confirmed'
            AND (m.embedding IS NULL OR m.embedded_model IS DISTINCT FROM $2)
          ORDER BY m.id
          LIMIT $3",
    )
    .bind(kb_id)
    .bind(model)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    // 文本变了的也要重嵌。SQL 里拼这段文字要把 `embed_text` 抄一遍，
    // 两处迟早不一样，所以拉回来在 Rust 里比
    let stale_text: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT m.id FROM concept_mappings m
          WHERE m.kb_id = $1 AND m.status = 'confirmed'
            AND m.embedding IS NOT NULL AND m.embedded_model = $2",
    )
    .bind(kb_id)
    .bind(model)
    .fetch_all(pool)
    .await?;
    let mut out = rows;
    if !stale_text.is_empty() {
        let ids: Vec<Uuid> = stale_text.into_iter().map(|(id,)| id).collect();
        let current: Vec<MappingText> = sqlx::query_as(
            "SELECT m.id, e.canonical_name AS concept_name, m.summary, m.unit, m.table_name, m.expr
               FROM concept_mappings m JOIN entities e ON e.id = m.concept_id
              WHERE m.id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(pool)
        .await?;
        let embedded: std::collections::HashMap<Uuid, Option<String>> =
            sqlx::query_as::<_, (Uuid, Option<String>)>(
                "SELECT id, embedded_text FROM concept_mappings WHERE id = ANY($1)",
            )
            .bind(&ids)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
        for t in current {
            let was = embedded.get(&t.id).cloned().flatten();
            if was.as_deref() != Some(t.embed_text().as_str()) {
                out.push(t);
            }
        }
    }
    out.truncate(limit.max(0) as usize);
    Ok(out)
}

/// 写回向量，连同嵌的是哪段文字、哪个模型。
pub async fn set_embeddings(
    pool: &PgPool,
    model: &str,
    items: &[(MappingText, Vec<f32>)],
) -> AppResult<()> {
    for (t, v) in items {
        sqlx::query(
            "UPDATE concept_mappings
                SET embedding = $2, embedded_model = $3, embedded_text = $4
              WHERE id = $1",
        )
        .bind(t.id)
        .bind(pgvector::Vector::from(v.clone()))
        .bind(model)
        .bind(t.embed_text())
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 向量一路：离问题最近的确认口径。只比同一个模型嵌出来的行——换过模型、还没重嵌的
/// 不参与：维度碰巧相同时两个模型的空间也对不上，比出来的近远是假的。维度不同的
/// 再多挡一道，免得 pgvector 直接报错
pub async fn vector_search(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
    embedding: &[f32],
    limit: i64,
) -> AppResult<Vec<Uuid>> {
    let q = pgvector::Vector::from(embedding.to_vec());
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM concept_mappings
          WHERE kb_id = $1 AND status = 'confirmed' AND embedding IS NOT NULL
            AND embedded_model = $4
            AND vector_dims(embedding) = vector_dims($2)
          ORDER BY embedding <=> $2
          LIMIT $3",
    )
    .bind(kb_id)
    .bind(&q)
    .bind(limit)
    .bind(model)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 全部确认口径的文字，词面一路在 Rust 里打分——一个库几十到几百条，
/// 拉回来比在 SQL 里做中文分词便宜得多，也不用 pg_trgm
pub async fn confirmed_texts(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<MappingText>> {
    Ok(sqlx::query_as(
        "SELECT m.id, e.canonical_name AS concept_name, m.summary, m.unit, m.table_name, m.expr
           FROM concept_mappings m JOIN entities e ON e.id = m.concept_id
          WHERE m.kb_id = $1 AND m.status = 'confirmed'",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 按 id 取回口径，**保持给定的顺序**——顺序是检索排出来的，提示词里靠前的先被读
pub async fn by_ids(pool: &PgPool, kb_id: Uuid, ids: &[Uuid]) -> AppResult<Vec<ConceptMapping>> {
    let rows: Vec<ConceptMapping> = sqlx::query_as(
        "SELECT m.id, m.concept_id, e.canonical_name AS concept_name, m.source,
                m.table_name, m.expr, m.sql, m.unit, m.summary, m.derived, m.status, m.written_by,
                m.decided_by, m.confidence, m.rationale
           FROM concept_mappings m JOIN entities e ON e.id = m.concept_id
          WHERE m.kb_id = $1 AND m.id = ANY($2)",
    )
    .bind(kb_id)
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut by_id: std::collections::HashMap<Uuid, ConceptMapping> =
        rows.into_iter().map(|m| (m.id, m)).collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// 一条口径改过几次、每次改之前是什么样。
///
/// `revise` 从建表起就在写 `concept_mapping_revisions`，而**在此之前没有任何
/// 地方读它**——留痕留了个寂寞。0006 说留痕是为了「问数回溯历史报表时答得出
/// 『上季度这个数是怎么算的』」，那就得有人看得见。
pub async fn revisions(
    pool: &PgPool,
    kb_id: Uuid,
    mapping_id: Uuid,
) -> AppResult<Vec<MappingRevision>> {
    // kb_id 走 JOIN 校验归属：revisions 表自己没有 kb_id，
    // 不校验就能拿别的库的口径历史
    Ok(sqlx::query_as(
        "SELECT r.id, r.before, u.display_name AS changed_by_name, r.changed_at
           FROM concept_mapping_revisions r
           JOIN concept_mappings m ON m.id = r.mapping_id
           LEFT JOIN users u ON u.id = r.changed_by
          WHERE r.mapping_id = $2 AND m.kb_id = $1
          ORDER BY r.changed_at DESC
          LIMIT 50",
    )
    .bind(kb_id)
    .bind(mapping_id)
    .fetch_all(pool)
    .await?)
}
