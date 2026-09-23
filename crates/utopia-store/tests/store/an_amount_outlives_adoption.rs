//! 谓词还没被采纳时，金额已经落在事实上；采纳把属性搬到新行、并在关系上补声明（0037）。
//!
//! 一句「NVIDIA invested $1.5 billion in SB Energy」在 schema.org 库里 `invested_in`
//! 是未知说法，钱不能等到采纳那天才有地方放。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn a_qualifier_written_before_adoption_survives_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let tag = Uuid::now_v7();
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(format!("outlive-{tag}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(format!("outlive-{tag}"))
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(format!("outlive-{tag}"))
        .execute(&pool)
        .await?;
    let class = Uuid::now_v7();
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'org', 'Org')")
        .bind(class)
        .bind(kb)
        .execute(&pool)
        .await?;
    let amount = utopia_store::ontology::create_relation_type(
        &pool,
        kb,
        "amount",
        "amount",
        "state",
        Default::default(),
        "",
        "attribute",
        &[class],
        &[],
        Some("number"),
        Some("$"),
    )
    .await?;
    // 关系此刻还不存在于本体里——先建好但不声明任何属性，模拟"采纳"那一刻
    let invested = utopia_store::ontology::create_relation_type(
        &pool,
        kb,
        "invested_in",
        "invested in",
        "event",
        Default::default(),
        "",
        "relation",
        &[],
        &[],
        None,
        None,
    )
    .await?;
    let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, name) in [(a, "NVIDIA"), (b, "SB Energy")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(class)
        .bind(format!("{name}-{tag}"))
        .execute(&pool)
        .await?;
    }
    let (doc, chunk, fact) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1, $2, $3, $4)")
        .bind(doc)
        .bind(kb)
        .bind(format!("{tag}.txt"))
        .bind(tag.to_string())
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'x')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(&pool)
    .await?;
    // 谓词为空的事实 + 说法在证据上 + 金额已经在边上
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, NULL, $4, 0.9)",
    )
    .bind(fact)
    .bind(kb)
    .bind(a)
    .bind(b)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, document_id, proposed_predicate)
         VALUES ($1, $2, $3, 'invested_in')",
    )
    .bind(fact)
    .bind(chunk)
    .bind(doc)
    .execute(&pool)
    .await?;
    let value = serde_json::json!({ "value": 1500000000.0, "unit": "$" });
    utopia_store::graph::upsert_fact_qualifier(&pool, fact, amount, &value).await?;

    // 采纳：旧行作废、新行接上谓词
    let adopted = utopia_store::graph::adopt_proposed_predicates(
        &pool,
        kb,
        invested,
        &["invested_in".to_string()],
        false,
    )
    .await?;
    assert_eq!(adopted.moved, 1);

    // 活着的那一行带着金额
    let live: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM facts WHERE kb_id = $1 AND predicate_id = $2 AND invalidated_at IS NULL",
    )
    .bind(kb)
    .bind(invested)
    .fetch_all(&pool)
    .await?;
    assert_eq!(live.len(), 1, "采纳后正好一条活着的边");
    let quals = utopia_store::graph::fact_qualifiers_for(&pool, &[live[0].0]).await?;
    let q = &quals[&live[0].0];
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].key, "amount");
    assert_eq!(q[0].value, Some(value), "金额跟着搬到了新行");

    // 关系上补了声明
    let declared = utopia_store::graph::relation_types(&pool, kb)
        .await?
        .into_iter()
        .find(|r| r.id == invested)
        .map(|r| r.qualifiers)
        .unwrap_or_default();
    assert_eq!(declared, vec![amount], "采纳时按边上已有的属性补了声明");

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
