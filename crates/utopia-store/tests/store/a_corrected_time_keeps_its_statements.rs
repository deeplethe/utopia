//! 人改一条物化出来的行的区间（302），改完它仍是那条陈述算出来的行（#899 的第三个改写者）。
//!
//! 一条陈述「Zhang San —leads→ Aurora，自 2023-01-10」绑到 leads，物化成一行；另一条说
//! 李四自 2024-07-05 接手，物化后的对账把张三那一段关在交接日。然后人把张三的起点改成
//! 2023-02-01（章程日期是批准日，他 2 月 1 日才接手），终点照旧。
//!
//! 改写出来的修正行若不带上它由哪些陈述算出（`typed_fact_sources`、`from_statement_id`），
//! 两处读法同时出错，时态测量台上五道 Aurora 题就是这么错的：
//!
//! - 陈述不再被活着的类型化行代表，画布、实体面板和对话工具把它**原样**画回来：
//!   自 2023-01-10 起、没有终点。于是 1 月 20 日张三在管（人刚改掉的那一段），2024 年
//!   八月张三和李四同时在管（引擎关上的那一段）
//! - 下一轮物化看见这条陈述没有活着的行，照陈述再算一行，人改的起点被算回去
//!
//! 引擎自己的改写（闭合、搬移）从 #911 起就带着这些；人改区间走的是另一个函数。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_store::graph::{self, Validity};
use utopia_store::materialize::{materialize, Outcome};
use uuid::Uuid;

fn t(s: &str) -> DateTime<Utc> {
    s.parse().expect("fixed timestamp")
}

#[tokio::test]
async fn a_corrected_time_keeps_its_statements() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'corrected-time-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'corrected-time-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'corrected-time-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    let run = async {
        let (person, project, leads) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        for (id, key) in [(person, "person"), (project, "project")] {
            sqlx::query(
                "INSERT INTO entity_types (id, kb_id, key, label, color, shape)
                 VALUES ($1, $2, $3, $3, '#7fd0ff', 'circle')",
            )
            .bind(id)
            .bind(kb)
            .bind(key)
            .execute(&pool)
            .await?;
        }
        // 一个项目同时只有一个 lead：接任关上前任，与测量台的公理表一致
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, inverse_functional)
             VALUES ($1, $2, 'leads', 'leads', 'state', TRUE)",
        )
        .bind(leads)
        .bind(kb)
        .execute(&pool)
        .await?;
        let (zhang, li, aurora) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        for (id, type_id, name) in [
            (zhang, person, "Zhang San"),
            (li, person, "Li Si"),
            (aurora, project, "Aurora"),
        ] {
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(kb)
            .bind(type_id)
            .bind(name)
            .execute(&pool)
            .await?;
        }
        let (charter, handover) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, who, from) in [(charter, zhang, "2023-01-10"), (handover, li, "2024-07-05")] {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, object_id, layer, phrase, confidence,
                                    valid_from, valid_from_precision, valid_from_grade)
                 VALUES ($1, $2, $3, $4, 'open', 'leads', 0.9, $5::date, 'day', 'A')",
            )
            .bind(id)
            .bind(kb)
            .bind(who)
            .bind(aurora)
            .bind(from)
            .execute(&pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO phrase_bindings
                 (id, kb_id, phrase, subject_type_id, object_type_id, object_is_value,
                  relation_type_id, direction, status)
             VALUES ($1, $2, 'leads', $3, $4, false, $5, 'forward', 'bound')",
        )
        .bind(Uuid::now_v7())
        .bind(kb)
        .bind(person)
        .bind(project)
        .bind(leads)
        .execute(&pool)
        .await?;

        // 两条陈述两行；物化后的对账把张三那一段关在李四接手那天
        let first = materialize(&pool, kb).await?;
        assert_eq!((first.added, first.corrected), (2, 1), "{first:?}");
        let zhang_rows = |pool: PgPool| async move {
            sqlx::query_as::<_, (Uuid, Option<DateTime<Utc>>, Option<DateTime<Utc>>)>(
                "SELECT id, valid_from, valid_to FROM facts
                  WHERE kb_id = $1 AND layer = 'typed' AND subject_id = $2 AND invalidated_at IS NULL",
            )
            .bind(kb)
            .bind(zhang)
            .fetch_all(&pool)
            .await
        };
        let rows = zhang_rows(pool.clone()).await?;
        assert_eq!(rows.len(), 1);
        let (closed, _, to) = rows[0];
        assert_eq!(to, Some(t("2024-07-05T00:00:00Z")), "交接日关上前任");

        // 人改起点，终点照旧；与 PATCH /facts/{id} 同一条路：改完按搬移重新对账
        let corrected = utopia_store::temporal::correct_interval(
            &pool,
            closed,
            Validity {
                from: Some(t("2023-02-01T00:00:00Z")),
                from_precision: Some("day"),
                to: Some(t("2024-07-05T00:00:00Z")),
                to_precision: Some("day"),
                ..Default::default()
            },
        )
        .await?
        .expect("the row was live");
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[corrected]).await?;

        // 修正行仍是那条陈述算出来的
        let (from_statement,): (Option<Uuid>,) =
            sqlx::query_as("SELECT from_statement_id FROM facts WHERE id = $1")
                .bind(corrected)
                .fetch_one(&pool)
                .await?;
        let sources: Vec<Uuid> = sqlx::query_scalar(
            "SELECT statement_id FROM typed_fact_sources WHERE fact_id = $1",
        )
        .bind(corrected)
        .fetch_all(&pool)
        .await?;
        assert_eq!((from_statement, sources), (Some(charter), vec![charter]));

        // 画面上一条陈述只画一条边：改过的那一段。1 月 20 日还没人在管，2024 年八月只有李四
        let leads_of_aurora = |at: &'static str| {
            let pool = pool.clone();
            async move {
                let (nodes, edges) =
                    graph::neighborhood(&pool, kb, aurora, 1, Some(t(at)), None).await?;
                let mut names: Vec<String> = edges
                    .iter()
                    .filter(|e| e.target == aurora)
                    .filter_map(|e| nodes.iter().find(|n| n.id == e.source))
                    .map(|n| n.name.clone())
                    .collect();
                names.sort();
                anyhow::Ok(names)
            }
        };
        assert_eq!(leads_of_aurora("2023-01-20T00:00:00Z").await?, Vec::<String>::new());
        assert_eq!(leads_of_aurora("2023-03-01T00:00:00Z").await?, vec!["Zhang San"]);
        assert_eq!(leads_of_aurora("2024-08-01T00:00:00Z").await?, vec!["Li Si"]);

        // 下一轮物化没有要补的：人改的起点不被算回去
        assert_eq!(materialize(&pool, kb).await?, Outcome::default());
        let rows = zhang_rows(pool.clone()).await?;
        assert_eq!(
            rows.iter().map(|r| (r.1, r.2)).collect::<Vec<_>>(),
            vec![(Some(t("2023-02-01T00:00:00Z")), Some(t("2024-07-05T00:00:00Z")))],
            "张三只有人改过的那一段"
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
