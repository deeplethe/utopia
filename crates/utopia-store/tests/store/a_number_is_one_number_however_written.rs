//! 同一条边上，先写 `65`（老库里的整数）再写 `65.0`：是同一个数，不是冲突。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn a_number_is_one_number_however_written() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let tag = Uuid::now_v7();
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(format!("num-{tag}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(format!("num-{tag}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(format!("num-{tag}"))
        .execute(&pool)
        .await?;
    let class = Uuid::now_v7();
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
        .bind(class)
        .bind(kb)
        .bind("org")
        .bind("Org")
        .execute(&pool)
        .await?;
    let stake = utopia_store::ontology::create_relation_type(
        &pool,
        kb,
        "stake",
        "stake",
        "state",
        Default::default(),
        "",
        "attribute",
        &[class],
        &[],
        Some("number"),
        Some("%"),
    )
    .await?;
    let holds = utopia_store::ontology::create_relation_type(
        &pool,
        kb,
        "holds_stake_in",
        "holds stake in",
        "state",
        Default::default(),
        "",
        "relation",
        &[],
        &[],
        None,
        None,
    )
    .await?;
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, name) in [(a, "澜图科技"), (b, "澜图新材料")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(class)
        .bind(format!("{name}-{tag}"))
        .execute(&pool)
        .await?;
    }
    let (fact, _) = utopia_store::graph::insert_fact(
        &pool,
        kb,
        a,
        Some(holds),
        b,
        utopia_store::graph::Validity::default(),
        0.9,
    )
    .await?;

    let as_int = serde_json::json!({ "value": 65, "unit": "%" });
    let as_float = serde_json::json!({ "value": 65.0, "unit": "%" });
    let other = serde_json::json!({ "value": 60.0, "unit": "%" });
    assert_eq!(
        utopia_store::graph::upsert_fact_qualifier(&pool, fact, stake, &as_int).await?,
        utopia_store::graph::QualifierWrite::Set
    );
    assert_eq!(
        utopia_store::graph::upsert_fact_qualifier(&pool, fact, stake, &as_float).await?,
        utopia_store::graph::QualifierWrite::Same,
        "65 与 65.0 是同一个数"
    );
    assert_eq!(
        utopia_store::graph::upsert_fact_qualifier(&pool, fact, stake, &other).await?,
        utopia_store::graph::QualifierWrite::Conflict,
        "60 才是不一样的数"
    );

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
