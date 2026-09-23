//! 来源说了不抽取的文档，`queue_extraction` 不排——打在真库上（0035 决定 7，#553）。
//!
//! 挂载数据源时 schema 被摄成一份 markdown，好让问数检索得到表结构。它从前跟
//! 别的文档一样进抽取，抽取器把每个列名都当成实体：宽表语料上四十个概念实体里
//! 二十八个是列名。「只检索、不学习」记在来源的 config 上，而这条过滤放在库里
//! 而不是各个入口——全库重建、按来源重抽、以后任何新入口都走这一个查询。

use sqlx::PgPool;
use uuid::Uuid;

async fn base(pool: &PgPool) -> anyhow::Result<Uuid> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'skip-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'skip-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'skip-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(kb)
}

async fn ready_document(pool: &PgPool, kb: Uuid, source: Uuid, name: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, sha256, status)
         VALUES ($1, $2, $3, $4, $5, 'ready')",
    )
    .bind(id)
    .bind(kb)
    .bind(source)
    .bind(name)
    .bind(format!("sha-{id}"))
    .execute(pool)
    .await?;
    Ok(id)
}

#[tokio::test]
async fn a_source_that_does_not_extract_keeps_its_documents_out_of_the_queue() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = base(&pool).await?;

    let run = async {
        // 与 `sync_schema_doc` 建的那个一模一样：folder，config 里明说不抽取
        let schemas = utopia_store::sources::create(
            &pool,
            kb,
            "folder",
            "Data schemas",
            &serde_json::json!({ "extract": false }),
            Some("database"),
            None,
            None,
        )
        .await?;
        // 老来源：config 是 `{}`，照旧抽取
        let uploads = utopia_store::sources::create(
            &pool,
            kb,
            "folder",
            "Uploads",
            &serde_json::json!({}),
            None,
            None,
            None,
        )
        .await?;
        assert!(!schemas.extracts() && uploads.extracts());

        let schema_doc = ready_document(&pool, kb, schemas.id, "wide-schema.md").await?;
        let prose_doc = ready_document(&pool, kb, uploads.id, "report.md").await?;

        // 全库重建走的就是这一条（source_id = None）
        let queued = utopia_store::documents::queue_extraction(&pool, kb, None).await?;
        assert_eq!(queued, vec![prose_doc], "只有普通来源下的文档该被排上");
        assert!(!queued.contains(&schema_doc));

        // 按来源重抽：来源本身说了不抽取，排出来的是空
        let queued = utopia_store::documents::queue_extraction(&pool, kb, Some(schemas.id)).await?;
        assert!(queued.is_empty(), "不抽取的来源按来源重抽也该是空");

        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 只删知识库，不删 org——用户是软删除的，测试也不该造一个产品里不存在的动作
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
