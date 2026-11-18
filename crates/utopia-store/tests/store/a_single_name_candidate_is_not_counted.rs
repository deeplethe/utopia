//! 同名候选只有一个时没有可比的，不必数它的事实（#1296）。
//! 一条连接的池子：sqlx 把它跑过的语句都缓存在 `pg_prepared_statements` 里，没跑过的就不在。

use sqlx::postgres::PgPoolOptions;
use utopia_store::names;
use uuid::Uuid;

#[tokio::test]
async fn a_single_name_candidate_is_attached_without_counting_its_facts() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, entity) = (Uuid::now_v7(), Uuid::now_v7());
    let tag = "single-name-candidate";
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(tag)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(tag)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(tag)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
        .bind(person)
        .bind(kb)
        .bind("person")
        .bind("Person")
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(entity)
    .bind(kb)
    .bind(person)
    .bind("林小满")
    .execute(&pool)
    .await?;
    names::record(&pool, kb, entity, "林小满", None, None).await?;

    let run = async {
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            kb,
            Some(person),
            "林小满",
            None,
            None,
            None,
            &[],
        )
        .await?;
        assert!(!r.created && r.entity_id == entity, "唯一的同名候选就是它");
        let counted: Vec<String> = sqlx::query_scalar(
            "SELECT statement FROM pg_prepared_statements WHERE statement LIKE '%count(*)%facts f%'",
        )
        .persistent(false)
        .fetch_all(&pool)
        .await?;
        assert_eq!(counted, Vec::<String>::new(), "只有一个候选，不该数事实");
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
