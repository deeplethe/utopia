//! 采纳时的主宾对调，打在真库上。
//!
//! `X produced_by Y` 与 `Y produces X` 是同一条边。采纳被动形时不对调，
//! 图上就会多出一条反着的箭头，而且它跟正向那些永远合不到一起。
//!
//! 这条只能真跑：对调发生在 `adopt` 的 INSERT 里，`cargo check` 看不见 SQL。
//! 实测中它确实漏过——`demo-b3` 那个库里 `produced_by` 与 `produces`
//! 各成一个关系，因为采纳路径压根没走匹配器。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn adopting_a_passive_wording_flips_subject_and_object() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let produces = Uuid::now_v7();
    let (openai, chatgpt) = (Uuid::now_v7(), Uuid::now_v7());
    let (doc, chunk, fact) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'swap-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'swap-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'swap-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
        .bind(produces)
        .bind(kb)
        .bind("produces")
        .execute(&pool)
        .await?;
    for (id, name) in [(openai, "OpenAI"), (chatgpt, "ChatGPT")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1,$2,$3,$4)",
        )
        .bind(id)
        .bind(kb)
        .bind(etype)
        .bind(name)
        .execute(&pool)
        .await?;
    }
    sqlx::query("INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1,$2,'a.txt','a')")
        .bind(doc)
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1,$2,$3,0,'x')")
        .bind(chunk)
        .bind(kb)
        .bind(doc)
        .execute(&pool)
        .await?;
    // 原文说的是 "ChatGPT produced_by OpenAI"，本体里没有这个关系，
    // 于是这条事实**没有谓词**——兜底谓词已经不存在了
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
         VALUES ($1, $2, $3, NULL, $4)",
    )
    .bind(fact)
    .bind(kb)
    .bind(chatgpt)
    .bind(openai)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, document_id, proposed_predicate)
         VALUES ($1, $2, $3, 'produced_by')",
    )
    .bind(fact)
    .bind(chunk)
    .bind(doc)
    .execute(&pool)
    .await?;

    let run = async {
        let (_, moved) = utopia_store::graph::adopt_proposed_predicates(
            &pool,
            kb,
            produces,
            &["produced_by".to_string()],
            true,
        )
        .await?;
        assert_eq!(moved, 1);
        // 改写后应该是 OpenAI -[produces]-> ChatGPT，**方向反过来**
        let (s, o): (Uuid, Option<Uuid>) = sqlx::query_as(
            "SELECT subject_id, object_id FROM facts
             WHERE kb_id = $1 AND predicate_id = $2 AND invalidated_at IS NULL",
        )
        .bind(kb)
        .bind(produces)
        .fetch_one(&pool)
        .await?;
        assert_eq!(s, openai, "主语该是 OpenAI（原来是宾语）");
        assert_eq!(o, Some(chatgpt), "宾语该是 ChatGPT（原来是主语）");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
