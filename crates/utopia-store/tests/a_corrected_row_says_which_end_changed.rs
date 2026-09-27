//! 迁移 0099：这一列之前人改过区间的行，从审计里算出改的是哪一端。
//!
//! 照修正路由的来路造：`correct_interval` 改区间，审计记在被改的那一行上，`detail` 带改前
//! （`from`）与改后（`to`）的区间；再把这一列抹掉——那正是这一列出现之前写下的样子。
//!
//! - 张三：改了起点 → start；之后人手关上（一次改写）仍是 start
//! - 李四：先改起点、再改终点 → both
//! - 王五：改了终点 → end
//! - 赵六：只是人手关上，没有改区间的审计 → 空
//! - 钱七：审计没记改前改后 → both（说不清改了哪端，两端都不归原句）
//! - 孙八：只改了起点，可原来的终点是时间线推出来的 → both（人把那个终点钉住了）
//!
//! 只填还空着的：已经有标记的行，审计说什么都不改它。再跑一遍迁移，一样不动。
//!
//! 迁移的语句不按 kb 过滤，改的是全库的行，所以这个测试在 tests/ 顶层自成一个二进制
//! （与 0096 同理）。没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use utopia_store::graph::{self, Validity};
use uuid::Uuid;

const MIGRATION: &str =
    include_str!("../../../migrations/0099_a_corrected_row_says_which_end_a_person_changed.sql");

fn day(s: &str) -> DateTime<Utc> {
    format!("{s}T00:00:00Z").parse().expect("fixed day")
}

fn between(from: &str, to: Option<&str>) -> Validity<'static> {
    Validity {
        from: Some(day(from)),
        from_precision: Some("day"),
        to: to.map(day),
        to_precision: to.map(|_| "day"),
        attested_at: None,
        from_grade: None,
    }
}

/// 修正路由记下的一端：与它一样用 RFC 3339 写日期
fn side(v: &Validity<'_>) -> Value {
    json!({
        "valid_from": v.from.map(|t| t.to_rfc3339()),
        "valid_from_precision": v.from_precision,
        "valid_to": v.to.map(|t| t.to_rfc3339()),
        "valid_to_precision": v.to_precision,
    })
}

