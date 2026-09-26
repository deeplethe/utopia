//! 按名字找实体：名字完全相同的排在最前，再按度数。
//!
//! 从前只按度数排、截在 `limit` 上。问「Apple」而库里有九个事实更多的「Apple Store …」，
//! 叫 Apple 的那个排到第十——对话与 MCP 的工具只读前八条，于是它找不到，按名字读事实时
//! 读成了另一个。别名同理：「IBM」是 International Business Machines 的一个名字（0041），
//! 问 IBM 的人要的就是它。
//!
//! 回放那一支多绑一个时刻参数，顺序跟着挪了一位，所以也要走一遍。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

async fn entity(pool: &PgPool, kb: Uuid, ty: Uuid, name: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(kb)
    .bind(ty)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 一条事实，让 `subject` 的度数加一
async fn busy(
    pool: &PgPool,
    kb: Uuid,
    subject: Uuid,
    rel: Uuid,
    object: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, recorded_at)
         VALUES ($1, $2, $3, $4, $5, now())",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(subject)
    .bind(rel)
    .bind(object)
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_search_puts_the_exact_name_first() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, ty, rel) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'exact-name-search-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'exact-name-search-test')",
    )
    .bind(ws)
    .bind(org)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'exact-name-search-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'organization', 'Organization')",
        )
        .bind(ty)
        .bind(kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind)
             VALUES ($1, $2, 'located_in', 'located in', 'relation')",
        )
        .bind(rel)
        .bind(kb)
        .execute(&pool)
        .await?;
        let city = entity(&pool, kb, ty, "Cupertino").await?;

        // 叫 Apple 的没有事实；九个名字里带 Apple 的各有一条，度数都比它高
        let apple = entity(&pool, kb, ty, "Apple").await?;
        for i in 1..=9 {
            let store = entity(&pool, kb, ty, &format!("Apple Store {i}")).await?;
            busy(&pool, kb, store, rel, city).await?;
        }
        let (rows, total) =
            utopia_store::graph::search_entities(&pool, kb, "Apple", 8, 0, None).await?;
        assert_eq!(total, 10);
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0].id, apple, "the entity named exactly Apple comes first");

        // 回放那一支：时刻参数挪了一位，同名仍在最前
        let (rows, _) = utopia_store::graph::search_entities(
            &pool,
            kb,
            "Apple",
            8,
            0,
            Some(chrono::Utc::now()),
        )
        .await?;
        assert_eq!(rows[0].id, apple, "the same holds when replaying a moment");

        // 别名：IBM 是 International Business Machines 的一个名字，大小写不论
        let ibm = entity(&pool, kb, ty, "International Business Machines").await?;
        utopia_store::names::record(&pool, kb, ibm, "IBM", None, None).await?;
        for i in 1..=9 {
            let lab = entity(&pool, kb, ty, &format!("IBM Lab {i}")).await?;
            busy(&pool, kb, lab, rel, city).await?;
        }
        let (rows, total) =
            utopia_store::graph::search_entities(&pool, kb, "ibm", 8, 0, None).await?;
        assert_eq!(total, 10);
        assert_eq!(rows[0].id, ibm, "an entity whose name is exactly IBM comes first");
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
