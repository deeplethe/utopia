//! `exact = true`: the entity named exactly so, and nothing that merely contains the name.
//!
//! A caller that knows the name (a path, a symbol) wants one entity, not the substring scan of
//! the whole base. The default stays the substring search, exact name first.
//!
//! Skipped without `UTOPIA_DATABASE_URL`. Builds and removes its own rows.

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

#[tokio::test]
async fn an_exact_name_search_returns_only_that_entity() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, ty) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'exact-only-search-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'exact-only-search-test')",
    )
    .bind(ws)
    .bind(org)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'exact-only-search-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'file', 'File')",
        )
        .bind(ty)
        .bind(kb)
        .execute(&pool)
        .await?;
        let file = entity(&pool, kb, ty, "acme/repo:src/a.ts").await?;
        let symbol = entity(&pool, kb, ty, "acme/repo:src/a.ts#parse").await?;

        let (rows, total) = utopia_store::graph::search_entities(
            &pool,
            kb,
            "acme/repo:src/a.ts",
            10,
            0,
            None,
            true,
        )
        .await?;
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![file],
            "an exact search returns the entity named so and not the one that contains the name"
        );
        assert_eq!(total, 1, "the total counts only the exact match");

        let (rows, total) = utopia_store::graph::search_entities(
            &pool,
            kb,
            "acme/repo:src/a.ts",
            10,
            0,
            None,
            false,
        )
        .await?;
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![file, symbol],
            "without exact both are found, the exact name first"
        );
        assert_eq!(total, 2);

        let (rows, total) = utopia_store::graph::search_entities(
            &pool,
            kb,
            "ACME/Repo:SRC/a.ts",
            10,
            0,
            None,
            true,
        )
        .await?;
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![file],
            "the name is folded to lower case, so another case still finds it"
        );
        assert_eq!(total, 1);
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
