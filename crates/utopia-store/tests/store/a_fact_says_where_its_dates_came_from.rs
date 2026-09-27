//! 一条事实的日期从哪来（#970）。图谱工具的每条事实行带 `[n]`，打开的是那条事实读出来的
//! 原句（#968），可有两种日期原句里没有：
//!
//! - 时间线推出来的终点：李四自 2024-07-05 领 Aurora，周七 2025-09-01 接手，`leads` 一个项目
//!   同时只有一个，对账把李四那一段关在 2025-09-01——`end_derived`；
//! - 人改过的区间：张三的起点被人从 2023-01-10 改成 2023-02-01，`fact.time_corrected` 记在
//!   被改的那一行上——`time_corrected`，之后这一行再被改写也仍然是。
//!
//! 人手关上的一段两个都不是；它仍是一条修正行（`corrected`，网页证据栏那句提示按它说）。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use utopia_core::models::EntityFact;
use utopia_store::graph::{self, Validity};
use uuid::Uuid;

fn t(s: &str) -> DateTime<Utc> {
    s.parse().expect("fixed timestamp")
}

/// 一个日精度的日子
fn day(s: &str) -> DateTime<Utc> {
    t(&format!("{s}T00:00:00Z"))
}

fn since(at: &str) -> Validity<'static> {
    Validity::starting(Some(day(at)), Some("day"))
}

fn between(from: &str, to: &str) -> Validity<'static> {
    Validity {
        to: Some(day(to)),
        to_precision: Some("day"),
        ..since(from)
    }
}

/// Aurora 身上那个人的那一行：(终点是推出来的, 区间是人改过的, 是修正行)
fn flags(facts: &[EntityFact], who: &str) -> (bool, bool, bool) {
    let f = facts
        .iter()
        .find(|f| f.other_name.as_deref() == Some(who))
        .unwrap_or_else(|| panic!("no fact of {who}"));
    (f.end_derived, f.time_corrected, f.corrected)
}

#[tokio::test]
async fn a_fact_says_where_its_dates_came_from() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: Vec<Uuid> = (0..13).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, user, person, project) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
    let (leads, advises, zhang, li, zhou, wang, aurora) =
        (ids[6], ids[7], ids[8], ids[9], ids[10], ids[11], ids[12]);
    // Only locally generated UUIDs are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'dates-came-from');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'dates-came-from');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', 'dates-came-from');
         INSERT INTO users (id, org_id, email, display_name, password_hash)
              VALUES ('{user}', '{org}', '{user}@dates.test', 'editor', 'unused');
         INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
             ('{person}', '{kb}', 'person', 'person', '#7fd0ff', 'circle'),
             ('{project}', '{kb}', 'project', 'project', '#7fd0ff', 'circle');
         INSERT INTO relation_types (id, kb_id, key, label, temporal, inverse_functional) VALUES
             ('{leads}', '{kb}', 'leads', 'leads', 'state', TRUE),
             ('{advises}', '{kb}', 'advises', 'advises', 'state', FALSE);
         INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
             ('{zhang}', '{kb}', '{person}', 'Zhang San'), ('{li}', '{kb}', '{person}', 'Li Si'),
             ('{zhou}', '{kb}', '{person}', 'Zhou Qi'), ('{wang}', '{kb}', '{person}', 'Wang Wu'),
             ('{aurora}', '{kb}', '{project}', 'Aurora');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        let fact = |who: Uuid, predicate: Uuid, validity: Validity<'static>| {
            let pool = pool.clone();
            async move {
                graph::insert_fact(&pool, kb, who, Some(predicate), aurora, validity, 0.9)
                    .await
                    .map(|(id, _)| id)
            }
        };
        // 周七接手：一个项目同时只有一个 lead，对账把李四那一段关在 2025-09-01
        let li_row = fact(li, leads, since("2024-07-05")).await?;
        let zhou_row = fact(zhou, leads, since("2025-09-01")).await?;
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[li_row, zhou_row]).await?;

        // 人改张三的起点，终点照旧；审计记在被改的那一行上（与 PATCH /facts/{id} 同一条路）
        let zhang_row = fact(zhang, leads, between("2023-01-10", "2024-07-05")).await?;
        let corrected = utopia_store::temporal::correct_interval(
            &pool,
            zhang_row,
            between("2023-02-01", "2024-07-05"),
        )
        .await?
        .expect("the row was live");
        utopia_store::audit::record(
            &pool,
            Some(kb),
            user,
            "fact.time_corrected",
            "fact",
            Some(zhang_row),
            json!({ "note": "The charter date was the approval date" }),
        )
        .await?;

        // 人手关上的一段：一条修正行，但终点是人给的，区间也没被改过
        let wang_row = graph::insert_fact(
            &pool,
            kb,
            wang,
            Some(advises),
            aurora,
            since("2023-03-02"),
            0.9,
        )
        .await?
        .0;
        utopia_store::temporal::close_superseded(&pool, wang_row, t("2024-01-01T00:00:00Z"), "day")
            .await?;

        let (_, facts) = graph::entity_detail(&pool, kb, aurora, None, None).await?;
        assert_eq!(
            flags(&facts, "Li Si"),
            (true, false, true),
            "the timeline closed it"
        );
        assert_eq!(flags(&facts, "Zhou Qi"), (false, false, false));
        assert_eq!(
            flags(&facts, "Zhang San"),
            (false, true, true),
            "a person corrected it"
        );
        assert_eq!(
            flags(&facts, "Wang Wu"),
            (false, false, true),
            "closed by hand: rewritten, but neither mark"
        );

        // 改过的区间再被改写一次（这里是关上），往上的链里仍有那一次人改
        utopia_store::temporal::close_superseded(
            &pool,
            corrected,
            t("2024-06-30T00:00:00Z"),
            "day",
        )
        .await?;
        let (_, facts) = graph::entity_detail(&pool, kb, aurora, None, None).await?;
        assert_eq!(flags(&facts, "Zhang San"), (false, true, true));
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
