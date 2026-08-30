//! `graph_changes` 的账本推导，打在真库上。
//!
//! 为什么非要连库：这段逻辑**整个活在 SQL 字符串里**，`cargo check` 和 clippy
//! 一个字都看不见。本项目已经在这上面栽过好几次（合并去重漏掉 object_value、
//! `UPDATE … RETURNING` 返回新值、CHECK 约束悄悄拒绝一种 kind），
//! 每次都是运行时才发现。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时**跳过而不是失败**：这是本仓库第一个连库测试，
//! 不该让没起数据库的人 `cargo test` 变红。
//!
//! 自建自拆：造一个一次性 org/workspace/kb，跑完连 kb 一起删（facts/entities
//! 都是 ON DELETE CASCADE）。绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

/// 造一个最小账本，返回 (kb_id, 主语实体, 宾语实体)。
///
/// 四条事实覆盖全部四种事件，外加一条**不该出现的**事件：
/// - A 03-10 写入，03-20 作废，但 B 接了它 → 只出 asserted，作废不重复记
/// - B 03-20 写入且 supersedes=A → corrected
/// - C 03-12 写入，03-25 作废且无后继 → asserted + rejected
/// - D 03-13 写入，03-26 作废且无后继，带 merged 采纳记录 → asserted + merged
async fn seed(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let org = Uuid::now_v7();
    let ws = Uuid::now_v7();
    let kb = Uuid::now_v7();
    let etype = Uuid::now_v7();
    let pred = Uuid::now_v7();
    let (subj, obj, other) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (a, b, c, d) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'changes-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'changes-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'changes-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'located_in', 'located in')",
    )
    .bind(pred)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(subj, "Acme"), (obj, "Berlin"), (other, "Paris")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(etype)
        .bind(name)
        .execute(pool)
        .await?;
    }

    let fact = |id: Uuid, o: Uuid, rec: &str, inv: Option<&str>, sup: Option<Uuid>| {
        let (rec, inv) = (t(rec), inv.map(t));
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                recorded_at, invalidated_at, supersedes)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(kb)
        .bind(subj)
        .bind(pred)
        .bind(o)
        .bind(rec)
        .bind(inv)
        .bind(sup)
    };
    fact(
        a,
        obj,
        "2026-03-10T00:00:00Z",
        Some("2026-03-20T00:00:00Z"),
        None,
    )
    .execute(pool)
    .await?;
    fact(b, obj, "2026-03-20T00:00:00Z", None, Some(a))
        .execute(pool)
        .await?;
    fact(
        c,
        obj,
        "2026-03-12T00:00:00Z",
        Some("2026-03-25T00:00:00Z"),
        None,
    )
    .execute(pool)
    .await?;
    // D 的宾语换成 other：用来验证 entity_id 过滤认宾语侧
    fact(
        d,
        other,
        "2026-03-13T00:00:00Z",
        Some("2026-03-26T00:00:00Z"),
        None,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO fact_adoptions (batch_id, kb_id, predicate_id, old_fact_id, new_fact_id, mode)
         VALUES ($1, $2, $3, $4, $5, 'merged')",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(pred)
    .bind(d)
    .bind(b)
    .execute(pool)
    .await?;

    Ok((kb, subj, other))
}

/// (kind, 日期) 的多重集，排序后好比对
fn shape(rows: &[utopia_core::models::GraphChange]) -> Vec<String> {
    let mut v: Vec<String> = rows
        .iter()
        .map(|c| format!("{} {}", c.at.format("%m-%d"), c.kind))
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn ledger_events_are_derived_as_specified() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (kb, subj, other) = seed(&pool).await?;

    let run = |since: &'static str,
               until: &'static str,
               entity: Option<Uuid>,
               kinds: Option<Vec<String>>| {
        let pool = pool.clone();
        async move {
            utopia_store::graph::graph_changes(
                &pool,
                kb,
                t(since),
                t(until),
                entity,
                kinds.as_deref(),
                100,
            )
            .await
        }
    };

    // 1. 整窗：四条事实产出六个事件，且 A 的死亡**不出现**——
    //    它已经被 B 那条 corrected 解释过了，再记一次就是同一件事说两遍
    let all = run("2026-03-01T00:00:00Z", "2026-04-01T00:00:00Z", None, None).await?;
    assert_eq!(
        shape(&all),
        vec![
            "03-10 asserted",
            "03-12 asserted",
            "03-13 asserted",
            "03-20 corrected",
            "03-25 rejected",
            "03-26 merged",
        ],
        "四种事件各就各位，且 03-20 没有 rejected"
    );

    // 2. 两个分支各按**自己那根时间列**开窗：截到 03-15，写入进得来、作废进不来
    let early = run("2026-03-01T00:00:00Z", "2026-03-15T00:00:00Z", None, None).await?;
    assert_eq!(
        shape(&early),
        vec!["03-10 asserted", "03-12 asserted", "03-13 asserted"]
    );

    // 3. kinds 过滤（text[] 绑定）
    let only = run(
        "2026-03-01T00:00:00Z",
        "2026-04-01T00:00:00Z",
        None,
        Some(vec!["rejected".into(), "merged".into()]),
    )
    .await?;
    assert_eq!(shape(&only), vec!["03-25 rejected", "03-26 merged"]);

    // 4. entity_id 过滤（uuid 绑定）认宾语侧：other 只在 D 上当宾语，
    //    却要能捞出 D 的两个事件
    let by_object = run(
        "2026-03-01T00:00:00Z",
        "2026-04-01T00:00:00Z",
        Some(other),
        None,
    )
    .await?;
    assert_eq!(shape(&by_object), vec!["03-13 asserted", "03-26 merged"]);

    // 5. 主语侧命中全部六条
    let by_subject = run(
        "2026-03-01T00:00:00Z",
        "2026-04-01T00:00:00Z",
        Some(subj),
        None,
    )
    .await?;
    assert_eq!(by_subject.len(), 6);

    // 6. 主宾与谓词都拼了出来，不是一堆 uuid
    let sample = all.iter().find(|c| c.kind == "corrected").unwrap();
    assert_eq!(sample.subject_name, "Acme");
    assert_eq!(sample.predicate_label.as_deref(), Some("located in"));
    assert_eq!(sample.object_name.as_deref(), Some("Berlin"));

    // 拆台：facts/entities/… 全是 ON DELETE CASCADE
    let gone = sqlx::query(
        "DELETE FROM organizations WHERE id = (
             SELECT w.org_id FROM workspaces w
             JOIN knowledge_bases k ON k.workspace_id = w.id WHERE k.id = $1)",
    )
    .bind(kb)
    .execute(&pool)
    .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
