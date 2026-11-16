//! 绑上了却按 datatype 读不出的值记成 `attr_datatype` 丢弃行，而不是无声地不写；
//! 属性改成读得出之后那一行退掉、类型化行写下。没有 `UTOPIA_DATABASE_URL` 时跳过。

use super::materialize_in_tx;
use crate::{extraction_drops, phrase_bindings};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    document: Uuid,
    property: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb, document, chunk, company, property, statement) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'unreadable')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'unreadable')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'unreadable')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents(id,kb_id,filename,sha256,status,graph_status) VALUES($1,$2,'启明.txt','x','ready','done')",
    )
    .bind(document)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,0,'员工人数超过2000人')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(document)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,'启明半导体')")
        .bind(company)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO relation_types(id,kb_id,key,label,temporal,kind,datatype) VALUES($1,$2,'employee_count','员工人数','state','attribute','number')",
    )
    .bind(property)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO facts(id,kb_id,subject_id,object_value,layer,phrase) VALUES($1,$2,$3,$4,'open','员工人数')",
    )
    .bind(statement)
    .bind(kb)
    .bind(company)
    .bind(json!({ "value": "超过2000人" }))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence(fact_id,chunk_id,document_id,quote) VALUES($1,$2,$3,'员工人数超过2000人')",
    )
    .bind(statement)
    .bind(chunk)
    .bind(document)
    .execute(pool)
    .await?;
    let sig = phrase_bindings::signatures(pool, kb).await?.remove(0);
    let mut tx = pool.begin().await?;
    phrase_bindings::decide_on(
        &mut tx,
        kb,
        &sig,
        phrase_bindings::Decision {
            relation_type_id: Some(property),
            direction: Some("forward"),
            status: "bound",
            votes: &serde_json::Value::Null,
            decided_by: "person",
            basis: None,
            marks: None,
            marks_asked: false,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(Fx {
        org,
        kb,
        document,
        property,
    })
}

async fn recompute(pool: &PgPool, kb: Uuid) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    materialize_in_tx(&mut tx, kb).await?;
    tx.commit().await?;
    Ok(())
}

async fn typed_rows(pool: &PgPool, kb: Uuid) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM facts WHERE kb_id=$1 AND layer='typed' AND invalidated_at IS NULL",
    )
    .bind(kb)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn a_value_the_datatype_cannot_read_is_a_drop_row_until_it_can() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        recompute(&pool, f.kb).await?;
        assert_eq!(typed_rows(&pool, f.kb).await?, 0);
        let drops = extraction_drops::for_kb(&pool, f.kb).await?;
        assert_eq!(drops.len(), 1, "{drops:?}");
        assert_eq!(drops[0].reason, extraction_drops::reason::ATTR_DATATYPE);
        assert_eq!(drops[0].document_id, f.document);
        assert_eq!(drops[0].detail, "employee_count");
        assert_eq!(drops[0].count, 1);
        assert_eq!(drops[0].example.as_deref(), Some("启明半导体 → 超过2000人"));
        // 再算一遍不翻倍：按集合重写
        recompute(&pool, f.kb).await?;
        let drops = extraction_drops::for_kb(&pool, f.kb).await?;
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].count, 1);
        // 属性改成文本：值读得出了，行写下，丢弃退掉
        sqlx::query("UPDATE relation_types SET datatype='text' WHERE id=$1")
            .bind(f.property)
            .execute(&pool)
            .await?;
        recompute(&pool, f.kb).await?;
        assert_eq!(typed_rows(&pool, f.kb).await?, 1);
        assert!(extraction_drops::for_kb(&pool, f.kb).await?.is_empty());
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
