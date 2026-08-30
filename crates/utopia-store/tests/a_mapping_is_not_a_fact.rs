//! 语义层映射的两条硬性质——打在真库上（见 `docs/decisions/0011`）。
//!
//! 这两条都是搬出账本换来的东西，也正是从前做不到的：
//!
//! 1. **同一个 (概念, 源) 只有一条。** 从前这条唯一性藏在 `object_value`
//!    这个 JSONB 内部，数据库看不见，只能靠确认流程显式闭合——也就是靠流程
//!    而不是约束。现在它是主键。
//! 2. **表过态的不被下一轮探索刷回待看。** `ontology_proposals` 那边踩过同一个
//!    坑（`ontology_proposals` 那边）：重跑必然再次算出被拒绝过的那条，不挡住就等于每跑一次都把
//!    人的否决抹掉一次。

use sqlx::PgPool;
use uuid::Uuid;

async fn fixture(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (org, ws, kb, ent) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'map-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'map-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'map-test')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, aliases) VALUES ($1, $2, '营收', '{}')",
    )
    .bind(ent)
    .bind(kb)
    .execute(pool)
    .await?;
    Ok((org, kb, ent))
}

#[tokio::test]
async fn one_concept_one_source_one_mapping() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (_org, kb, ent) = fixture(&pool).await?;

    let run = async {
        let p = |t: &'static str| {
            utopia_store::mappings::propose(
                &pool,
                kb,
                ent,
                "warehouse",
                Some(t),
                None,
                None,
                None,
                None,
                false,
            )
        };
        let a = p("orders").await?;
        let b = p("orders_v2").await?;
        assert_eq!(a, b, "同一个 (概念, 源) 该是同一行，不是两行");

        let got = utopia_store::mappings::proposed(&pool, kb, 100).await?;
        assert_eq!(got.len(), 1, "只该有一条");
        assert_eq!(
            got[0].table_name.as_deref(),
            Some("orders_v2"),
            "重跑探索该刷新定义"
        );

        // 换一个源就是另一条：同一概念在不同源上有不同定义是有意支持的
        utopia_store::mappings::propose(
            &pool,
            kb,
            ent,
            "lakehouse",
            Some("f_orders"),
            None,
            None,
            None,
            None,
            false,
        )
        .await?;
        assert_eq!(
            utopia_store::mappings::proposed(&pool, kb, 100)
                .await?
                .len(),
            2,
            "不同源该各有一条"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // **只删知识库,不删 org/user。** 用户是软删除的,生产代码里没有
    // `DELETE FROM users`——测试也不该造一个产品里不存在的动作,否则
    // 撞上的外键约束是夹具自己的问题,会被误当成产品缺陷（实测发生过）
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn a_rejected_mapping_does_not_come_back() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb, ent) = fixture(&pool).await?;

    let run = async {
        let user = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO users (id, org_id, email, display_name, password_hash)
             VALUES ($1, $2, $1 || '@m.test', 'm', 'x')",
        )
        .bind(user)
        .bind(org)
        .execute(&pool)
        .await?;

        let id = utopia_store::mappings::propose(
            &pool,
            kb,
            ent,
            "warehouse",
            Some("orders"),
            None,
            None,
            None,
            None,
            false,
        )
        .await?;
        utopia_store::mappings::decide(&pool, kb, id, "rejected", user).await?;

        // 下一轮探索会再次算出同一条——它不该被刷回待看
        utopia_store::mappings::propose(
            &pool,
            kb,
            ent,
            "warehouse",
            Some("orders"),
            None,
            None,
            None,
            None,
            false,
        )
        .await?;
        assert!(
            utopia_store::mappings::proposed(&pool, kb, 100)
                .await?
                .is_empty(),
            "拒绝过的不该重新排队"
        );
        // 确认过的同理不该被探索覆盖
        assert!(
            utopia_store::mappings::confirmed(&pool, kb, 100)
                .await?
                .is_empty(),
            "拒绝的也不该出现在确认列表里"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // **只删知识库,不删 org/user。** 用户是软删除的,生产代码里没有
    // `DELETE FROM users`——测试也不该造一个产品里不存在的动作,否则
    // 撞上的外键约束是夹具自己的问题,会被误当成产品缺陷（实测发生过）
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
