//! 一条事实的日期从哪来（#970）。图谱工具的每条事实行带 `[n]`，打开的是那条事实读出来的
//! 原句（#968），可有两种日期原句里没有：
//!
//! - 时间线推出来的终点：李四自 2024-07-05 领 Aurora，周七 2025-09-01 接手，`leads` 一个项目
//!   同时只有一个，对账把李四那一段关在 2025-09-01——`end_derived`；
//! - 人改过的区间：张三的起点被人从 2023-01-10 改成 2023-02-01——`time_corrected`，改的是
//!   起点（`corrected_ends`，随修正行写下），之后这一行再被改写也仍然是；再改终点就是两端。
//!   标记在行上，不靠修正路由事后写的那条审计：李四的起点被改时没有审计，照样有标记。
//!   人交来的是整段区间：李四那个推出来的终点，人没改它的值也是钉住了，两端都算人的。
//!
//! 人手关上的一段两个都不是；它仍是一条修正行（`corrected`，网页证据栏那句提示按它说）。
//!
//! 第二步：推出来的终点找得到关上它的那一行（李四那段是周七那一行；functional 的雇主是后
//! 一个雇主），人改的区间带着修正时写下的备注。记录轴回到接手之前，李四那段还开着，
//! 没有谁关它。
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

fn fact_of<'f>(facts: &'f [EntityFact], who: &str) -> &'f EntityFact {
    facts
        .iter()
        .find(|f| f.other_name.as_deref() == Some(who))
        .unwrap_or_else(|| panic!("no fact of {who}"))
}

/// 那个人的那一行：(终点是推出来的, 区间是人改过的, 是修正行)
fn flags(facts: &[EntityFact], who: &str) -> (bool, bool, bool) {
    let f = fact_of(facts, who);
    (f.end_derived, f.time_corrected, f.corrected)
}

/// 那个人的那一行，人改过哪一端
fn ends(facts: &[EntityFact], who: &str) -> Option<String> {
    fact_of(facts, who).corrected_ends.clone()
}

/// 那个人的那一行被谁关上：(关它的那一行, 接任的那一端)
fn closer(facts: &[EntityFact], who: &str) -> (Option<Uuid>, Option<String>) {
    let f = fact_of(facts, who);
    (f.closed_by_id, f.closed_by.clone())
}