/// 一行活着的那一版的标记
async fn ends(pool: &PgPool, kb: Uuid, subject: Uuid) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT corrected_ends FROM facts
          WHERE kb_id = $1 AND subject_id = $2 AND invalidated_at IS NULL",
    )
    .bind(kb)
    .bind(subject)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn a_row_corrected_before_the_column_gets_its_ends_back() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: Vec<Uuid> = (0..11).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, user, person, project, leads) =
        (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6]);
    let people = [
        ids[7],
        ids[8],
        ids[9],
        ids[10],
        Uuid::now_v7(),
        Uuid::now_v7(),
    ];
    let aurora = Uuid::now_v7();
    // Only locally generated UUIDs are interpolated into fixture SQL.
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'corrected-ends');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'corrected-ends');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', 'corrected-ends');
         INSERT INTO users (id, org_id, email, display_name, password_hash)
              VALUES ('{user}', '{org}', '{user}@ends.test', 'editor', 'unused');
         INSERT INTO entity_types (id, kb_id, key, label, color, shape) VALUES
             ('{person}', '{kb}', 'person', 'person', '#7fd0ff', 'circle'),
             ('{project}', '{kb}', 'project', 'project', '#7fd0ff', 'circle');
         INSERT INTO relation_types (id, kb_id, key, label, temporal)
              VALUES ('{leads}', '{kb}', 'leads', 'leads', 'state');
         INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES
             ('{aurora}', '{kb}', '{project}', 'Aurora'),
             ('{zhang}', '{kb}', '{person}', 'Zhang San'), ('{li}', '{kb}', '{person}', 'Li Si'),
             ('{wang}', '{kb}', '{person}', 'Wang Wu'), ('{zhao}', '{kb}', '{person}', 'Zhao Liu'),
             ('{qian}', '{kb}', '{person}', 'Qian Qi'), ('{sun}', '{kb}', '{person}', 'Sun Ba');",
        zhang = people[0],
        li = people[1],
        wang = people[2],
        zhao = people[3],
        qian = people[4],
        sun = people[5],
    ))
    .execute(&pool)
    .await?;
    let [zhang, li, wang, zhao, qian, sun] = people;

    let run = async {
        let row = |who: Uuid, v: Validity<'static>| {
            let pool = pool.clone();
            async move {
                anyhow::Ok(
                    graph::insert_fact(&pool, kb, who, Some(leads), aurora, v, 0.9)
                        .await?
                        .0,
                )
            }
        };
        // 人改一次区间：修正行，加上修正路由记在被改的那一行上的审计
        let correct =
            |fact: Uuid, before: Validity<'static>, after: Validity<'static>, detail: bool| {
                let pool = pool.clone();
                async move {
                    let corrected = utopia_store::temporal::correct_interval(&pool, fact, after)
                        .await?
                        .expect("the row was live");
                    let detail = if detail {
                        json!({ "from": side(&before), "to": side(&after) })
                    } else {
                        json!({})
                    };
                    utopia_store::audit::record(
                        &pool,
                        Some(kb),
                        user,
                        "fact.time_corrected",
                        "fact",
                        Some(fact),
                        detail,
                    )
                    .await?;
                    anyhow::Ok(corrected)
                }
            };

        let first = between("2023-01-10", Some("2024-07-05"));
        let z = row(zhang, first).await?;
        let z = correct(z, first, between("2023-02-01", Some("2024-07-05")), true).await?;
        utopia_store::temporal::close_superseded(&pool, z, day("2024-06-30"), "day").await?;

        let l = row(li, first).await?;
        let moved = between("2023-02-01", Some("2024-07-05"));
        let l = correct(l, first, moved, true).await?;
        correct(l, moved, between("2023-02-01", Some("2024-08-01")), true).await?;

        let w = row(wang, first).await?;
        correct(w, first, between("2023-01-10", Some("2024-06-01")), true).await?;

        let open = between("2023-01-10", None);
        let h = row(zhao, open).await?;
        utopia_store::temporal::close_superseded(&pool, h, day("2024-01-01"), "day").await?;

        let q = row(qian, first).await?;
        correct(q, first, between("2023-03-01", Some("2024-07-05")), false).await?;

        // 时间线推出来的终点（这里直接标上），人只改起点、终点照抄
        let s = row(sun, first).await?;
        sqlx::query("UPDATE facts SET end_derived = TRUE WHERE id = $1")
            .bind(s)
            .execute(&pool)
            .await?;
        correct(s, first, between("2023-02-01", Some("2024-07-05")), true).await?;

        let now: Vec<Option<String>> = {
            let mut v = Vec::new();
            for who in people {
                v.push(ends(&pool, kb, who).await?);
            }
            v
        };
        assert_eq!(
            now,
            [
                Some("start"),
                Some("both"),
                Some("end"),
                None,
                Some("start"),
                Some("both")
            ]
            .map(|e| e.map(str::to_string)),
            "what correct_interval wrote"
        );

        // 这一列出现之前的样子：没有标记。王五那一行已经有一个（与审计说的不一样）的标记，
        // 回填不动它
        sqlx::query("UPDATE facts SET corrected_ends = NULL WHERE kb_id = $1 AND subject_id <> $2")
            .bind(kb)
            .bind(wang)
            .execute(&pool)
            .await?;
        sqlx::query(
            "UPDATE facts SET corrected_ends = 'both'
              WHERE kb_id = $1 AND subject_id = $2 AND invalidated_at IS NULL",
        )
        .bind(kb)
        .bind(wang)
        .execute(&pool)
        .await?;

        sqlx::raw_sql(MIGRATION).execute(&pool).await?;
        assert_eq!(ends(&pool, kb, zhang).await?.as_deref(), Some("start"));
        assert_eq!(ends(&pool, kb, li).await?.as_deref(), Some("both"));
        assert_eq!(
            ends(&pool, kb, wang).await?.as_deref(),
            Some("both"),
            "a row that has its mark keeps it"
        );
        assert_eq!(ends(&pool, kb, zhao).await?, None, "closed by hand");
        assert_eq!(
            ends(&pool, kb, qian).await?.as_deref(),
            Some("both"),
            "an audit without the two intervals cannot say which end"
        );
        assert_eq!(
            ends(&pool, kb, sun).await?.as_deref(),
            Some("both"),
            "the derived end the person kept"
        );

        // 再跑一遍，一样不动
        sqlx::raw_sql(MIGRATION).execute(&pool).await?;
        assert_eq!(ends(&pool, kb, zhang).await?.as_deref(), Some("start"));
        assert_eq!(ends(&pool, kb, li).await?.as_deref(), Some("both"));
        assert_eq!(ends(&pool, kb, zhao).await?, None);
        anyhow::Ok(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
