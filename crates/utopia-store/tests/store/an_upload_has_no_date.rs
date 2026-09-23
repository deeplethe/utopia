//! 一次上传没有日期（0045 决定 3，#714）。
//!
//! 上传、同步的时刻是记录时间，不是文档的日期：没读出日期的文档 `doc_time` 为空、
//! 来源 'none'，`dated_at` 是 None——每一个写入者都这样。正文里读出的日期落成
//! 'content'，来源系统给的落成 'source'，只有这两种算文档的日期；老值
//! 'upload_time' / 'file_mtime' 读作没有日期。时间语境原样进出。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{TimeZone, Utc};
use sqlx::PgPool;
use utopia_store::documents;
use uuid::Uuid;

const ORG: &str = "upload-has-no-date-test";

async fn seed(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws, kb, src) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
        .bind(org)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3)")
        .bind(ws)
        .bind(org)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
        .bind(kb)
        .bind(ws)
        .bind(ORG)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO sources (id, kb_id, name) VALUES ($1, $2, '临时来源')")
        .bind(src)
        .bind(kb)
        .execute(pool)
        .await?;
    Ok((kb, src))
}

#[tokio::test]
async fn an_undated_document_has_no_date_from_any_writer() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let (kb, src) = seed(&pool).await?;

    let run = async {
        // 上传，正文里没读出日期
        let doc =
            documents::create_from_upload(&pool, kb, "a.txt", "text/plain", 1, "sha-a", None, None)
                .await?;
        assert_eq!((doc.doc_time, doc.doc_time_source.as_str()), (None, "none"));
        assert!(doc.dated_at().is_none());
        assert!(doc.time_context.is_none() && doc.time_context_at.is_none());

        // 同步来的没给日期
        let synced = documents::create(
            &pool,
            kb,
            "b.txt",
            "text/plain",
            1,
            "sha-b",
            Some(src),
            None,
            Some("key-b"),
        )
        .await?;
        assert_eq!(
            (synced.doc_time, synced.doc_time_source.as_str()),
            (None, "none")
        );
        let queued = documents::create_with_version_and_processing(
            &pool,
            kb,
            "c.txt",
            "text/plain",
            1,
            "sha-c",
            Some(src),
            None,
            Some("key-c"),
        )
        .await?;
        assert_eq!(
            (queued.doc_time, queued.doc_time_source.as_str()),
            (None, "none")
        );
        let mut tx = pool.begin().await?;
        let upserted = documents::upsert_source_document_tx(
            &mut tx,
            kb,
            src,
            "key-d",
            "d.txt",
            "text/plain",
            1,
            "sha-d",
            None,
        )
        .await?;
        tx.commit().await?;
        assert_eq!(
            (upserted.doc_time, upserted.doc_time_source.as_str()),
            (None, "none")
        );

        // 来源系统给的日期留着它的来源
        let published = Utc.with_ymd_and_hms(2011, 3, 4, 8, 0, 0).unwrap();
        let dated = documents::create(
            &pool,
            kb,
            "e.txt",
            "text/plain",
            1,
            "sha-e",
            Some(src),
            Some(published),
            Some("key-e"),
        )
        .await?;
        assert_eq!(
            (dated.doc_time, dated.doc_time_source.as_str()),
            (Some(published), "source")
        );
        assert_eq!(dated.dated_at(), Some(published));

        // 正文里读出的日期
        let written = Utc.with_ymd_and_hms(2024, 2, 29, 0, 0, 0).unwrap();
        documents::set_content_date(&pool, doc.id, written).await?;
        let doc = documents::get(&pool, doc.id).await?;
        assert_eq!(
            (doc.dated_at(), doc.doc_time_source.as_str()),
            (Some(written), "content")
        );

        // 时间语境原样进出
        let context = serde_json::json!({
            "date": "2024-02-29",
            "periods": [{ "name": "fiscal 2024", "from": "2023-01-29", "to": "2024-01-28" }],
            "anchors": [{ "mention": "the acquisition", "date": "2011-03-04" }]
        });
        documents::set_time_context(&pool, doc.id, &context).await?;
        let doc = documents::get(&pool, doc.id).await?;
        assert_eq!(doc.time_context, Some(context));
        assert!(doc.time_context_at.is_some());

        // 老值读作没有日期
        for old in ["upload_time", "file_mtime"] {
            let mut d = doc.clone();
            d.doc_time = Some(Utc::now());
            d.doc_time_source = old.into();
            assert!(
                d.dated_at().is_none(),
                "{old} is recorded time, not a document date"
            );
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    run
}
