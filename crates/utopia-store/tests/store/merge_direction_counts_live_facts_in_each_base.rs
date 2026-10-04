//! 合并方向只看有效事实；知识库限定不能改变度数和 UUID 平局规则。

use sqlx::PgPool;
use utopia_store::resolution::merge_direction;
use uuid::Uuid;

async fn fact(
    pool: &PgPool,
    kb: Uuid,
    predicate: Uuid,
    subject: Uuid,
    object: Uuid,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(kb)
    .bind(subject)
    .bind(predicate)
    .bind(object)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn assert_direction(
    pool: &PgPool,
    survivor: Uuid,
    merged: Uuid,
    reason: &str,
) -> anyhow::Result<()> {
    for (left, right) in [(survivor, merged), (merged, survivor)] {
        assert_eq!(
            merge_direction(pool, left, right).await?,
            (survivor, merged),
            "{reason}; input order: {left}, {right}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn merge_direction_counts_live_facts_in_each_base() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, workspace) = (Uuid::now_v7(), Uuid::now_v7());
    let (kb, other_kb) = (Uuid::now_v7(), Uuid::now_v7());
    let (predicate, other_predicate) = (Uuid::now_v7(), Uuid::now_v7());
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
    let (older, newer) = (first.min(second), first.max(second));
    let (peer, other, other_peer) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'merge-degree-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'merge-degree-test')")
        .bind(workspace)
        .bind(org)
        .execute(&pool)
        .await?;
    for (base, relation) in [(kb, predicate), (other_kb, other_predicate)] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name)
             VALUES ($1, $2, 'merge-degree-test')",
        )
        .bind(base)
        .bind(workspace)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label)
             VALUES ($1, $2, 'related_to', 'related to')",
        )
        .bind(relation)
        .bind(base)
        .execute(&pool)
        .await?;
    }
    for (id, base, name) in [
        (older, kb, "Older"),
        (newer, kb, "Newer"),
        (peer, kb, "Peer"),
        (other, other_kb, "Other"),
        (other_peer, other_kb, "Other peer"),
    ] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(base)
            .bind(name)
            .execute(&pool)
            .await?;
    }

    assert_direction(&pool, older, newer, "zero degrees use the smaller UUID").await?;

    fact(&pool, kb, predicate, older, older).await?;
    fact(&pool, kb, predicate, peer, newer).await?;
    let outgoing = fact(&pool, kb, predicate, newer, peer).await?;
    // older 的自关联只算一条；若拆成两次计数相加，2:2 会错误地留下 older。
    assert_direction(
        &pool,
        newer,
        older,
        "incoming and outgoing facts beat one self-reference",
    )
    .await?;

    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(outgoing)
        .execute(&pool)
        .await?;
    assert_direction(
        &pool,
        older,
        newer,
        "invalidated facts do not break the one-to-one degree tie",
    )
    .await?;

    fact(&pool, kb, predicate, older, peer).await?;
    assert_direction(&pool, older, newer, "the larger live degree survives").await?;

    fact(&pool, other_kb, other_predicate, other, other).await?;
    fact(&pool, other_kb, other_predicate, other, other_peer).await?;
    fact(&pool, other_kb, other_predicate, other_peer, other).await?;
    assert_direction(
        &pool,
        older,
        newer,
        "facts in another knowledge base leave the result unchanged",
    )
    .await?;
    assert_direction(
        &pool,
        other,
        older,
        "each entity's degree uses its own knowledge base",
    )
    .await?;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = ANY($1)")
        .bind(vec![kb, other_kb])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
