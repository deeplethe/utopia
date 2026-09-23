//! 两份文档并行抽取，同一个名字同时到达消解：只建一个实体。
//!
//! 实测「澜图数据」在同一秒里建了两个，之后每次提到它都撞上两个候选、再各建一个，
//! 一篇语料跑完裂成四个。新建按（库，名字）拿咨询锁串行化，锁里再查一次。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn a_name_created_twice_at_once_is_one_entity() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let tag = Uuid::now_v7();
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(format!("race-{tag}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(format!("race-{tag}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(format!("race-{tag}"))
        .execute(&pool)
        .await?;
    let class = Uuid::now_v7();
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
        .bind(class)
        .bind(kb)
        .bind("company")
        .bind("公司")
        .execute(&pool)
        .await?;

    let name = format!("澜图数据-{tag}");
    let go = || {
        utopia_store::resolution::resolve_mention(&pool, kb, Some(class), &name, None, None, &[])
    };
    let (a, b, c, d) = tokio::join!(go(), go(), go(), go());
    let ids = [a?.entity_id, b?.entity_id, c?.entity_id, d?.entity_id];
    assert!(
        ids.iter().all(|id| *id == ids[0]),
        "四次并行消解落到同一个实体：{ids:?}"
    );
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entities WHERE kb_id = $1 AND canonical_name = $2 AND merged_into IS NULL",
    )
    .bind(kb)
    .bind(&name)
    .fetch_one(&pool)
    .await?;
    assert_eq!(rows, 1, "库里只有一个这个名字");

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
