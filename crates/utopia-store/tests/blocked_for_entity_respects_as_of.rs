//! 面板的「没落地的派生」按记录轴回放（#307 / 0019）。
//!
//! 之前 `blocked_for_entity` 写死 `v.status = 'open'`：三月被发现、四月被人
//! 裁掉的违规，在回放中的面板上仍然挂着——而该行此刻在 `axiom_violations`
//! 上已经是 `resolved`。`violation_open_at` 是答案：和 0031 同款的「判定
//! 在 T 时刻还开着」问法。
//!
//! 三个方向都要钉：
//!
//! - **当下还开着的** 在所有时刻都出现（`status = 'open'` 与 held_at 同真）
//! - **当下被裁的** 在 `decided_at` 之前仍出现，之后消失
//! - **当下还没检测到的** 在 `detected_at` 之前不出现
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

#[tokio::test]
async fn blocked_for_entity_respects_as_of() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, user, person_type, works_at) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let mira = Uuid::now_v7();
    let acme = Uuid::now_v7();
    let ceo_of = Uuid::now_v7();
    let ceo_fact_a = Uuid::now_v7();
    let ceo_fact_b = Uuid::now_v7();
    let works_at_fact = Uuid::now_v7();
    let still_open_violation = Uuid::now_v7();
    let resolved_violation = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'as-of-blocked-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'as-of-blocked-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'as-of-blocked-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $3, 'Blocked Tester', 'x')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("blocked-{}@test.local", user.simple()))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'person', 'Person')",
    )
    .bind(person_type)
    .bind(kb)
    .execute(&pool)
    .await?;
    for (id, name) in [(mira, "Mira"), (acme, "Acme")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, canonical_name, created_at)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .bind(t("2026-01-01T00:00:00Z"))
        .execute(&pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'works_at', 'works at')",
    )
    .bind(works_at)
    .bind(kb)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'ceo_of', 'ceo of')",
    )
    .bind(ceo_of)
    .bind(kb)
    .execute(&pool)
    .await?;
    // Mira 上的断言事实：被两条违规拿来当 left_fact
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(works_at_fact)
    .bind(kb)
    .bind(mira)
    .bind(works_at)
    .bind(acme)
    .execute(&pool)
    .await?;
    // 派生事实：被两条违规拿来当 right_fact（得是事实才能填 FK）
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(ceo_fact_a)
    .bind(kb)
    .bind(mira)
    .bind(ceo_of)
    .bind(acme)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(ceo_fact_b)
    .bind(kb)
    .bind(mira)
    .bind(ceo_of)
    .bind(acme)
    .execute(&pool)
    .await?;
    // 「现在还开着」：3 月检测至今未裁
    sqlx::query(
        "INSERT INTO axiom_violations (id, kb_id, kind, left_fact, right_fact, status,
                                      detail, detected_at)
         VALUES ($1, $2, 'derived_contradiction', $3, $4, 'open',
                 jsonb_build_object('subject_id', $5::text, 'object_id', $6::text,
                                    'predicate', 'works at'),
                 $7)",
    )
    .bind(still_open_violation)
    .bind(kb)
    .bind(works_at_fact)
    .bind(ceo_fact_a)
    .bind(mira)
    .bind(acme)
    .bind(t("2026-03-15T00:00:00Z"))
    .execute(&pool)
    .await?;
    // 「三月检测、四月被人裁掉」：当前 status=resolved，decided_at=4 月
    sqlx::query(
        "INSERT INTO axiom_violations (id, kb_id, kind, left_fact, right_fact, status,
                                      resolution, decided_at,
                                      detail, detected_at)
         VALUES ($1, $2, 'derived_contradiction', $3, $4, 'resolved',
                 'fact_retracted', $7,
                 jsonb_build_object('subject_id', $5::text, 'object_id', $6::text,
                                    'predicate', 'works at'),
                 $8)",
    )
    .bind(resolved_violation)
    .bind(kb)
    .bind(works_at_fact)
    .bind(ceo_fact_b)
    .bind(mira)
    .bind(acme)
    .bind(t("2026-04-15T00:00:00Z"))
    .bind(t("2026-03-20T00:00:00Z"))
    .execute(&pool)
    .await?;

    let ids = |rows: Vec<utopia_core::models::BlockedDerivation>| -> Vec<Uuid> {
        rows.into_iter().map(|r| r.violation_id).collect()
    };
    let ids_sorted = |rows: Vec<utopia_core::models::BlockedDerivation>| -> Vec<Uuid> {
        let mut v = ids(rows);
        v.sort();
        v
    };

    let blocks = |when: Option<chrono::DateTime<chrono::Utc>>| {
        let pool = pool.clone();
        async move { utopia_store::reasoning::blocked_for_entity(&pool, kb, mira, when).await }
    };

    // 当下：resolved 那条已裁，open 那条留着 → 只有 still_open
    let now = blocks(None).await?;
    assert_eq!(
        ids_sorted(now),
        vec![still_open_violation],
        "当下：open 一条"
    );

    // 2 月：两条都还没检测到 → 都没有
    let feb = blocks(Some(t("2026-02-01T00:00:00Z"))).await?;
    assert!(feb.is_empty(), "2 月：检测之前都不到");

    // 3 月 25 日：resolved 在 decided_at 之前仍开着；still_open 在 → 两条
    let late_mar = blocks(Some(t("2026-03-25T00:00:00Z"))).await?;
    assert_eq!(
        ids_sorted(late_mar),
        vec![still_open_violation, resolved_violation],
        "3 月 25 日：resolved 还在、still_open 还在"
    );

    // 5 月：resolved 在 decided_at 之后消失、still_open 还在 → 一条
    let may = blocks(Some(t("2026-05-01T00:00:00Z"))).await?;
    assert_eq!(
        ids_sorted(may),
        vec![still_open_violation],
        "5 月：resolved 已裁、still_open 还在"
    );

    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
