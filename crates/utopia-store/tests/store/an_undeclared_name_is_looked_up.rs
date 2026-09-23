//! `existing_by_name`（#559）：一个没声明的主宾是不是库里已经有的东西。
//! 不问类型，认别名，并掉的不算；没有就是没有。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    acme: Uuid,
    merged: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (organization, acme, merged) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'lookup-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'lookup-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'lookup-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'organization', 'Organization')")
        .bind(organization)
        .bind(kb)
        .execute(pool)
        .await?;
    // 有类型的、带别名的一个；并掉的一个（同名，不该被找到）。
    // 别名是一条名字事实（0041），不再是 entities 上的一列
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name)
         VALUES ($1, $2, $3, 'Acme Corporation')",
    )
    .bind(acme)
    .bind(kb)
    .bind(organization)
    .execute(pool)
    .await?;
    utopia_store::names::record(pool, kb, acme, "Acme Corporation", None, None).await?;
    utopia_store::names::record(pool, kb, acme, "ACME", None, None).await?;
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, merged_into) VALUES ($1, $2, 'Nova Labs', $3)",
    )
    .bind(merged)
    .bind(kb)
    .bind(acme)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        acme,
        merged,
    })
}

async fn teardown(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_known_name_is_found_by_any_spelling_and_an_unknown_one_is_not() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let run = async {
        let find = |name: &'static str| {
            let pool = pool.clone();
            async move { utopia_store::resolution::existing_by_name(&pool, f.kb, name).await }
        };
        assert_eq!(find("Acme Corporation").await?, Some(f.acme), "原名");
        assert_eq!(find("acme corporation").await?, Some(f.acme), "大小写不论");
        assert_eq!(find("ACME").await?, Some(f.acme), "别名也算");
        assert_eq!(
            find("Acme").await?,
            Some(f.acme),
            "泛用后缀词干互推：Acme ↔ Acme Corporation"
        );
        assert_eq!(find("Nova Labs").await?, None, "并掉的不算");
        assert_eq!(find("lawsuit against Acme").await?, None, "描述不是实体");
        let _ = f.merged;
        anyhow::Ok(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}
