//! 合并本身也进这条认识轴（#337 的时钟接进 `entity_history`），打在真库上。
//!
//! 从前 History 只看得见合并**顺手作废的那些事实**（`merged`），看不见合并这件事
//! 本身：打开一个被并过的实体，界面在说结果，不说发生了什么。而合并是这条轴上
//! 最大的一次认识改变——从此它和另一个实体算同一个东西。
//!
//! 三件事各自会以不同的方式坏掉，所以分开断言：
//! - **方向**：吸收了谁（`merged_in`）与被谁吸收（`merged_away`）在图上是两件事，
//!   回滚也是按方向做的；同一句话说两头，读的人分不出谁并进了谁
//! - **撤销的归因**：撤的人常常不是当初合的人，自动合并那一行的 `merged_by` 还是
//!   NULL——所以撤销那一条要从审计里取人，不能借合并行的
//! - **总数**：分页的总数漏掉这一支，翻到最后一页会少几条

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    /// 留下的那个
    keeper: Uuid,
    /// 四月被并进 keeper，至今仍并着
    absorbed: Uuid,
    /// 五月被并进 keeper，六月撤销
    returned: Uuid,
    /// 撤销那一下的人
    reverter: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let person = Uuid::now_v7();
    let (keeper, absorbed, returned) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let reverter = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'history-merge-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'history-merge-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'history-merge-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $3, 'Reverter', 'x')",
    )
    .bind(reverter)
    .bind(org)
    .bind(format!("reverter-{}@test.local", reverter.simple()))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'person', 'Person')",
    )
    .bind(person)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [
        (keeper, "Zhang Wei"),
        (absorbed, "Zhang Wei"),
        (returned, "Zhang Wei"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(person)
        .bind(name)
        .bind(t("2026-02-01T00:00:00Z"))
        .execute(pool)
        .await?;
    }

    // 四月：absorbed 并进 keeper，自动裁决（merged_by 为空）
    utopia_store::resolution::merge_entities(pool, kb, absorbed, keeper, None, "test").await?;
    backdate(pool, absorbed, "2026-04-01T00:00:00Z", None).await?;

    // 五月：returned 并进 keeper；六月由人撤销
    let merge_id =
        utopia_store::resolution::merge_entities(pool, kb, returned, keeper, None, "test").await?;
    utopia_store::resolution::revert_merge(pool, kb, merge_id).await?;
    backdate(
        pool,
        returned,
        "2026-05-01T00:00:00Z",
        Some("2026-06-01T00:00:00Z"),
    )
    .await?;
    // 撤销的人记在审计里（`merge.revert`），history 从那儿取归因
    utopia_store::audit::record(
        pool,
        Some(kb),
        reverter,
        "merge.revert",
        "merge",
        Some(merge_id),
        serde_json::json!({}),
    )
    .await?;

    Ok(Fixture {
        org,
        kb,
        keeper,
        absorbed,
        returned,
        reverter,
    })
}

/// 合并的时刻由 `now()` 落库，测试要的是确定的日期。
async fn backdate(
    pool: &PgPool,
    source: Uuid,
    created: &str,
    reverted: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE entity_merges SET created_at = $2, reverted_at = $3 WHERE source_id = $1")
        .bind(source)
        .bind(t(created))
        .bind(reverted.map(t))
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn history_carries_the_merge_and_its_direction() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    // 收编的那一头：两次并入 + 一次撤销
    let (events, total) = utopia_store::graph::entity_history(&pool, f.kb, f.keeper, 50, 0).await?;
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == "merged_in").count(),
        2,
        "两次都是别人并进来：{kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| **k == "merge_reverted").count(),
        1,
        "六月那次撤销要在：{kinds:?}"
    );
    assert!(
        !kinds.contains(&"merged_away"),
        "keeper 从没被并走过：{kinds:?}"
    );
    assert_eq!(total, events.len() as i64, "总数漏了合并这一支");

    // 对方的名字要在，否则界面上只剩一句「有实体并了进来」
    assert!(
        events
            .iter()
            .filter(|e| e.kind == "merged_in")
            .all(|e| e.other_name.as_deref() == Some("Zhang Wei")),
        "并入事件要带对方实体的名字"
    );

    // 归因：合并是引擎自动裁的（merged_by 为空），撤销是人做的
    let merged_in = events.iter().find(|e| e.kind == "merged_in").unwrap();
    assert!(
        merged_in.actor_name.is_none(),
        "自动合并没有人，归因该是空（界面显示为引擎）"
    );
    let reverted = events.iter().find(|e| e.kind == "merge_reverted").unwrap();
    assert_eq!(
        reverted.actor_name.as_deref(),
        Some("Reverter"),
        "撤销的人从审计里取，而不是借合并行的 merged_by"
    );

    // 被并走的那一头：方向相反，而且看得见自己是四月没的
    let (events, _) = utopia_store::graph::entity_history(&pool, f.kb, f.absorbed, 50, 0).await?;
    let away: Vec<_> = events.iter().filter(|e| e.kind == "merged_away").collect();
    assert_eq!(away.len(), 1, "它并进了别人，只有这一次");
    assert_eq!(away[0].at, t("2026-04-01T00:00:00Z"));
    assert!(
        !events.iter().any(|e| e.kind == "merged_in"),
        "它没吸收过谁"
    );

    // 撤销过的那一头：并出去、又回来，两条都在
    let (events, _) = utopia_store::graph::entity_history(&pool, f.kb, f.returned, 50, 0).await?;
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert!(
        kinds.contains(&"merged_away") && kinds.contains(&"merge_reverted"),
        "并出去与撤销都要在：{kinds:?}"
    );

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(f.reverter)
        .execute(&pool)
        .await?;
    let gone = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
