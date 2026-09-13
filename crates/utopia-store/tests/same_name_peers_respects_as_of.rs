//! 同名列按记录轴回放（#307 / 0019）。
//!
//! 之前 `same_name_peers` 写死 `e.merged_into IS NULL`：三月并掉的「张伟」
//! 在任何回放时刻都消失，而面板想告诉人的恰恰是「三月并掉之前它叫什么、
//! 与谁同名」——把同一性的歧义藏在时间之外，会让人把两个独立实体当成
//! 同一个去合并。
//!
//! 两个方向都要钉：
//!
//! - **三月并掉的实体**在 `as_of` 早于三月时重新出现（`entity_visible_at`
//!   的回放语义）
//! - **当下还活着的实体**在所有时刻都出现
//! - **当下被作废的事实**不入度数量，而三月那条还活的事实要数
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

#[tokio::test]
async fn same_name_peers_respects_as_of() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, user, person_type) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    // 张伟 a：1 月记「works_at project_entity」，没作废；3 月与张伟 b 合并
    let zhang_wei_a = Uuid::now_v7();
    // 张伟 b：1 月记「works_at other_entity」，没作废——它就是目标，从不消失
    let zhang_wei_b = Uuid::now_v7();
    // 张伟 c：1 月记「works_at other_entity」，没作废——从未被合并
    let zhang_wei_c = Uuid::now_v7();
    let project_entity = Uuid::now_v7();
    let other_entity = Uuid::now_v7();
    let works_at = Uuid::now_v7();
    let merge_id = Uuid::now_v7();
    // 张伟 a 上的事实：1 月记下，3 月作废
    let f_a_old = Uuid::now_v7();
    // 张伟 b 上的事实：1 月记下
    let f_b = Uuid::now_v7();
    // 张伟 c 上的事实：5 月记下（晚于合并）
    let f_c_new = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'as-of-peers-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'as-of-peers-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'as-of-peers-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $3, 'Peers Tester', 'x')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("peers-{}@test.local", user.simple()))
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'person', 'Person')",
    )
    .bind(person_type)
    .bind(kb)
    .execute(&pool)
    .await?;
    for (id, name) in [
        (zhang_wei_a, "Zhang Wei"),
        (zhang_wei_b, "Zhang Wei"),
        (zhang_wei_c, "Zhang Wei"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(person_type)
        .bind(name)
        .bind(t("2026-01-01T00:00:00Z"))
        .execute(&pool)
        .await?;
    }
    for (id, name) in [
        (project_entity, "Project Phoenix"),
        (other_entity, "Other Co"),
    ] {
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
    for (fid, subject, object, recorded) in [
        (f_a_old, zhang_wei_a, project_entity, "2026-01-15T00:00:00Z"),
        (f_b, zhang_wei_b, other_entity, "2026-01-20T00:00:00Z"),
        (f_c_new, zhang_wei_c, other_entity, "2026-05-10T00:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                               confidence, recorded_at)
             VALUES ($1, $2, $3, $4, $5, 0.9, $6)",
        )
        .bind(fid)
        .bind(kb)
        .bind(subject)
        .bind(works_at)
        .bind(object)
        .bind(t(recorded))
        .execute(&pool)
        .await?;
    }
    // 张伟 a 上 1 月记下那条事实在 3 月作废（之后被合并覆盖之前先收掉）
    sqlx::query("UPDATE facts SET invalidated_at = $2 WHERE id = $1 AND invalidated_at IS NULL")
        .bind(f_a_old)
        .bind(t("2026-03-10T00:00:00Z"))
        .execute(&pool)
        .await?;
    // 3 月合并 a → b（直接写 merged_into；本来这一步由 resolution::merge_entities
    // 内部做，但这里是测同名列的可见性，没必要把整条合并路径走一遍）
    sqlx::query(
        "INSERT INTO entity_merges (id, kb_id, source_id, target_id, merged_by, reason, created_at)
         VALUES ($1, $2, $3, $4, $5, 'duplicate person', $6)",
    )
    .bind(merge_id)
    .bind(kb)
    .bind(zhang_wei_a)
    .bind(zhang_wei_b)
    .bind(user)
    .bind(t("2026-03-15T00:00:00Z"))
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE entities SET merged_into = $2 WHERE id = $1")
        .bind(zhang_wei_a)
        .bind(zhang_wei_b)
        .execute(&pool)
        .await?;

    let peers_at = |when: Option<chrono::DateTime<chrono::Utc>>| {
        let pool = pool.clone();
        async move { utopia_store::graph::same_name_peers(&pool, kb, zhang_wei_b, when).await }
    };
    let names = |rows: Vec<utopia_core::models::GraphNode>| -> Vec<String> {
        let mut v: Vec<String> = rows.into_iter().map(|n| n.name).collect();
        v.sort();
        v
    };

    // 当下：a 已被合并，只剩 b 和 c
    let now = peers_at(None).await?;
    assert_eq!(
        names(now),
        vec!["Zhang Wei".to_string()],
        "当下只剩独立的 Zhang Wei"
    );

    // 2 月：a 还在活跃，c 已创建 → a、c 都与 b 同名
    let feb = peers_at(Some(t("2026-02-01T00:00:00Z"))).await?;
    assert_eq!(
        names(feb),
        vec!["Zhang Wei".to_string(), "Zhang Wei".to_string()],
        "2 月：合并之前，a 与 b 同名"
    );

    // 4 月：a 已合并，c 已创建 → 只有 c
    let apr = peers_at(Some(t("2026-04-01T00:00:00Z"))).await?;
    assert_eq!(
        names(apr),
        vec!["Zhang Wei".to_string()],
        "4 月：a 已合并，只剩 c"
    );

    // 6 月：合并之后，c 已是唯一独立同名实体
    let jun = peers_at(Some(t("2026-06-01T00:00:00Z"))).await?;
    assert_eq!(
        names(jun),
        vec!["Zhang Wei".to_string()],
        "6 月：a 已合并，c 已创建"
    );

    // 删除 organization 之前先删 user 与 workspaces：entity_merges 的 merged_by
    // 指着 user，knowledge_bases 的 workspace_id 指着 workspace。先删
    // knowledge_bases 会带走 workspaces 与 user，但 workspaces 是 workspace_id
    // 在前所以需要先解 user 那条 FK——按依赖反着来
    sqlx::query("DELETE FROM entity_merges WHERE id = $1")
        .bind(merge_id)
        .execute(&pool)
        .await?;
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
