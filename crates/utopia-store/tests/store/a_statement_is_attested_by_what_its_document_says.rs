//! 陈述的见证来自文档自己说的日期，没有就是没有（0064 决定 3、5）。
//!
//! - 抽取写下的开放陈述不再拿处理文档的那一刻当见证：文档没说就留空；
//! - 留空的行在世界轴上任何时点都成立（下界开放）；没日期的事件除外，它照旧任何时点都不成立；
//! - `attest_statement` 写上见证和那条日期的名字，只往早挪；
//! - 物化出来的类型化行跟着它的来源陈述走。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, TimeZone, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject};
use uuid::Uuid;

const ORG: &str = "statement-attested-by-document-test";

async fn anchor(
    pool: &PgPool,
    fact: Uuid,
) -> anyhow::Result<(Option<DateTime<Utc>>, Option<String>)> {
    Ok(
        sqlx::query_as("SELECT attested_from, attested_by FROM facts WHERE id = $1")
            .bind(fact)
            .fetch_one(pool)
            .await?,
    )
}

/// 这一行在 T 时刻成立吗，按读路径自己的谓词
async fn holds_at(pool: &PgPool, fact: Uuid, at: DateTime<Utc>) -> anyhow::Result<bool> {
    let sql = format!(
        "SELECT {} FROM facts f WHERE f.id = $1",
        utopia_store::world_axis::facts_hold_at("f", 2)
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(fact)
        .bind(at)
        .fetch_one(pool)
        .await?)
}

#[tokio::test]
async fn an_unattested_statement_holds_at_every_moment_and_an_attested_one_from_its_date(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    let (org, ws, kb, watch, market, launch) = (
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
             ('{watch}','{kb}','码表'), ('{market}','{kb}','全球');
         INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES
             ('{launch}','{kb}','launched_in','launched in','relation','event','');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        let long_ago = Utc.with_ymd_and_hms(2015, 1, 1, 0, 0, 0).unwrap();
        let report_day = Utc.with_ymd_and_hms(2026, 9, 4, 0, 0, 0).unwrap();
        let earlier_report = Utc.with_ymd_and_hms(2026, 8, 28, 0, 0, 0).unwrap();

        // 文档没说自己是哪天的：没有见证，任何时点都成立
        let (statement, _) = graph::insert_open_statement(
            &pool,
            kb,
            watch,
            "所属市场为",
            FactObject::Entity(market),
            None,
            0.9,
        )
        .await?;
        assert_eq!(anchor(&pool, statement).await?, (None, None));
        assert!(holds_at(&pool, statement, long_ago).await?, "no time at all: holds at every moment");
        assert!(holds_at(&pool, statement, Utc::now()).await?);

        // 它所在那一节的日期作证：从那天起读得到，之前读不到（0022 的下界）
        assert!(graph::attest_statement(&pool, statement, report_day, "提报日期 2026年9月4日").await?);
        assert_eq!(
            anchor(&pool, statement).await?,
            (Some(report_day), Some("提报日期 2026年9月4日".to_string()))
        );
        assert!(!holds_at(&pool, statement, long_ago).await?);
        assert!(holds_at(&pool, statement, report_day).await?);
        // 更早的文档也说过：见证往早挪，名字跟着；更晚的不动它
        assert!(graph::attest_statement(&pool, statement, earlier_report, "提报日期 2026年8月28日").await?);
        assert!(!graph::attest_statement(&pool, statement, report_day, "提报日期 2026年9月4日").await?);
        assert_eq!(
            anchor(&pool, statement).await?,
            (Some(earlier_report), Some("提报日期 2026年8月28日".to_string()))
        );

        // 物化出来的类型化行跟着来源陈述走；来源没有见证的，它也没有
        let (typed, bare_typed) = (Uuid::now_v7(), Uuid::now_v7());
        let (bare, _) = graph::insert_open_statement(
            &pool,
            kb,
            watch,
            "上市于",
            FactObject::Entity(market),
            None,
            0.9,
        )
        .await?;
        for (id, source) in [(typed, statement), (bare_typed, bare)] {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, object_id, predicate_id, layer, from_statement_id)
                 VALUES ($1, $2, $3, $4, $5, 'typed', $6)",
            )
            .bind(id)
            .bind(kb)
            .bind(watch)
            .bind(market)
            .bind(launch)
            .bind(source)
            .execute(&pool)
            .await?;
            sqlx::query("INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2)")
                .bind(id)
                .bind(source)
                .execute(&pool)
                .await?;
        }
        utopia_store::materialize::sync_typed_attestation(&pool, kb).await?;
        assert_eq!(
            anchor(&pool, typed).await?,
            (Some(earlier_report), Some("提报日期 2026年8月28日".to_string()))
        );
        assert_eq!(anchor(&pool, bare_typed).await?, (None, None));
        // 没日期的事件任何时点都不算成立（0022），没有见证也一样
        assert!(!holds_at(&pool, bare_typed, long_ago).await?);
        assert!(!holds_at(&pool, bare_typed, Utc::now()).await?);
        Ok::<(), anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE name = $1")
        .bind(ORG)
        .execute(&pool)
        .await?;
    run
}
