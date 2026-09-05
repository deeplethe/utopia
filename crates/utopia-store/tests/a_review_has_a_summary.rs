//! 审核台的总览（#377），打在真库上。
//!
//! 三段都是 SQL 聚合，`cargo check` 一个字看不见；要钉住的是口径：
//!
//! - **等着办的**与左栏的计数同一套 WHERE，多出来的「最老」取的是各表自己的
//!   时钟（冲突是 created_at，事实队列是 recorded_at）
//! - **办过的**只认 review 域的四族动作，7 天窗口是 30 天窗口的子集，
//!   没有 actor 的算自裁，按天补齐到 14 条
//! - **成色**只数在世的事实；「有争议」是挂在开着的冲突任一端
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

struct Seed {
    kb: Uuid,
    org: Uuid,
    ada: Uuid,
    conflict_at: DateTime<Utc>,
    review_at: DateTime<Utc>,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Seed> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, rel) = (Uuid::now_v7(), Uuid::now_v7());
    let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (f1, f2, f3) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (ada, bo) = (Uuid::now_v7(), Uuid::now_v7());
    let now = Utc::now();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'review-summary-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'review-summary-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'review-summary-test')",
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
    sqlx::query("INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
        .bind(rel)
        .bind(kb)
        .bind(format!("knows_{}", Uuid::now_v7().simple()))
        .execute(pool)
        .await?;
    for (id, name) in [(a, "Ada"), (b, "Bo"), (c, "Cy")] {
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
    // 三条在世的事实：f2 低置信（0.5 < 0.75），f1 与 f3 之间挂一条开着的冲突
    for (id, s, o, conf) in [(f1, a, b, 0.9f32), (f2, a, c, 0.5), (f3, b, c, 0.9)] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(s)
        .bind(rel)
        .bind(o)
        .bind(conf)
        .execute(pool)
        .await?;
    }
    let conflict_at = now - Duration::days(3);
    sqlx::query(
        "INSERT INTO fact_conflicts (id, kb_id, old_fact_id, new_fact_id, reason, created_at)
         VALUES ($1, $2, $3, $4, 'simultaneous', $5)",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(f1)
    .bind(f3)
    .bind(conflict_at)
    .execute(pool)
    .await?;
    let review_at = now - Duration::days(5);
    sqlx::query(
        "INSERT INTO resolution_reviews (id, kb_id, left_id, right_id, score, created_at)
         VALUES ($1, $2, $3, $4, 0.8, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(a)
    .bind(b)
    .bind(review_at)
    .execute(pool)
    .await?;

    // 台账：Ada 两条在 7 天内，一条自裁在 7 天内，Bo 一条在 20 天前（30 天内），
    // Ada 还有一条 40 天前（窗口外）；再夹一条不是 review 域的动作，不该被数
    let events: [(Option<Uuid>, Option<&str>, &str, i64); 6] = [
        (Some(ada), Some("Ada"), "review.merge", 1),
        (Some(ada), Some("Ada"), "fact.confirm", 2),
        (None, None, "review.keep", 3),
        (Some(bo), Some("Bo"), "conflict.close_old", 20),
        (Some(ada), Some("Ada"), "fact.reject", 40),
        (Some(ada), Some("Ada"), "document.upload", 1),
    ];
    for (actor, label, action, days_ago) in events {
        sqlx::query(
            "INSERT INTO audit_events (id, kb_id, actor_id, actor_label, action, target_kind, created_at)
             VALUES ($1, $2, $3, $4, $5, 'test', $6)",
        )
        .bind(Uuid::now_v7())
        .bind(kb)
        .bind(actor)
        .bind(label)
        .bind(action)
        .bind(now - Duration::days(days_ago))
        .execute(pool)
        .await?;
    }
    Ok(Seed {
        kb,
        org,
        ada,
        conflict_at,
        review_at,
    })
}

async fn teardown(pool: &PgPool, s: &Seed) -> anyhow::Result<()> {
    // 台账只增不删（触发器挡着 DELETE），这几行留在库里，挂着一个已经不存在
    // 的 kb_id，谁也读不到；组织不级联到库，两个都要删
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(s.kb)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(s.org)
        .execute(pool)
        .await?;
    Ok(())
}

fn close(a: Option<DateTime<Utc>>, b: DateTime<Utc>) -> bool {
    a.is_some_and(|a| (a - b).num_seconds().abs() < 2)
}

#[tokio::test]
async fn the_summary_reads_the_three_ledgers_with_one_yardstick() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let s = seed(&pool).await?;

    let got = utopia_store::review_summary::summary(&pool, s.kb).await;
    let counts = utopia_store::review::counts(&pool, s.kb).await;
    teardown(&pool, &s).await?;
    let sum = got?;
    let counts = counts?;

    // 等着办的：与左栏同一套数，多了最老的一条从何时起等
    let w = &sum.waiting;
    assert_eq!(w.conflicts.count, 1);
    assert!(
        close(w.conflicts.oldest_at, s.conflict_at),
        "冲突按 created_at"
    );
    assert_eq!(w.duplicates.count, 1);
    assert!(
        close(w.duplicates.oldest_at, s.review_at),
        "重复按 created_at"
    );
    assert_eq!(w.lowconf.count, 1);
    assert!(w.lowconf.oldest_at.is_some(), "低置信按事实的 recorded_at");
    assert_eq!(w.unconfirmed.count, 0);
    assert_eq!(w.pending.count, 0);
    assert_eq!(w.pending.oldest_at, None, "空队列没有「最老」");
    assert_eq!(w.violations.count, 0);
    assert_eq!(w.defects.count, 0);
    assert_eq!(
        (
            w.conflicts.count,
            w.duplicates.count,
            w.lowconf.count,
            w.unconfirmed.count
        ),
        (
            counts.conflicts,
            counts.duplicates,
            counts.lowconf,
            counts.unconfirmed
        ),
        "总览与左栏计数是同一套口径"
    );

    // 成色：三条在世的事实，一条低置信，两条挂在开着的冲突上
    assert_eq!(sum.health.facts, 3);
    assert_eq!(sum.health.low_confidence, 1);
    assert_eq!(sum.health.contested, 2);
    assert_eq!(sum.health.unconfirmed, 0);

    // 办过的：7 天窗口三条（两条 Ada、一条自裁），30 天窗口再加 Bo 的一条；
    // 40 天前的与非 review 域的动作都不算
    let d7 = &sum.decided.last_7d;
    assert_eq!(d7.total, 3);
    assert_eq!(d7.automatic, 1);
    let by_action: Vec<(&str, i64)> = d7
        .by_action
        .iter()
        .map(|a| (a.action.as_str(), a.count))
        .collect();
    assert_eq!(
        by_action,
        vec![("fact.confirm", 1), ("review.keep", 1), ("review.merge", 1)],
        "同数按动作名排序"
    );
    assert_eq!(d7.by_actor[0].actor_id, Some(s.ada));
    assert_eq!(d7.by_actor[0].label.as_deref(), Some("Ada"));
    assert_eq!(d7.by_actor[0].count, 2);
    assert!(
        d7.by_actor
            .iter()
            .any(|a| a.actor_id.is_none() && a.count == 1),
        "自裁的那条按「没有人」单列"
    );
    let d30 = &sum.decided.last_30d;
    assert_eq!(d30.total, 4);
    assert_eq!(d30.automatic, 1);
    assert!(
        d30.by_actor
            .iter()
            .any(|a| a.label.as_deref() == Some("Bo") && a.count == 1),
        "20 天前的那条进 30 天窗口"
    );

    // 按天：固定 14 条、今天在最后、加起来就是近 14 天的三条
    let daily = &sum.decided.daily;
    assert_eq!(daily.len() as i64, utopia_store::review_summary::DAILY_DAYS);
    assert_eq!(daily.last().map(|d| d.day), Some(Utc::now().date_naive()));
    assert_eq!(daily.iter().map(|d| d.count).sum::<i64>(), 3);
    for pair in daily.windows(2) {
        assert_eq!(
            pair[1].day - pair[0].day,
            Duration::days(1),
            "一天一条，不跳"
        );
    }
    Ok(())
}
