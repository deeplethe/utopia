//! 实体搜索结果按记录轴回放（#307 / 0019）。
//!
//! 之前 `search_entities` 写死 `node_sql(None, None)`：度量子查询拿的是「现在
//! 还活着的边」。回放中的图上点搜索框，结果的 `degree` 还数今天的边——
//! 同一个实体，两种视图下读出两个数。
//!
//! 三个方向都要断言，因为它们会以不同的方式坏掉：
//! - 撤掉的边在 `as_of = 现在` **不算入** `degree`
//! - 撤掉的边在作废时刻**之前**算入 `degree`（谓词没接上时永远只看「现在」，
//!   回放照旧空数）
//! - `recorded_at` 晚于 T 的边在 T **不算入** `degree`（只写下界会让三月看
//!   见四月的修正）
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

#[tokio::test]
async fn search_entities_degree_respects_as_of() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let person_type = Uuid::now_v7();
    let project_type = Uuid::now_v7();
    let leads = Uuid::now_v7();
    // 两个 person 实体，让搜索词能同时命中两个，结果按 degree 倒序排
    let alpha = Uuid::now_v7();
    let beta = Uuid::now_v7();
    // 一个 project 实体（被指的那个对象）
    let project_entity = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'as-of-search-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'as-of-search-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'as-of-search-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    for (id, key, label) in [
        (person_type, "person", "Person"),
        (project_type, "project", "Project"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(&pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'leads', 'leads')",
    )
    .bind(leads)
    .bind(kb)
    .execute(&pool)
    .await?;
    for (id, type_id, name) in [
        (alpha, person_type, "Alpha Person"),
        (beta, person_type, "Beta Person"),
        (project_entity, project_type, "Project Phoenix"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .bind(t("2026-01-01T00:00:00Z"))
        .execute(&pool)
        .await?;
    }

    // alpha：1 月记下「leads project_entity」，没有作废
    let f_alpha = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                           confidence, recorded_at)
         VALUES ($1, $2, $3, $4, $5, 0.9, $6)",
    )
    .bind(f_alpha)
    .bind(kb)
    .bind(alpha)
    .bind(leads)
    .bind(project_entity)
    .bind(t("2026-01-15T00:00:00Z"))
    .execute(&pool)
    .await?;

    // beta：1 月记下「leads project_entity」→ 3 月作废；5 月又记下第二条
    let f_beta_old = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                           confidence, recorded_at)
         VALUES ($1, $2, $3, $4, $5, 0.9, $6)",
    )
    .bind(f_beta_old)
    .bind(kb)
    .bind(beta)
    .bind(leads)
    .bind(project_entity)
    .bind(t("2026-01-20T00:00:00Z"))
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE facts SET invalidated_at = $2 WHERE id = $1 AND invalidated_at IS NULL")
        .bind(f_beta_old)
        .bind(t("2026-03-10T00:00:00Z"))
        .execute(&pool)
        .await?;
    let f_beta_new = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                           confidence, recorded_at)
         VALUES ($1, $2, $3, $4, $5, 0.9, $6)",
    )
    .bind(f_beta_new)
    .bind(kb)
    .bind(beta)
    .bind(leads)
    .bind(project_entity)
    .bind(t("2026-05-10T00:00:00Z"))
    .execute(&pool)
    .await?;

    let find_degree = |rows: &[utopia_core::models::GraphNode], id: Uuid| -> i64 {
        rows.iter()
            .find(|n| n.id == id)
            .map(|n| n.degree)
            .unwrap_or(-1)
    };

    // 当下：alpha=1，beta=1（旧的已作废、新的还活着）
    let now_rows = utopia_store::graph::search_entities(&pool, kb, "Person", 50, 0, None).await?;
    assert_eq!(find_degree(&now_rows.0, alpha), 1, "当下 alpha degree=1");
    assert_eq!(
        find_degree(&now_rows.0, beta),
        1,
        "当下 beta degree=1（新的那条）"
    );

    // 2 月：alpha 已记下=1；beta 的旧边还在作废之前=1
    let feb_rows = utopia_store::graph::search_entities(
        &pool,
        kb,
        "Person",
        50,
        0,
        Some(t("2026-02-01T00:00:00Z")),
    )
    .await?;
    assert_eq!(find_degree(&feb_rows.0, alpha), 1, "2 月 alpha degree=1");
    assert_eq!(
        find_degree(&feb_rows.0, beta),
        1,
        "2 月 beta 旧边在作废之前 degree=1"
    );

    // 4 月：alpha 还是 1；beta 旧边已作废、新边还没记下 → 0
    let apr_rows = utopia_store::graph::search_entities(
        &pool,
        kb,
        "Person",
        50,
        0,
        Some(t("2026-04-01T00:00:00Z")),
    )
    .await?;
    assert_eq!(find_degree(&apr_rows.0, alpha), 1, "4 月 alpha degree=1");
    assert_eq!(
        find_degree(&apr_rows.0, beta),
        0,
        "4 月 beta 旧边已作废、新边还没记下 degree=0"
    );

    // 6 月：alpha 还是 1；beta 新边已记下=1
    let jun_rows = utopia_store::graph::search_entities(
        &pool,
        kb,
        "Person",
        50,
        0,
        Some(t("2026-06-01T00:00:00Z")),
    )
    .await?;
    assert_eq!(find_degree(&jun_rows.0, alpha), 1, "6 月 alpha degree=1");
    assert_eq!(
        find_degree(&jun_rows.0, beta),
        1,
        "6 月 beta 新边已记下 degree=1"
    );

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
