//! 检索候选要**带上祖先**，否则提示词里没有泛化基类。
//!
//! 为什么非要连库：`ancestors_of` 整个活在一段递归 SQL 里，多继承与菱形
//! 走不走得通、会不会把同一个祖先展开两次，`cargo check` 一个字都不说。
//!
//! 守的是这条实测出来的因果链：向量检索天然偏爱字面出现在正文里的叶子类
//!（一个讲 Sutskever 的分块，976 个类里 `researcher` 排第 4、`person` 排第 359），
//! 前 40 名里一个泛化基类都没有。于是两个症状一起出现——实体被判成
//! `researcher`（schema.org 里它是 `Audience` 的子类），而
//! `employee (organization → person)` 的签名退化成 `(* → *)`，模型根本没见过方向约束。
//!
//! 从前这道地板由「内置类恒在」兜着，种子退场后判据悬空了。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆。

use sqlx::PgPool;
use uuid::Uuid;

/// 造一段菱形：`researcher → audience → thing`，`corporation → organization → thing`，
/// 外加一个多继承的 `agent`（同时挂在 thing 与 organization 下）。
///
/// 菱形是关键：没有去重的递归会把 `thing` 展开两次。
async fn seed(pool: &PgPool) -> anyhow::Result<(Uuid, Vec<(&'static str, Uuid)>)> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'floor-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'floor-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'floor-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;

    let keys = [
        "thing",
        "audience",
        "researcher",
        "organization",
        "corporation",
        "agent",
    ];
    let mut ids: Vec<(&'static str, Uuid)> = Vec::new();
    for k in keys {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
            .bind(id)
            .bind(kb)
            .bind(k)
            .execute(pool)
            .await?;
        ids.push((k, id));
    }
    let get = |k: &str| ids.iter().find(|(n, _)| *n == k).unwrap().1;
    for (child, parent) in [
        ("audience", "thing"),
        ("researcher", "audience"),
        ("organization", "thing"),
        ("corporation", "organization"),
        // 多继承 + 菱形：agent 两条路都通到 thing
        ("agent", "thing"),
        ("agent", "organization"),
    ] {
        sqlx::query(
            "INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(get(child))
        .bind(get(parent))
        .execute(pool)
        .await?;
    }
    Ok((kb, ids))
}

#[tokio::test]
async fn a_retrieved_leaf_brings_its_ancestors() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // 开跑前先扫地：断言 panic 会跳过 teardown
    sqlx::query("DELETE FROM organizations WHERE name = 'floor-test'")
        .execute(&pool)
        .await?;
    let (kb, ids) = seed(&pool).await?;
    let id = |k: &str| ids.iter().find(|(n, _)| *n == k).unwrap().1;

    let run = async {
        // 检索只捞到了叶子——正文写的是 "a researcher at the corporation"
        let leaves = vec![id("researcher"), id("corporation")];
        let anc = utopia_store::ontology::ancestors_of(&pool, &leaves).await?;

        for k in ["audience", "thing", "organization"] {
            assert!(anc.contains(&id(k)), "祖先里少了 {k}——地板没补上");
        }
        // 自己不算祖先：调用方会把两者并起来，重复只是噪声
        for k in ["researcher", "corporation"] {
            assert!(!anc.contains(&id(k)), "{k} 是它自己，不该出现在祖先里");
        }

        // **菱形去重**：agent 有两条路通到 thing，thing 只该出现一次
        let anc2 = utopia_store::ontology::ancestors_of(&pool, &[id("agent")]).await?;
        let things = anc2.iter().filter(|x| **x == id("thing")).count();
        assert_eq!(things, 1, "菱形继承把 thing 展开了 {things} 次");
        assert!(anc2.contains(&id("organization")), "多继承的另一条腿丢了");

        // 空输入不该炸，也不该扫全表
        assert!(utopia_store::ontology::ancestors_of(&pool, &[])
            .await?
            .is_empty());
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = 'floor-test'")
        .execute(&pool)
        .await?;
    let _ = kb;
    run
}
