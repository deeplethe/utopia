//! 判定落下之后建的实体也拿到它类别词绑上的类（`apply_bound`）。
//!
//! 判定那一刻的投影（`apply`）只看得见那一刻的实体；抽到一半的文档后半建的公司落成没有类的，
//! 它的属性陈述就进不了任何声明了定义域的属性的候选。没有 `UTOPIA_DATABASE_URL` 时跳过。

use super::apply_bound;
use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    class: Uuid,
    late: Uuid,
    human: Uuid,
    other: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb, class, late, human, other) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'catch-up')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'catch-up')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'catch-up')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO entity_types(id,kb_id,key,label) VALUES($1,$2,'company','企业')")
        .bind(class)
        .bind(kb)
        .execute(pool)
        .await?;
    // 「公司」已经绑到企业类；判定时还没有下面这些实体
    sqlx::query(
        "INSERT INTO type_bindings(id,kb_id,kind_word,type_id,status) VALUES($1,$2,'公司',$3,'bound')",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(class)
    .execute(pool)
    .await?;
    // 判定之后抽出来的公司：类别词写法带空白，归一后是同一个词
    sqlx::query(
        "INSERT INTO entities(id,kb_id,canonical_name,specific_type) VALUES($1,$2,'蓝海物流有限公司',' 公司 ')",
    )
    .bind(late)
    .bind(kb)
    .execute(pool)
    .await?;
    // 人说了它没有类：绑定不盖人的决定
    sqlx::query(
        "INSERT INTO entities(id,kb_id,canonical_name,specific_type,type_source) VALUES($1,$2,'某公司','公司','human')",
    )
    .bind(human)
    .bind(kb)
    .execute(pool)
    .await?;
    // 别的类别词：没有绑定，不动
    sqlx::query(
        "INSERT INTO entities(id,kb_id,canonical_name,specific_type,proposed_type) VALUES($1,$2,'王海','董事长','chairman')",
    )
    .bind(other)
    .bind(kb)
    .execute(pool)
    .await?;
    Ok(Fx {
        org,
        kb,
        class,
        late,
        human,
        other,
    })
}

async fn class_of(
    pool: &PgPool,
    id: Uuid,
) -> anyhow::Result<(Option<Uuid>, String, Option<String>)> {
    Ok(
        sqlx::query_as("SELECT type_id, type_source, proposed_type FROM entities WHERE id=$1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
async fn an_entity_built_after_the_decision_still_gets_its_class() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        assert_eq!(apply_bound(&pool, f.kb).await?, 1);
        assert_eq!(
            class_of(&pool, f.late).await?,
            (Some(f.class), "aligned".into(), None)
        );
        assert_eq!(
            class_of(&pool, f.human).await?,
            (None, "human".into(), None)
        );
        assert_eq!(
            class_of(&pool, f.other).await?,
            (None, "extracted".into(), Some("chairman".into()))
        );
        // 幂等：已经在那个类上的不再算改动
        assert_eq!(apply_bound(&pool, f.kb).await?, 0);
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
