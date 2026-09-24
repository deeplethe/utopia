//! 一条「结束了，不知哪天」的行（0022 / #393）：终点锚在说出结束的那份文档的日期上。之后才到、
//! 日期更早、说它**成立**的一次观察并进这一行时，起点锚往早挪是对的——那是它成立的更早证据；
//! 终点锚不该跟着挪——那份文档说的是成立，不是结束。两个锚点一起挪，区间缩成 [t0, t0)：
//! 读出来这件事从来没成立过，挂在它上面的派生也跟着没了。
//!
//! #875 的回放里撞上的：物品先在桌上，对账把桌面那一段关在搬走那份观察的日期上（锚点），
//! 一条更早的「在桌上」晚到，桌面那一段就读成了空的。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use utopia_store::graph::{self, Validity};
use uuid::Uuid;

#[tokio::test]
async fn an_earlier_mention_that_it_held_keeps_the_stated_end() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (class, location, cup) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'stated-end-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'stated-end-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'stated-end-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'cup', 'Cup')")
            .bind(class)
            .bind(kb)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, temporal)
             VALUES ($1, $2, 'location', 'location', 'attribute', 'text', 'state')",
        )
        .bind(location)
        .bind(kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, 'cup-7')",
        )
        .bind(cup)
        .bind(kb)
        .bind(class)
        .execute(&pool)
        .await?;
        let t = |s: &str| s.parse::<DateTime<Utc>>();
        let (t0, t1, t2) = (
            t("2026-09-23T07:50:00Z")?,
            t("2026-09-23T08:00:00Z")?,
            t("2026-09-23T08:10:00Z")?,
        );
        let desk = json!({ "value": "desk" });
        // t1：在桌上。没有起点，从这份证据起成立
        graph::insert_value_fact(
            &pool,
            kb,
            cup,
            Some(location),
            &desk,
            Validity::default().attested(Some(t1)),
            1.0,
        )
        .await?;
        // t2：它结束了，不知哪天——终点锚在这份文档上
        let (closed, _) = graph::insert_value_fact(
            &pool,
            kb,
            cup,
            Some(location),
            &desk,
            Validity::default().attested(Some(t2)).ended_when_unknown(),
            1.0,
        )
        .await?;
        // t0 < t1：一次更早的「在桌上」晚到。同一断言并进已经关上的那一行
        let merged = graph::insert_value_fact(
            &pool,
            kb,
            cup,
            Some(location),
            &desk,
            Validity::default().attested(Some(t0)),
            1.0,
        )
        .await?;
        assert_eq!(merged, (closed, false));
        let (from, to, precision): (DateTime<Utc>, Option<DateTime<Utc>>, Option<String>) =
            sqlx::query_as(
                "SELECT attested_from, attested_to, valid_to_precision FROM facts WHERE id = $1",
            )
            .bind(closed)
            .fetch_one(&pool)
            .await?;
        assert_eq!(precision.as_deref(), Some(graph::ENDED_UNKNOWN));
        assert_eq!(from, t0, "the earlier mention is earlier evidence that it held");
        assert_eq!(
            to,
            Some(t2),
            "a mention that it held is no evidence that it ended earlier"
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
