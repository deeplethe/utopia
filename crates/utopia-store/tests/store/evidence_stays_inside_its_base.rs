//! 证据只在事实自己的库里读：别库事实的 id 拿到本库路径下问，答 NotFound，
//! 不吐出别库的原句。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn another_bases_fact_is_not_found() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc, chunk, ent, fact) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'evidence-test');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'evidence-test');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{a}', '{ws}', 'a'), ('{b}', '{ws}', 'b');
         INSERT INTO documents (id, kb_id, filename, sha256, status, external_key)
           VALUES ('{doc}', '{b}', 'b.md', '{doc}', 'ready', 'file:///b.md');
         INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ('{chunk}', '{b}', '{doc}', 0, 'x');
         INSERT INTO entities (id, kb_id, canonical_name) VALUES ('{ent}', '{b}', 'e');
         INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
           VALUES ('{fact}', '{b}', '{ent}', '{ent}', 0.9);
         INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
           VALUES ('{fact}', '{chunk}', 'b only', '{doc}', 1);"
    ))
    .execute(&pool)
    .await?;

    let from_a = utopia_store::graph::fact_evidence(&pool, a, fact).await;
    let from_b = utopia_store::graph::fact_evidence(&pool, b, fact).await?;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = ANY($1)")
        .bind([a, b])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    assert!(matches!(from_a, Err(utopia_core::AppError::NotFound)));
    assert_eq!(from_b.len(), 1);
    Ok(())
}