#[tokio::test]
async fn a_fact_says_where_its_dates_came_from() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: Vec<Uuid> = (0..17).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, user, person, project) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
    let (leads, advises, zhang, li, zhou, wang, aurora) =
        (ids[6], ids[7], ids[8], ids[9], ids[10], ids[11], ids[12]);
    let (company, employer, acme, globex) = (ids[13], ids[14], ids[15], ids[16]);
    // Only locally generated UUIDs are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'dates-came-from');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'dates-came-from');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', 'dates-came-from');
         INSERT INTO users (id, org_id, email, display_name, password_hash)
              VALUES ('{user}', '{org}', '{user}@dates.test', 'editor', 'unused');
         INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
             ('{person}', '{kb}', 'person', 'person', '#7fd0ff', 'circle'),
             ('{project}', '{kb}', 'project', 'project', '#7fd0ff', 'circle'),
             ('{company}', '{kb}', 'organization', 'organization', '#7fd0ff', 'circle');
         INSERT INTO relation_types (id, kb_id, key, label, temporal, inverse_functional) VALUES
             ('{leads}', '{kb}', 'leads', 'leads', 'state', TRUE),
             ('{advises}', '{kb}', 'advises', 'advises', 'state', FALSE);
         INSERT INTO relation_types (id, kb_id, key, label, temporal, functional) VALUES
             ('{employer}', '{kb}', 'employer', 'employer', 'state', TRUE);
         INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
             ('{zhang}', '{kb}', '{person}', 'Zhang San'), ('{li}', '{kb}', '{person}', 'Li Si'),
             ('{zhou}', '{kb}', '{person}', 'Zhou Qi'), ('{wang}', '{kb}', '{person}', 'Wang Wu'),
             ('{aurora}', '{kb}', '{project}', 'Aurora'),
             ('{acme}', '{kb}', '{company}', 'Acme'), ('{globex}', '{kb}', '{company}', 'Globex');"
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
        let before_close: DateTime<Utc> =
            sqlx::query_scalar("SELECT now()").fetch_one(&pool).await?;
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[li_row, zhou_row]).await?;

        // 王五换了雇主：一个人同时只有一个雇主（functional），对账把 Acme 那段关在换的那天
        let acme_row = graph::insert_fact(
            &pool,
            kb,
            wang,
            Some(employer),
            acme,
            since("2020-01-01"),
            0.9,
        )
        .await?
        .0;
        let globex_row = graph::insert_fact(
            &pool,
            kb,
            wang,
            Some(employer),
            globex,
            since("2022-03-01"),
            0.9,
        )
        .await?
        .0;
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[acme_row, globex_row]).await?;

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
        // 第二步：谁关的它，人改时说了什么
        assert_eq!(
            closer(&facts, "Li Si"),
            (Some(zhou_row), Some("Zhou Qi".to_string())),
            "Zhou Qi's row took over"
        );
        assert_eq!(closer(&facts, "Zhou Qi"), (None, None), "still open");
        assert_eq!(closer(&facts, "Wang Wu"), (None, None), "closed by hand");
        assert_eq!(
            fact_of(&facts, "Zhang San").correction_note.as_deref(),
            Some("The charter date was the approval date")
        );
        assert_eq!(ends(&facts, "Zhang San").as_deref(), Some("start"));
        assert_eq!(
            ends(&facts, "Wang Wu"),
            None,
            "closed by hand, not corrected"
        );
        let (_, wang_facts) = graph::entity_detail(&pool, kb, wang, None, None).await?;
        assert_eq!(
            closer(&wang_facts, "Acme"),
            (Some(globex_row), Some("Globex".to_string())),
            "a functional property: the next employer took over"
        );
        // 记录轴回到接手之前：李四那段还开着，没有谁关它
        let (_, then) = graph::entity_detail(&pool, kb, aurora, None, Some(before_close)).await?;
        assert!(!flags(&then, "Li Si").0, "not closed yet");
        assert_eq!(closer(&then, "Li Si"), (None, None));

        // 改过的区间再被改写一次（这里是关上），标记随行带着，备注仍在链上找得到
        utopia_store::temporal::close_superseded(
            &pool,
            corrected,
            t("2024-06-30T00:00:00Z"),
            "day",
        )
        .await?;
        let (_, facts) = graph::entity_detail(&pool, kb, aurora, None, None).await?;
        assert_eq!(flags(&facts, "Zhang San"), (false, true, true));
        assert_eq!(ends(&facts, "Zhang San").as_deref(), Some("start"));
        assert_eq!(
            fact_of(&facts, "Zhang San").correction_note.as_deref(),
            Some("The charter date was the approval date"),
            "the note is found up the chain"
        );

        // 人只改了李四的起点、终点照抄：交来的是整段区间，那个推出来的终点从此是人钉住的
        // （时间线不再重画它），两端都算人的——行上不能说终点是原句的。这一次没有审计，
        // 标记照样在行上
        let li_now = fact_of(&facts, "Li Si").id;
        utopia_store::temporal::correct_interval(
            &pool,
            li_now,
            between("2024-07-08", "2025-09-01"),
        )
        .await?
        .expect("the row was live");
        let (_, facts) = graph::entity_detail(&pool, kb, aurora, None, None).await?;
        assert_eq!(
            flags(&facts, "Li Si"),
            (false, true, true),
            "a person's interval: the timeline no longer owns its end"
        );
        assert_eq!(
            ends(&facts, "Li Si").as_deref(),
            Some("both"),
            "the derived end the person kept is theirs too"
        );
        assert_eq!(
            fact_of(&facts, "Li Si").correction_note,
            None,
            "no note written"
        );

        // 张三再改一次，这回改终点：两端都是人改的了，备注取最近那一次
        let zhang_now = fact_of(&facts, "Zhang San").id;
        utopia_store::temporal::correct_interval(
            &pool,
            zhang_now,
            between("2023-02-01", "2024-07-01"),
        )
        .await?
        .expect("the row was live");
        utopia_store::audit::record(
            &pool,
            Some(kb),
            user,
            "fact.time_corrected",
            "fact",
            Some(zhang_now),
            json!({ "note": "The handover was on 1 July" }),
        )
        .await?;
        let (_, facts) = graph::entity_detail(&pool, kb, aurora, None, None).await?;
        assert_eq!(ends(&facts, "Zhang San").as_deref(), Some("both"));
        assert_eq!(
            fact_of(&facts, "Zhang San").correction_note.as_deref(),
            Some("The handover was on 1 July")
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

/// 关闭者的两种情形（#986 合并时的评审）：
/// - 同一天开始的两个后任：时间线按 id 取最先的那一行关上前任，行上说的也是它；
/// - 关闭者自己后来又被下一任关上，改写成了新的一行：说的是活着的那一版，名字不变
#[tokio::test]
async fn a_closer_is_the_row_the_timeline_closed_with() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: Vec<Uuid> = (0..12).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, person, project, leads) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
    let (li, zhou, sun, qian, aurora, helios) = (ids[6], ids[7], ids[8], ids[9], ids[10], ids[11]);
    // Only locally generated UUIDs are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'closers');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'closers');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', 'closers');
         INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
             ('{person}', '{kb}', 'person', 'person', '#7fd0ff', 'circle'),
             ('{project}', '{kb}', 'project', 'project', '#7fd0ff', 'circle');
         INSERT INTO relation_types (id, kb_id, key, label, temporal, inverse_functional)
              VALUES ('{leads}', '{kb}', 'leads', 'leads', 'state', TRUE);
         INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
             ('{li}', '{kb}', '{person}', 'Li Si'), ('{zhou}', '{kb}', '{person}', 'Zhou Qi'),
             ('{sun}', '{kb}', '{person}', 'Sun Ba'), ('{qian}', '{kb}', '{person}', 'Qian Qi'),
             ('{aurora}', '{kb}', '{project}', 'Aurora'), ('{helios}', '{kb}', '{project}', 'Helios');"
    ))
    .execute(&pool)
    .await?;

    let run = async {
        let lead = |who: Uuid, project: Uuid, at: &'static str| {
            let pool = pool.clone();
            async move {
                graph::insert_fact(&pool, kb, who, Some(leads), project, since(at), 0.9)
                    .await
                    .map(|(id, _)| id)
            }
        };
        let live = |who: Uuid, project: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM facts WHERE kb_id = $1 AND subject_id = $2 AND object_id = $3
                        AND invalidated_at IS NULL",
                )
                .bind(kb)
                .bind(who)
                .bind(project)
                .fetch_one(&pool)
                .await
            }
        };

        // Aurora：周七与孙八同一天接手。一个项目同时只有一个 lead，李四那段关在那一天，
        // 关它的是 id 在前的周七那一行
        let li_a = lead(li, aurora, "2024-07-05").await?;
        let zhou_a = lead(zhou, aurora, "2025-09-01").await?;
        let sun_a = lead(sun, aurora, "2025-09-01").await?;
        assert!(zhou_a < sun_a);
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[li_a, zhou_a, sun_a]).await?;
        let (_, facts) = graph::entity_detail(&pool, kb, aurora, None, None).await?;
        assert!(flags(&facts, "Li Si").0, "the timeline closed it");
        assert_eq!(fact_of(&facts, "Li Si").valid_to, Some(day("2025-09-01")));
        assert_eq!(
            closer(&facts, "Li Si"),
            (Some(zhou_a), Some("Zhou Qi".to_string())),
            "the first of the two by id, as the engine orders them"
        );

        // Helios：周七接手之后又被钱七接手，周七那一行被关上、改写成新的一行。李四那段的
        // 关闭者是周七活着的那一版
        let li_h = lead(li, helios, "2024-07-05").await?;
        let zhou_h = lead(zhou, helios, "2025-09-01").await?;
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[li_h, zhou_h]).await?;
        let qian_h = lead(qian, helios, "2026-03-01").await?;
        utopia_store::temporal::reconcile_moved_facts(&pool, kb, &[qian_h]).await?;
        let zhou_now = live(zhou, helios).await?;
        assert_ne!(
            zhou_now, zhou_h,
            "the closer was rewritten when Qian Qi took over"
        );
        let (_, facts) = graph::entity_detail(&pool, kb, helios, None, None).await?;
        assert_eq!(fact_of(&facts, "Zhou Qi").valid_to, Some(day("2026-03-01")));
        assert_eq!(fact_of(&facts, "Li Si").valid_to, Some(day("2025-09-01")));
        assert_eq!(
            closer(&facts, "Li Si"),
            (Some(zhou_now), Some("Zhou Qi".to_string())),
            "the live version of the closer"
        );
        assert_eq!(
            closer(&facts, "Zhou Qi"),
            (Some(qian_h), Some("Qian Qi".to_string()))
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
