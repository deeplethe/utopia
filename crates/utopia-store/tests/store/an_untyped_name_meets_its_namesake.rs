//! 没有类的实体也要撞得上同名者（0044 第一刀之后的实测）。
//!
//! 开放图谱里的实体都没有类：类由对齐来定，抽取只记文档的字。`resolve_mention` 的候选
//! 查询从前写的是 `e.type_id = $2`，传 NULL 时这个条件永远不成立——于是同一个库里
//! 「Securities and Exchange Commission」和它的全大写写法成了两个实体，只有建实体时
//! 的精确同名匹配还兜得住。这里守的是：两次没有类的提及，写法只差大小写，落到同一个实体。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn an_untyped_mention_attaches_to_its_untyped_namesake() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'untyped-namesake-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'untyped-namesake-test')",
    )
    .bind(ws)
    .bind(org)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'untyped-namesake-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let first = utopia_store::resolution::resolve_mention(
        &pool,
        kb,
        None,
        "Securities and Exchange Commission",
        None,
        None,
        &[],
    )
    .await?;
    assert!(first.created, "第一次提及要建实体");
    let again = utopia_store::resolution::resolve_mention(
        &pool,
        kb,
        None,
        "SECURITIES AND EXCHANGE COMMISSION",
        None,
        None,
        &[],
    )
    .await?;
    assert_eq!(
        again.entity_id, first.entity_id,
        "只差大小写的同名提及要落到同一个没有类的实体上"
    );
    assert!(!again.created);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entities WHERE kb_id = $1 AND merged_into IS NULL",
    )
    .bind(kb)
    .fetch_one(&pool)
    .await?;
    assert_eq!(n, 1);

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
