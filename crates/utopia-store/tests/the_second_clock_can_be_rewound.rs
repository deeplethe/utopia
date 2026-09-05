//! 记录轴回退（0019 / #307），打在真库上。
//!
//! 为什么非要连库：这道防御**整个活在 SQL 字符串里**。`cargo check` 看不见
//! 一个漏写的 WHERE，clippy 也不会说一个字——0009 栽的正是这一跤（`NULL <> uuid`
//! 不报错、只是什么都不选），`human_type_decisions` 那个测试就是那次留下的。
//!
//! 两个方向都要断言，因为它们会以不同的方式坏掉：
//! - 撤掉的行在 `as_of = 现在` **不出现**（谓词写反了会让作废行全体复活）
//! - 撤掉的行在作废时刻**之前出现**（谓词没接上就永远是"现在"，回放照旧空手）
//! - `recorded_at` 晚于 T 的行在 T **不出现**（只写下界会让三月看见四月的修正）
//!
//! 自建自拆：一次性 org/workspace/kb，跑完连 org 一起删。绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

struct Fixture {
    kb: Uuid,
    zhang: Uuid,
    /// 03-01 记下、03-20 被修正掉的那条断言
    retracted: Uuid,
    /// 03-20 记下的修正行
    correction: Uuid,
    /// 03-05 推出、03-22 前提没了的派生
    derived: Uuid,
}

/// 一个人接过一个项目，我们三月改了主意，引擎在这中间推过一条边。
async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, project) = (Uuid::now_v7(), Uuid::now_v7());
    let (leads, part_of) = (Uuid::now_v7(), Uuid::now_v7());
    let (zhang, phoenix, program) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (retracted, correction, derived, rule) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'rewind-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'rewind-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'rewind-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (project, "project", "Project"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, key, label) in [(leads, "leads", "leads"), (part_of, "part_of", "part of")] {
        sqlx::query("INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, type_id, name) in [
        (zhang, person, "Zhang San"),
        (phoenix, project, "Project Phoenix"),
        (program, project, "Phoenix Program"),
    ] {
        // **实体的出生时刻也要回填**：事实记在三月，实体却是"刚才"建的，那种账本
        // 现实里不存在——而 #336 之后 T 时刻还没出生的实体不再出现在图上
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .bind(t("2026-01-01T00:00:00Z"))
        .execute(pool)
        .await?;
    }

    let fact = |id: Uuid, rec: &str, inv: Option<&str>, sup: Option<Uuid>| {
        let (rec, inv) = (t(rec), inv.map(t));
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                recorded_at, invalidated_at, supersedes)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(kb)
        .bind(zhang)
        .bind(leads)
        .bind(phoenix)
        .bind(rec)
        .bind(inv)
        .bind(sup)
    };
    fact(
        retracted,
        "2026-03-01T00:00:00Z",
        Some("2026-03-20T00:00:00Z"),
        None,
    )
    .execute(pool)
    .await?;
    fact(correction, "2026-03-20T00:00:00Z", None, Some(retracted))
        .execute(pool)
        .await?;

    // 派生也有两根轴（derived_at / invalidated_at）：回放的图上留着**当时**推出
    // 的边，而不是今天这套规则的结论
    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule)
    .bind(kb)
    .bind(part_of)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id,
                                    derived_at, invalidated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(derived)
    .bind(kb)
    .bind(phoenix)
    .bind(part_of)
    .bind(program)
    .bind(rule)
    .bind(t("2026-03-05T00:00:00Z"))
    .bind(t("2026-03-22T00:00:00Z"))
    .execute(pool)
    .await?;

    Ok(Fixture {
        kb,
        zhang,
        retracted,
        correction,
        derived,
    })
}

