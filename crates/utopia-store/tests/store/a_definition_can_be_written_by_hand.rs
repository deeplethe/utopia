//! 人从零写的口径——打在真库上（#562）。
//!
//! 三条性质：
//! 1. **写下来就是确认的**，且记得是谁写的（`written_by`），探索提的那些为空。
//! 2. **同一个 (概念, 源) 不悄悄覆盖**：已经有一条时报冲突，改要走 `revise`。
//! 3. **探索盖不掉人写的**：`propose` 只刷新 `proposed` 的行——与它不刷回一条
//!    拒绝是同一条规则。

use sqlx::PgPool;
use uuid::Uuid;

async fn fixture(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (org, ws, kb, ent, user) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'write-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'write-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'write-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'GMV')")
        .bind(ent)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $1 || '@w.test', 'w', 'x')",
    )
    .bind(user)
    .bind(org)
    .execute(pool)
    .await?;
    Ok((kb, ent, user))
}

#[tokio::test]
async fn a_written_definition_is_confirmed_and_signed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (kb, ent, user) = fixture(&pool).await?;

    let run = async {
        let id = utopia_store::mappings::create(
            &pool,
            kb,
            ent,
            "warehouse",
            Some("dw.dwd_ord_dtl"),
            Some("sum(amt_pay) FILTER (WHERE pay_st IN (1,2) AND is_test = 0) / 100.0"),
            None,
            Some("CNY"),
            Some("paid, non-test, in yuan"),
            false,
            user,
        )
        .await?;

        // 落下来就是确认的：问数下一句就读得到，不用再过审
        let confirmed = utopia_store::mappings::confirmed(&pool, kb, 100).await?;
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].id, id);
        assert_eq!(confirmed[0].written_by, Some(user), "人写的记谁写的");
        assert!(
            utopia_store::mappings::proposed(&pool, kb, 100, 0)
                .await?
                .is_empty(),
            "不该出现在待审队列里"
        );

        // 同一个 (概念, 源) 再写一条：冲突，而不是悄悄盖掉已有的那条
        let again = utopia_store::mappings::create(
            &pool,
            kb,
            ent,
            "warehouse",
            Some("dw.dwd_ord_dtl"),
            Some("sum(amt_total)"),
            None,
            None,
            None,
            false,
            user,
        )
        .await;
        assert!(
            matches!(again, Err(utopia_core::AppError::Conflict(_))),
            "同键第二条该是 Conflict，得到 {again:?}"
        );

        // 探索盖不掉人写的：propose 同键 → 行原样不动、仍是 confirmed
        utopia_store::mappings::propose(
            &pool,
            kb,
            ent,
            "warehouse",
            Some("orders_v2"),
            Some("sum(amt_total)"),
            None,
            None,
            None,
            false,
        )
        .await?;
        let confirmed = utopia_store::mappings::confirmed(&pool, kb, 100).await?;
        assert_eq!(confirmed.len(), 1);
        assert_eq!(confirmed[0].table_name.as_deref(), Some("dw.dwd_ord_dtl"));
        assert_eq!(confirmed[0].written_by, Some(user));
        // 探索自己提的那些 written_by 为空——页面靠这个分「人写」与「探索提的」
        let ent2 = Uuid::now_v7();
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'Orders')")
            .bind(ent2)
            .bind(kb)
            .execute(&pool)
            .await?;
        utopia_store::mappings::propose(
            &pool,
            kb,
            ent2,
            "warehouse",
            Some("dw.dwd_ord_dtl"),
            Some("count(distinct ord_id)"),
            None,
            None,
            None,
            false,
        )
        .await?;
        let proposed = utopia_store::mappings::proposed(&pool, kb, 100, 0).await?;
        assert_eq!(proposed.len(), 1);
        assert_eq!(proposed[0].written_by, None);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 只删知识库，不删 org/user——用户是软删除的，测试也不该造一个产品里不存在的动作
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
