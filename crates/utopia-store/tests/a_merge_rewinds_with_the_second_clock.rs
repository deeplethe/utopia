//! 合并也倒得回去（0019 第二刀 / #336），打在真库上。
//!
//! 合并是**原地改写**：`UPDATE facts SET subject_id = target`。行上只剩合并之后的
//! 样子，所以「三月那天这条事实挂在谁身上」既不在事实行里、也不在实体行里——
//! 只在 `entity_merges` 的数组里。这个测试打的就是那条映射。
//!
//! 三个时刻，因为它们各自会以不同的方式坏掉：
//! - **合并之前**：被并掉的实体要重新长出来，且带着它自己的那些事实
//! - **一次已撤销的合并的窗口之内**：那段时间它们**确实**是一个——撤销把行搬回去了，
//!   当前行里看不见这件事，只能从数组里读出来
//! - **现在**：一切照旧，且不能因为加了映射就变慢或变样

use sqlx::PgPool;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    /// 留下的那个
    zhang_a: Uuid,
    /// 四月被并进 A，至今仍并着
    zhang_b: Uuid,
    /// 五月被并进 A，六月又撤销了
    zhang_c: Uuid,
    fact_a: Uuid,
    fact_b: Uuid,
    fact_c: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, company) = (Uuid::now_v7(), Uuid::now_v7());
    let works_for = Uuid::now_v7();
    let (acme, zenith) = (Uuid::now_v7(), Uuid::now_v7());
    let (zhang_a, zhang_b, zhang_c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (fact_a, fact_b, fact_c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'merge-rewind-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'merge-rewind-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'merge-rewind-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (company, "company", "Company"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'works_for', 'works for')",
    )
    .bind(works_for)
    .bind(kb)
    .execute(pool)
    .await?;
    // 实体的出生时刻也倒着看：一月的图上它们还不存在
    for (id, type_id, name) in [
        (acme, company, "Acme"),
        (zenith, company, "Zenith"),
        (zhang_a, person, "Zhang Wei"),
        (zhang_b, person, "Zhang Wei"),
        (zhang_c, person, "Zhang Wei"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name, created_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .bind(t("2026-02-01T00:00:00Z"))
        .execute(pool)
        .await?;
    }
    for (id, subject, object, rec) in [
        (fact_a, zhang_a, acme, "2026-03-01T00:00:00Z"),
        (fact_b, zhang_b, zenith, "2026-03-02T00:00:00Z"),
        (fact_c, zhang_c, acme, "2026-03-03T00:00:00Z"),
    ] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence,
                                recorded_at)
             VALUES ($1, $2, $3, $4, $5, 0.9, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(subject)
        .bind(works_for)
        .bind(object)
        .bind(t(rec))
        .execute(pool)
        .await?;
    }

    // 四月：B 并进 A，至今仍并着
    utopia_store::resolution::merge_entities(pool, kb, zhang_b, zhang_a, None, "test").await?;
    backdate(pool, zhang_b, "2026-04-01T00:00:00Z", None).await?;

    // 五月：C 并进 A；六月撤销。**撤销把事实搬回去了**，所以现在的行上看不出
    // 五月到六月之间它们曾是一个——那段窗口只在 entity_merges 里
    let merge_c =
        utopia_store::resolution::merge_entities(pool, kb, zhang_c, zhang_a, None, "test").await?;
    backdate(pool, zhang_c, "2026-05-01T00:00:00Z", None).await?;
    utopia_store::resolution::revert_merge(pool, kb, merge_c).await?;
    backdate(
        pool,
        zhang_c,
        "2026-05-01T00:00:00Z",
        Some("2026-06-01T00:00:00Z"),
    )
    .await?;

    Ok(Fixture {
        org,
        kb,
        zhang_a,
        zhang_b,
        zhang_c,
        fact_a,
        fact_b,
        fact_c,
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

async fn facts_of(
    pool: &PgPool,
    kb: Uuid,
    entity: Uuid,
    as_of: Option<&str>,
) -> anyhow::Result<Vec<Uuid>> {
    let (_, facts) =
        utopia_store::graph::entity_detail(pool, kb, entity, None, as_of.map(t)).await?;
    let mut ids: Vec<Uuid> = facts.iter().map(|f| f.id).collect();
    ids.sort();
    Ok(ids)
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

#[tokio::test]
async fn a_merged_entity_comes_back_before_the_merge() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let nodes = |as_of: Option<&'static str>| {
        let pool = pool.clone();
        async move {
            let (nodes, _, total, _) =
                utopia_store::graph::overview(&pool, f.kb, 50, None, as_of.map(t)).await?;
            let mut ids: Vec<Uuid> = nodes.iter().map(|n| n.id).collect();
            ids.sort();
            Ok::<(Vec<Uuid>, i64), anyhow::Error>((ids, total))
        }
    };

    // 1. 现在：B 并着（不出现），C 已撤销（照常出现）
    let (ids, total) = nodes(None).await?;
    assert!(!ids.contains(&f.zhang_b), "仍并着的实体不该出现在画布上");
    assert!(ids.contains(&f.zhang_c), "撤销过的合并不该继续吞掉那个实体");
    assert_eq!(total, 4, "节点总数与画布同一口径");
    assert_eq!(
        facts_of(&pool, f.kb, f.zhang_a, None).await?,
        sorted(vec![f.fact_a, f.fact_b])
    );
    assert_eq!(
        facts_of(&pool, f.kb, f.zhang_c, None).await?,
        vec![f.fact_c]
    );

    // 2. 三月：三个张伟各自站着，各自带着自己的那条事实。
    //    这是这一刀的全部意义——事实行上写的是 A，而三月那天它挂在 B 身上
    let (ids, total) = nodes(Some("2026-03-15T00:00:00Z")).await?;
    assert!(ids.contains(&f.zhang_b) && ids.contains(&f.zhang_c));
    assert_eq!(total, 5);
    assert_eq!(
        facts_of(&pool, f.kb, f.zhang_a, Some("2026-03-15T00:00:00Z")).await?,
        vec![f.fact_a],
        "三月的 A 只有自己那一条"
    );
    assert_eq!(
        facts_of(&pool, f.kb, f.zhang_b, Some("2026-03-15T00:00:00Z")).await?,
        vec![f.fact_b],
        "被并掉的 B 在三月还拿着自己那条事实"
    );

    // 3. 五月中：C 那次合并当时**生效着**（六月才撤销）。撤销已经把行搬回 C，
    //    所以这一格只能从 entity_merges 的数组里读出来
    let (ids, total) = nodes(Some("2026-05-15T00:00:00Z")).await?;
    assert!(!ids.contains(&f.zhang_c), "五月中 C 正并在 A 里");
    assert!(!ids.contains(&f.zhang_b));
    assert_eq!(total, 3);
    assert_eq!(
        facts_of(&pool, f.kb, f.zhang_a, Some("2026-05-15T00:00:00Z")).await?,
        sorted(vec![f.fact_a, f.fact_b, f.fact_c]),
        "五月中三条事实都在 A 身上"
    );

    // 4. 一月：实体还没被建出来，图是空的，而不是退化成现在
    let (ids, total) = nodes(Some("2026-01-01T00:00:00Z")).await?;
    assert!(ids.is_empty(), "一月这些实体还不存在");
    assert_eq!(total, 0);

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    let gone = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
