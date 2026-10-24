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
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, '营收')")
        .bind(ent)
        .bind(kb)
        .execute(pool)
        .await?;
    Ok((org, kb, ent))
}

#[tokio::test]
async fn one_concept_one_source_one_mapping() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
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
        let (a, wrote_a) = p("orders").await?;
        let (b, wrote_b) = p("orders_v2").await?;
        assert_eq!(a, b, "同一个 (概念, 源) 该是同一行，不是两行");
        assert!(wrote_a && wrote_b, "第一次插入、第二次刷新，都算写入");

        let got = utopia_store::mappings::proposed(&pool, kb, 100, 0).await?;
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
            utopia_store::mappings::proposed(&pool, kb, 100, 0)
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
    let Some(url) = utopia_store::test_db::url() else {
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

        let (id, _) = utopia_store::mappings::propose(
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

        // 下一轮探索会再次算出同一条——它不该被刷回待看，也不算写入
        let (_, written) = utopia_store::mappings::propose(
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
        assert!(!written, "撞上决定的提议不算写入，探索账把它记成 decided");
        assert!(
            utopia_store::mappings::proposed(&pool, kb, 100, 0)
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

#[tokio::test]
async fn automatic_definitions_refresh_until_a_person_decides() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, kb, ent) = fixture(&pool).await?;
    let run = async {
        let user = Uuid::now_v7();
        sqlx::query("INSERT INTO users (id, org_id, email, display_name, password_hash)
                    VALUES ($1, $2, $1 || '@auto-map.test', 'Editor', 'unused')")
            .bind(user).bind(org).execute(&pool).await?;
        let p = |confidence, rationale, expr| utopia_store::mappings::propose_with_evidence(
            &pool, kb, ent, "warehouse", Some("orders"), expr, None, None,
            Some("Revenue"), false, confidence, rationale);
        assert!(!utopia_store::kbs::get(&pool, kb).await?.auto_confirm_mappings);
        let (id, _) = p(Some(1.0), Some("The schema states revenue"), Some("SUM(amount)")).await?;
        assert_eq!(utopia_store::mappings::proposed(&pool, kb, 10, 0).await?.len(), 1);
        sqlx::query("UPDATE knowledge_bases SET auto_confirm_mappings = true WHERE id = $1")
            .bind(kb).execute(&pool).await?;
        // Neither missing/invalid evidence nor a table name on its own is enough to
        // turn configuration on. Boundary 0.8 is accepted in the next write.
        for (conf, reason, expr) in [
            (None, Some("Grounded"), Some("SUM(amount)")),
            (Some(0.79), Some("Grounded"), Some("SUM(amount)")),
            (Some(f64::NAN), Some("Grounded"), Some("SUM(amount)")),
            (Some(1.1), Some("Grounded"), Some("SUM(amount)")),
            (Some(0.9), Some("  "), Some("SUM(amount)")),
            (Some(0.9), Some("Grounded"), None),
        ] {
            p(conf, reason, expr).await?;
            assert!(utopia_store::mappings::confirmed(&pool, kb, 10).await?.is_empty());
        }
        // The trigger is scoped to this fixture's base, so other tests can run alongside it.
        let fault = format!("auto_mapping_fault_{}", kb.simple());
        sqlx::raw_sql(&format!(
            "CREATE FUNCTION {fault}() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
               IF NEW.kb_id = '{kb}' AND NEW.action = 'mapping.auto_confirmed' THEN
                 RAISE EXCEPTION 'injected mapping audit failure'; END IF; RETURN NEW; END $$;
             CREATE TRIGGER {fault} BEFORE INSERT ON audit_events
               FOR EACH ROW EXECUTE FUNCTION {fault}();"
        )).execute(&pool).await?;
        let failed = p(Some(0.9), Some("Grounded"), Some("SUM(amount)")).await;
        sqlx::raw_sql(&format!("DROP TRIGGER {fault} ON audit_events; DROP FUNCTION {fault}();"))
            .execute(&pool).await?;
        assert!(failed.is_err());
        assert!(utopia_store::mappings::confirmed(&pool, kb, 10).await?.is_empty());
        let pending = utopia_store::mappings::proposed(&pool, kb, 10, 0).await?.remove(0);
        assert_eq!(pending.expr, None, "failed auto write must restore the preceding proposal");
        assert_eq!(p(Some(0.8), Some("  The schema states revenue  "), Some("SUM(amount)")).await?, (id, true));
        let auto = utopia_store::mappings::confirmed(&pool, kb, 10).await?.remove(0);
        assert_eq!(auto.confidence, Some(0.8));
        assert_eq!(auto.rationale.as_deref(), Some("The schema states revenue"));
        assert_eq!(auto.decided_by, None);
        assert_eq!(utopia_store::mappings::page(&pool, kb, None, None, 10, 0).await?.0[0].rationale, auto.rationale);
        assert_eq!(utopia_store::mappings::by_ids(&pool, kb, &[id]).await?[0].confidence, Some(0.8));
        let events: Vec<(Option<Uuid>, serde_json::Value)> = sqlx::query_as(
            "SELECT actor_id, detail FROM audit_events WHERE target_id = $1 AND action = 'mapping.auto_confirmed'"
        ).bind(id).fetch_all(&pool).await?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, None);
        assert_eq!(events[0].1["definition"]["expr"], "SUM(amount)");
        assert_eq!(p(Some(0.9), Some("Updated schema"), Some("SUM(net_amount)")).await?, (id, true));
        assert_eq!(utopia_store::mappings::confirmed(&pool, kb, 10).await?[0].expr.as_deref(), Some("SUM(net_amount)"));
        p(Some(0.7), Some("Uncertain"), Some("SUM(amount)")).await?;
        assert!(utopia_store::mappings::confirmed(&pool, kb, 10).await?.is_empty());
        p(Some(0.9), Some("Grounded"), Some("SUM(amount)")).await?;
        sqlx::query("UPDATE knowledge_bases SET auto_confirm_mappings = false WHERE id = $1")
            .bind(kb).execute(&pool).await?;
        assert_eq!(utopia_store::mappings::confirmed(&pool, kb, 10).await?.len(), 1);
        p(Some(0.9), Some("Grounded"), Some("SUM(amount)")).await?;
        assert!(utopia_store::mappings::confirmed(&pool, kb, 10).await?.is_empty());
        sqlx::query("UPDATE knowledge_bases SET auto_confirm_mappings = true WHERE id = $1")
            .bind(kb).execute(&pool).await?;
        // Editing a pending proposal also takes ownership. Its status remains pending.
        utopia_store::mappings::revise(&pool, kb, id, Some("orders"), Some("SUM(human_amount)"),
            None, None, None, false, user).await?;
        assert_eq!(p(Some(1.0), Some("Grounded"), Some("SUM(amount)")).await?, (id, false));
        let human = utopia_store::mappings::proposed(&pool, kb, 10, 0).await?.remove(0);
        assert_eq!(human.expr.as_deref(), Some("SUM(human_amount)"));
        assert_eq!(human.decided_by, Some(user));
        assert_eq!(human.confidence, None, "model evidence described the preceding definition");
        assert_eq!(human.rationale, None);
        let revisions = utopia_store::mappings::revisions(&pool, kb, id).await?;
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].before["decided_by"], serde_json::Value::Null);
        for status in ["confirmed", "rejected"] {
            utopia_store::mappings::decide(&pool, kb, id, status, user).await?;
            assert_eq!(p(Some(1.0), Some("Grounded"), Some("SUM(amount)")).await?, (id, false));
        }
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn concurrent_automatic_proposals_return_the_stored_id() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (_, kb, ent) = fixture(&pool).await?;
    let run = async {
        sqlx::query("UPDATE knowledge_bases SET auto_confirm_mappings = true WHERE id = $1")
            .bind(kb).execute(&pool).await?;
        let p = || utopia_store::mappings::propose_with_evidence(&pool, kb, ent, "warehouse",
            None, Some("COUNT(*)"), None, None, None, false, Some(0.9), Some("Count the rows"));
        let (a, b) = tokio::join!(p(), p());
        let (a, b) = (a?, b?);
        assert_eq!(a, b);
        let ids: Vec<Uuid> = sqlx::query_scalar("SELECT target_id FROM audit_events WHERE kb_id = $1 AND action = 'mapping.auto_confirmed'")
            .bind(kb).fetch_all(&pool).await?;
        assert_eq!(ids, vec![a.0, a.0]);
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