fn edge_ids(edges: &[utopia_core::models::GraphEdge]) -> Vec<Uuid> {
    let mut v: Vec<Uuid> = edges.iter().map(|e| e.id).collect();
    v.sort();
    v
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

#[tokio::test]
async fn the_recording_axis_rewinds_on_every_graph_read() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let overview = |as_of: Option<&'static str>| {
        let pool = pool.clone();
        async move { utopia_store::graph::overview(&pool, f.kb, 50, None, as_of.map(t)).await }
    };

    // 1. 现在：修正行在，被它顶掉的断言不在，前提没了的派生也不在
    let (nodes, edges, _, total_edges) = overview(None).await?;
    assert_eq!(
        edge_ids(&edges),
        vec![f.correction],
        "现在只该有修正行那条边"
    );
    assert_eq!(total_edges, 1, "边总数跟着画布数，不是历史全量");
    let degree = |nodes: &[utopia_core::models::GraphNode], id: Uuid| {
        nodes.iter().find(|n| n.id == id).map(|n| n.degree)
    };
    assert_eq!(degree(&nodes, f.zhang), Some(1));

    // 2. 倒回 03-10：作废前的断言回来了，三月才推出的派生也在，
    //    而 03-20 才记下的修正行**不该出现**——那是当时还没发生的事
    let (nodes, edges, _, total_edges) = overview(Some("2026-03-10T00:00:00Z")).await?;
    assert_eq!(
        edge_ids(&edges),
        sorted(vec![f.retracted, f.derived]),
        "03-10 该看见当时持有的断言与派生"
    );
    assert_eq!(total_edges, 2);
    assert_eq!(degree(&nodes, f.zhang), Some(1), "度数按当时的边数");
    assert!(
        edges
            .iter()
            .find(|e| e.id == f.derived)
            .is_some_and(|e| e.derived),
        "派生边要带 derived 位，界面靠它画出区别"
    );

    // 3. 倒回 03-25：修正行已经记下，派生已经被推翻
    let (_, edges, _, _) = overview(Some("2026-03-25T00:00:00Z")).await?;
    assert_eq!(edge_ids(&edges), vec![f.correction]);

    // 4. 更早于一切：图是空的，而不是"退化成现在"
    let (_, edges, _, total_edges) = overview(Some("2026-02-01T00:00:00Z")).await?;
    assert!(edges.is_empty(), "02-01 我们还什么都没记下");
    assert_eq!(total_edges, 0);

    // 5. 邻域铺开也按当时的边找邻居
    let (_, edges) = utopia_store::graph::neighborhood(
        &pool,
        f.kb,
        f.zhang,
        1,
        None,
        Some(t("2026-03-10T00:00:00Z")),
    )
    .await?;
    assert_eq!(edge_ids(&edges), vec![f.retracted]);
    let (_, edges) = utopia_store::graph::neighborhood(&pool, f.kb, f.zhang, 1, None, None).await?;
    assert_eq!(edge_ids(&edges), vec![f.correction]);

    // 6. 点开节点，面板说的是**当时**的事实——回放的图上点开一个节点，
    //    侧栏还答今天的事，两个说法就并排摆在同一个屏幕上
    let facts = |as_of: Option<&'static str>| {
        let pool = pool.clone();
        async move {
            utopia_store::graph::entity_detail(&pool, f.kb, f.zhang, None, as_of.map(t))
                .await
                .map(|(_, facts)| facts)
        }
    };
    let now = facts(None).await?;
    assert_eq!(now.len(), 1);
    assert_eq!(now[0].id, f.correction);
    assert!(now[0].corrected, "修正行认得出自己改写了谁");

    let march = facts(Some("2026-03-10T00:00:00Z")).await?;
    assert_eq!(march.len(), 1);
    assert_eq!(march[0].id, f.retracted, "03-10 面板该给出当时那条");

    let before = facts(Some("2026-02-01T00:00:00Z")).await?;
    assert!(before.is_empty(), "记下之前不该有事实");

    // 拆台：facts/entities/… 全是 ON DELETE CASCADE
    let gone = sqlx::query(
        "DELETE FROM organizations WHERE id = (
             SELECT w.org_id FROM workspaces w
             JOIN knowledge_bases k ON k.workspace_id = w.id WHERE k.id = $1)",
    )
    .bind(f.kb)
    .execute(&pool)
    .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
