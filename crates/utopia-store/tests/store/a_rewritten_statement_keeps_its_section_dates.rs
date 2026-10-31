//! 开放陈述改挂到另一个实体、人改了它的区间，新行还是那条开放陈述：层和短语不变，带着
//! 见证日期的名字，它的证据也照样记着各自那一节的日期（0104）。
//!
//! 之后删掉最早那篇文档，见证挪到还在的证据里最早的那一节，连同名字；不退回那篇文档
//! 自己的日期。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject, Validity};
use uuid::Uuid;

const ORG: &str = "rewritten-statement-keeps-its-section-dates-test";

fn day(y: i32, m: u32, d: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap()
}

type Anchor = (Option<DateTime<Utc>>, Option<String>);

/// 改写出来的新行还是开放陈述，说的还是原文那个短语
async fn shape(pool: &PgPool, fact: Uuid) -> anyhow::Result<(String, Option<String>)> {
    Ok(
        sqlx::query_as("SELECT layer, phrase FROM facts WHERE id = $1")
            .bind(fact)
            .fetch_one(pool)
            .await?,
    )
}

async fn anchor(pool: &PgPool, fact: Uuid) -> anyhow::Result<Anchor> {
    Ok(
        sqlx::query_as("SELECT attested_from, attested_by FROM facts WHERE id = $1")
            .bind(fact)
            .fetch_one(pool)
            .await?,
    )
}

/// 一篇自己带日期的文档和它的一块正文
async fn document(pool: &PgPool, kb: Uuid, own: DateTime<Utc>) -> anyhow::Result<(Uuid, Uuid)> {
    let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, doc_time, doc_time_source)
         VALUES ($1, $2, $3, $3, $4, 'content')",
    )
    .bind(doc)
    .bind(kb)
    .bind(format!("doc-{doc}.md"))
    .bind(own)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .bind("# 年报\n码表所属市场为全球。")
    .execute(pool)
    .await?;
    Ok((doc, chunk))
}

/// 一条开放陈述，两篇文档各有一条证据，各记着自己那一节的日期
async fn statement(
    pool: &PgPool,
    kb: Uuid,
    subject: Uuid,
    object: Uuid,
    evidence: &[(Uuid, DateTime<Utc>, &str)],
) -> anyhow::Result<Uuid> {
    let (id, _) = graph::insert_open_statement(
        pool,
        kb,
        subject,
        "所属市场为",
        FactObject::Entity(object),
        None,
        0.9,
    )
    .await?;
    for (chunk, at, by) in evidence {
        graph::add_evidence(pool, id, *chunk, Some("码表所属市场为全球"), None).await?;
        graph::witness_evidence(pool, id, *chunk, Some((*at, *by))).await?;
        graph::attest_statement(pool, id, *at, by).await?;
    }
    Ok(id)
}

#[tokio::test]
async fn a_rewritten_statement_keeps_the_dates_its_sections_speak_from() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let (org, ws, kb, watch, meter, market) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','{ORG}');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','{ORG}');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','{ORG}');
         INSERT INTO entities(id,kb_id,canonical_name) VALUES
             ('{watch}','{kb}','码表'), ('{meter}','{kb}','里程表'), ('{market}','{kb}','全球');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        let (chapter_2015, chapter_2020) = (day(2015, 6, 1), day(2020, 6, 1));
        let (by_2015, by_2020) = ("2015 年年报", "2020 年年报");
        // 两篇文档自己的日期都在 2026 年，章节说的是 2015 和 2020
        let (early, early_chunk) = document(&pool, kb, day(2026, 1, 10)).await?;
        let (_late, late_chunk) = document(&pool, kb, day(2026, 3, 1)).await?;
        let evidence = [(early_chunk, chapter_2015, by_2015), (late_chunk, chapter_2020, by_2020)];

        // 一条改挂到另一个实体上（合并时走的那条路）
        let to_move = statement(&pool, kb, watch, market, &evidence).await?;
        let mut tx = pool.begin().await?;
        let moved = utopia_store::temporal::rehome_tx(&mut tx, to_move, Some(meter), None)
            .await?
            .expect("a live row is rehomed");
        tx.commit().await?;
        // 一条由人改了区间
        let to_correct = statement(&pool, kb, watch, market, &evidence).await?;
        let corrected = utopia_store::temporal::correct_interval(
            &pool,
            to_correct,
            Validity {
                from: Some(day(2014, 1, 1)),
                from_precision: Some("year"),
                ..Default::default()
            },
        )
        .await?
        .expect("a live row is corrected");

        let earliest: Anchor = (Some(chapter_2015), Some(by_2015.to_string()));
        for fact in [moved, corrected] {
            assert_eq!(
                shape(&pool, fact).await?,
                ("open".to_string(), Some("所属市场为".to_string())),
                "a rewritten statement stays an open statement with its phrase"
            );
            assert_eq!(anchor(&pool, fact).await?, earliest, "before the deletion");
        }

        utopia_store::documents::delete(&pool, kb, early, None).await?;

        let remaining: Anchor = (Some(chapter_2020), Some(by_2020.to_string()));
        assert_eq!(
            anchor(&pool, moved).await?,
            remaining,
            "a rehomed statement reads the chapter date of the evidence that remains, not its document's own date"
        );
        assert_eq!(
            anchor(&pool, corrected).await?,
            remaining,
            "a corrected statement reads the chapter date of the evidence that remains, not its document's own date"
        );
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
