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

/// 回放时列表与总数也按当时可见的实体算，走真的合并路径。A 在四月并进 B：
/// 三月搜「Zhang」要看见 A 和 B、各一度、总数 2；现在只剩 B。
/// 只回放度数、列表还按 `merged_into IS NULL` 过滤的话，三月只看见 B
#[tokio::test]
async fn a_search_in_replay_lists_the_entities_of_that_moment() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, works_at) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (a, b, later, acme, other) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'as-of-search-merge')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'as-of-search-merge')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'as-of-search-merge')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    let run = async {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'works_at', 'works at')",
        )
        .bind(works_at)
        .bind(kb)
        .execute(&pool)
        .await?;
        for (id, name, created) in [
            (a, "Zhang Wei A", "2026-01-01T00:00:00Z"),
            (b, "Zhang Wei B", "2026-01-01T00:00:00Z"),
            // 五月才建：三月的搜索里不该有它
            (later, "Zhang Wei C", "2026-05-01T00:00:00Z"),
            (acme, "Acme", "2026-01-01T00:00:00Z"),
            (other, "Other Co", "2026-01-01T00:00:00Z"),
        ] {
            sqlx::query(
                "INSERT INTO entities (id, kb_id, canonical_name, created_at) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(kb)
            .bind(name)
            .bind(t(created))
            .execute(&pool)
            .await?;
        }
        for (subject, object) in [(a, acme), (b, other)] {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence, recorded_at)
                 VALUES ($1, $2, $3, $4, $5, 0.9, $6)",
            )
            .bind(Uuid::now_v7())
            .bind(kb)
            .bind(subject)
            .bind(works_at)
            .bind(object)
            .bind(t("2026-01-10T00:00:00Z"))
            .execute(&pool)
            .await?;
        }
        utopia_store::resolution::merge_entities(&pool, kb, a, b, None, "test").await?;
        sqlx::query("UPDATE entity_merges SET created_at = $2 WHERE source_id = $1")
            .bind(a)
            .bind(t("2026-04-01T00:00:00Z"))
            .execute(&pool)
            .await?;

        let (march, total) = utopia_store::graph::search_entities(
            &pool,
            kb,
            "Zhang",
            10,
            0,
            Some(t("2026-03-01T00:00:00Z")),
        )
        .await?;
        let mut seen: Vec<(Uuid, i64)> = march.iter().map(|n| (n.id, n.degree)).collect();
        seen.sort();
        let mut want = vec![(a, 1), (b, 1)];
        want.sort();
        assert_eq!(seen, want, "三月：A 还没并进 B，各一度；C 还没建");
        assert_eq!(total, 2);

        let (now, total) = utopia_store::graph::search_entities(&pool, kb, "Zhang", 10, 0, None).await?;
        let ids: Vec<Uuid> = now.iter().map(|n| n.id).collect();
        assert!(ids.contains(&b) && ids.contains(&later) && !ids.contains(&a));
        assert_eq!(total, 2);
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
