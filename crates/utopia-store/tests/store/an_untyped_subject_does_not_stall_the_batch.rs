//! 主语没有类型的值事实，不该把整批采纳卡死。
//!
//! `entities.type_id` 可空——「抽取器抽到了东西，但本体里没有对应的类」是一个
//! 正常状态（0009）。可 `value_facts_for_forms` 一度把它解成裸 `Uuid`：批里
//! 只要有一条主语没类型，采纳就在**解码那一步**报错退出，一条也改写不了。
//!
//! 这条只能真跑：`cargo check` 看不见 sqlx 的行解码，而那正是出事的地方。
//! 实测一个库里攒着 2454 条等谓词的值事实，其中 58 条主语无类型。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn an_untyped_subject_still_comes_back_with_the_batch() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let (infosys, mystery) = (Uuid::now_v7(), Uuid::now_v7());
    let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
    let (f_typed, f_untyped) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'untyped-subject')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'untyped-subject')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'untyped-subject')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'org', 'Organization')",
    )
    .bind(etype)
    .bind(kb)
    .execute(&pool)
    .await?;
    // 一个有类型、一个没有——后者正是从前把整批打翻的那种
    sqlx::query("INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1,$2,$3,$4)")
        .bind(infosys)
        .bind(kb)
        .bind(etype)
        .bind("Infosys")
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1,$2,NULL,$3)")
        .bind(mystery)
        .bind(kb)
        .bind("Some Startup")
        .execute(&pool)
        .await?;
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
    for (fact, subject) in [(f_typed, infosys), (f_untyped, mystery)] {
        // 值事实：没有谓词、没有宾语实体，原词记在证据上
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, object_value)
             VALUES ($1, $2, $3, NULL, NULL, $4)",
        )
        .bind(fact)
        .bind(kb)
        .bind(subject)
        .bind(serde_json::json!({ "value": "$1 billion", "unit": "$" }))
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, document_id, proposed_predicate)
             VALUES ($1, $2, $3, 'pledged_amount')",
        )
        .bind(fact)
        .bind(chunk)
        .bind(doc)
        .execute(&pool)
        .await?;
    }

    let run = async {
        let rows =
            utopia_store::graph::value_facts_for_forms(&pool, kb, &["pledged_amount".to_string()])
                .await?;
        assert_eq!(rows.len(), 2, "两条都该回来，没类型的那条不该把批次打翻");
        let typed = rows.iter().filter(|(_, t, _)| t.is_some()).count();
        assert_eq!(typed, 1, "只有一条主语有类型，它是 domain 的唯一来源");
        // 没类型的那条照样在名单里：它不贡献 domain，但要跟着改写，
        // 否则一条有名有姓的事实会继续没有谓词
        assert!(
            rows.iter()
                .any(|(id, t, _)| *id == f_untyped && t.is_none()),
            "没类型的那条该在名单里，且类型是 None"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
