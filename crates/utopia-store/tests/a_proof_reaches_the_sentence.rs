//! R2：一条派生的证明要能一路读到原句（`docs/decisions/0002`）。
//!
//! 前提一律是断言（`fact_derivations` 不记派生），所以证明是一条链：
//! 派生 → 按 `seq` 排好的断言 → 每条断言的证据 → chunk。这里守三件事：
//!
//! 1. **顺序对**。`A part_of B`、`B part_of C` 推出 `A part_of C`，证明第一步是 A→B。
//! 2. **叶子是原句**。每一步带着它的证据，引句就是当初抽出它的那句话。
//! 3. **撤了的前提照样列出并打标记**。派生随之失效，`proof` 仍能回看当时靠的是什么。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    part_of: Uuid,
    a: Uuid,
    b: Uuid,
    c: Uuid,
    doc: Uuid,
    chunk_ab: Uuid,
    chunk_bc: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let part_of = Uuid::now_v7();
    let (a, b, c) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (src, doc, chunk_ab, chunk_bc) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'proof-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'proof-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name, materialize_inferences)
         VALUES ($1, $2, 'proof-test', TRUE)",
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
        "INSERT INTO relation_types (id, kb_id, key, label, is_transitive)
         VALUES ($1, $2, 'part_of', 'part of', TRUE)",
    )
    .bind(part_of)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(a, "FarmBeats"), (b, "Azure"), (c, "Microsoft")] {
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
    sqlx::query("INSERT INTO sources (id, kb_id, name) VALUES ($1, $2, 'proof-test')")
        .bind(src)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, sha256, status)
         VALUES ($1, $2, $3, 'press.md', 'proof', 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .bind(src)
    .execute(pool)
    .await?;
    for (id, seq, text) in [
        (chunk_ab, 0i32, "FarmBeats is part of Azure."),
        (chunk_bc, 1i32, "Azure is part of Microsoft."),
    ] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(doc)
        .bind(seq)
        .bind(text)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        kb,
        part_of,
        a,
        b,
        c,
        doc,
        chunk_ab,
        chunk_bc,
    })
}

/// 一条断言，带一句原文当证据
async fn asserted(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    object: Uuid,
    chunk: Uuid,
    quote: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(subject)
    .bind(f.part_of)
    .bind(object)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, proposed_predicate, document_id, doc_version)
         VALUES ($1, $2, $3, 'part of', $4, 1)",
    )
    .bind(id)
    .bind(chunk)
    .bind(quote)
    .bind(f.doc)
    .execute(pool)
    .await?;
    Ok(id)
}

#[tokio::test]
async fn a_proof_reaches_the_sentence() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let ab = asserted(
            &pool,
            &f,
            f.a,
            f.b,
            f.chunk_ab,
            "FarmBeats is part of Azure",
        )
        .await?;
        let bc = asserted(
            &pool,
            &f,
            f.b,
            f.c,
            f.chunk_bc,
            "Azure is part of Microsoft",
        )
        .await?;
        reasoning::materialize(&pool, f.kb).await?;

        let derived: Vec<utopia_core::models::DerivedFactView> =
            reasoning::derived_for_entity(&pool, f.kb, f.a, None).await?;
        let ac = derived
            .iter()
            .find(|d| d.subject_id == f.a && d.object_id == Some(f.c))
            .expect("A part_of C should be derived");

        // 1. 顺序对，2. 叶子是原句
        let proof = reasoning::proof(&pool, f.kb, ac.id)
            .await?
            .expect("live derivation has a proof");
        assert_eq!(proof.derived.id, ac.id);
        assert_eq!(proof.steps.len(), 2);
        assert_eq!(
            proof.steps[0].fact_id, ab,
            "the chain starts where the derivation starts"
        );
        assert_eq!(proof.steps[1].fact_id, bc);
        assert_eq!(proof.steps[0].subject, "FarmBeats");
        assert_eq!(proof.steps[0].object.as_deref(), Some("Azure"));
        assert_eq!(proof.steps[0].predicate.as_deref(), Some("part of"));
        assert_eq!(proof.steps[0].evidence.len(), 1);
        assert_eq!(
            proof.steps[0].evidence[0].quote.as_deref(),
            Some("FarmBeats is part of Azure"),
            "the leaf of a proof is the sentence it was extracted from"
        );
        assert_eq!(proof.steps[0].evidence[0].chunk_id, f.chunk_ab);
        assert_eq!(proof.steps[1].evidence[0].chunk_id, f.chunk_bc);
        assert!(proof.steps.iter().all(|s| !s.retracted));

        // 一个不存在的 id 不是错误，是「没有证明」
        assert!(reasoning::proof(&pool, f.kb, Uuid::now_v7())
            .await?
            .is_none());

        // 3. 撤掉一条前提：派生失效，证明还在，且那一步打上标记
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(bc)
            .execute(&pool)
            .await?;
        reasoning::materialize(&pool, f.kb).await?;
        let (gone,): (bool,) =
            sqlx::query_as("SELECT invalidated_at IS NOT NULL FROM derived_facts WHERE id = $1")
                .bind(ac.id)
                .fetch_one(&pool)
                .await?;
        assert!(gone, "a derivation falls with its premise");
        let proof = reasoning::proof(&pool, f.kb, ac.id)
            .await?
            .expect("an invalidated derivation still explains itself");
        assert!(!proof.steps[0].retracted);
        assert!(
            proof.steps[1].retracted,
            "the retracted premise is marked, not hidden"
        );
        anyhow::Ok(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
