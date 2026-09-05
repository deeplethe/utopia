//! 未知的日期不是开放的日期（0022 / #345 / #352），打在真库上。
//!
//! 为什么非要连库：这道防御整个活在 SQL 字符串里（`world_axis`），与 0019 的记录轴
//! 同一种危险——`cargo check` 看不见一个写反的 CASE，漏掉一处读点也没有人报错。
//!
//! 表里的四种行，两个方向都断言：
//! - 没有起点的事实，在它的文档日期**之前不出现**、**当天出现**（读成「自古如此」
//!   会让 2023 年 1 月看见一张 2024 年的调薪单）
//! - 结束了不知哪天的事实，在它的文档日期**之前出现**、**当天起不出现**（读成
//!   「至今仍是」会让 2026 年还挂着一个原文说已经不担任的头衔）
//! - 原文给了两端的事实，两端照旧——这条改动不碰有日期的行
//! - 两端都不知道的行，**任何时刻都不成立**，但实体的时间线上仍列着它
//!
//! 再钉两条写入侧的约定：同一断言被更早的文档再观察到一次，锚点往早挪；
//! 修正行从被替代的行继承锚点，而不是重新锚在修正的那一刻。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use std::collections::HashSet;

use sqlx::PgPool;
use utopia_store::graph::Validity;
use uuid::Uuid;

/// 关上之后那一行：起点、终点、终点精度、supersedes、锚点
type ClosedRow = (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
);

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    lin: Uuid,
    meridian: Uuid,
    platform: Uuid,
    staff_engineer: Uuid,
    lead: Uuid,
    works_for: Uuid,
    approved: Uuid,
    holds_title: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, organization, title) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (lin, meridian, platform) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (staff_engineer, lead) = (Uuid::now_v7(), Uuid::now_v7());
    let (works_for, approved, holds_title) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'unknown-date-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'unknown-date-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'unknown-date-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (organization, "organization", "Organization"),
        (title, "title", "Title"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, key, label) in [
        (works_for, "works_for", "works for"),
        (approved, "approved", "approved"),
        (holds_title, "holds_title", "holds title"),
    ] {
        sqlx::query("INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, ty, name) in [
        (lin, person, "Lin Zhao"),
        (meridian, organization, "Meridian Systems"),
        (platform, organization, "Platform Group"),
        (staff_engineer, title, "Staff Engineer"),
        (lead, title, "Tech Lead"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(ty)
        .bind(name)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        kb,
        lin,
        meridian,
        platform,
        staff_engineer,
        lead,
        works_for,
        approved,
        holds_title,
    })
}

async fn fact(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    predicate: Uuid,
    object: Uuid,
    validity: Validity<'_>,
) -> anyhow::Result<(Uuid, bool)> {
    Ok(utopia_store::graph::insert_fact(
        pool,
        f.kb,
        subject,
        Some(predicate),
        object,
        validity,
        0.9,
    )
    .await?)
}

/// T 时刻林昭一跳邻域里的边（按事实 id）。`None` = 每一刻。
async fn edges_at(pool: &PgPool, f: &Fixture, at: Option<&str>) -> anyhow::Result<HashSet<Uuid>> {
    let (_, edges) =
        utopia_store::graph::neighborhood(pool, f.kb, f.lin, 1, at.map(t), None).await?;
    Ok(edges.into_iter().map(|e| e.id).collect())
}

async fn attested_at(pool: &PgPool, id: Uuid) -> anyhow::Result<chrono::DateTime<chrono::Utc>> {
    Ok(
        sqlx::query_scalar("SELECT attested_at FROM facts WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
async fn an_unknown_bound_reaches_as_far_as_the_evidence() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // A：offer letter，2023-06-01 起，仍在继续——两端原文都给了（起点）
        let (a, _) = fact(
            &pool,
            &f,
            f.lin,
            f.works_for,
            f.meridian,
            Validity::starting(Some(t("2023-06-01T00:00:00Z")), Some("day"))
                .attested(Some(t("2023-06-01T00:00:00Z"))),
        )
        .await?;
        // B：2024-02-20 的调薪单说「加薪由平台组批准」——没有起点
        let (b, _) = fact(
            &pool,
            &f,
            f.platform,
            f.approved,
            f.lin,
            Validity::default().attested(Some(t("2024-02-20T00:00:00Z"))),
        )
        .await?;
        // C：2025-10-15 的角色说明说「不再担任 Staff Engineer，日期未记录」——两端都不知道
        let (c, _) = fact(
            &pool,
            &f,
            f.lin,
            f.holds_title,
            f.staff_engineer,
            Validity::default()
                .ended_when_unknown()
                .attested(Some(t("2025-10-15T00:00:00Z"))),
        )
        .await?;
        // D：2023-06-01 起任 Tech Lead，同一份 2025-10-15 的说明说已不再担任——起点有，终点不知哪天
        let (d, _) = fact(
            &pool,
            &f,
            f.lin,
            f.holds_title,
            f.lead,
            Validity::starting(Some(t("2023-06-01T00:00:00Z")), Some("day"))
                .ended_when_unknown()
                .attested(Some(t("2025-10-15T00:00:00Z"))),
        )
        .await?;

        // 2023 年 1 月：什么都还没有。B 的证据在 2024 年——「自古如此」读法会让它出现
        let jan_2023 = edges_at(&pool, &f, Some("2023-01-01T00:00:00Z")).await?;
        assert!(
            jan_2023.is_empty(),
            "证据出现之前的时刻，一条也不该成立：{jan_2023:?}"
        );

        // 2023-06-01：有起点的两条当天起成立；B 还没有证据；C 从不成立
        let jun_2023 = edges_at(&pool, &f, Some("2023-06-01T00:00:00Z")).await?;
        assert_eq!(jun_2023, HashSet::from([a, d]));

        // 2024-02-20：B 从它的文档日期起成立
        let feb_2024 = edges_at(&pool, &f, Some("2024-02-20T00:00:00Z")).await?;
        assert_eq!(feb_2024, HashSet::from([a, b, d]));
        let day_before = edges_at(&pool, &f, Some("2024-02-19T00:00:00Z")).await?;
        assert!(
            !day_before.contains(&b),
            "文档日期前一天，没起点的事实还不成立"
        );

        // 2025-10-15：D 到说出它结束的那份文档为止；前一天还在
        let oct_2025 = edges_at(&pool, &f, Some("2025-10-15T00:00:00Z")).await?;
        assert_eq!(
            oct_2025,
            HashSet::from([a, b]),
            "结束了不知哪天的，到它的文档日期为止"
        );
        let eve = edges_at(&pool, &f, Some("2025-10-14T00:00:00Z")).await?;
        assert!(
            eve.contains(&d),
            "文档日期前一天它还成立——保守包含到证据为止"
        );

        // 2026-01-01：#345 那道题——头衔不该还挂着
        let jan_2026 = edges_at(&pool, &f, Some("2026-01-01T00:00:00Z")).await?;
        assert!(
            !jan_2026.contains(&c) && !jan_2026.contains(&d),
            "原文说结束了，就不该在之后成立"
        );
        assert_eq!(jan_2026, HashSet::from([a, b]));

        // 两端都不知道的 C：任何时刻都不成立；但不给时刻时它在（画布画的是历史）
        let every = edges_at(&pool, &f, None).await?;
        assert_eq!(
            every,
            HashSet::from([a, b, c, d]),
            "at = None 是每一刻，四条都在"
        );

        // 边上带着读出来的区间，前端按它过滤，不自己解释 NULL
        let (_, edges) =
            utopia_store::graph::neighborhood(&pool, f.kb, f.lin, 1, None, None).await?;
        let by_id = |id: Uuid| edges.iter().find(|e| e.id == id).unwrap();
        assert_eq!(by_id(b).holds_from, Some(t("2024-02-20T00:00:00Z")));
        assert_eq!(by_id(b).holds_to, None);
        assert_eq!(by_id(d).holds_from, Some(t("2023-06-01T00:00:00Z")));
        assert_eq!(
            by_id(d).holds_to,
            Some(t("2025-10-15T00:00:00Z")),
            "结束端读到文档日期"
        );
        assert_eq!(by_id(a).holds_to, None, "仍在继续的还是开放的");
        assert_eq!(by_id(c).holds_from, Some(t("2025-10-15T00:00:00Z")));
        assert_eq!(
            by_id(c).holds_to,
            Some(t("2025-10-15T00:00:00Z")),
            "两端都不知道：区间为空"
        );

        // 实体面板：不给时刻时整条时间线都在（C 也列着，标着结束）；给时刻时按规则筛
        let (_, all) = utopia_store::graph::entity_detail(&pool, f.kb, f.lin, None, None).await?;
        assert_eq!(all.len(), 4);
        let (_, now_ish) = utopia_store::graph::entity_detail(
            &pool,
            f.kb,
            f.lin,
            Some(t("2026-01-01T00:00:00Z")),
            None,
        )
        .await?;
        let ids: HashSet<Uuid> = now_ish.iter().map(|x| x.id).collect();
        assert_eq!(ids, HashSet::from([a, b]));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 锚点只往早挪：更早的文档是更早的证据。修正行继承锚点：改区间是重述同一份证据。
#[tokio::test]
async fn the_anchor_moves_earlier_and_is_inherited() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let (b, created) = fact(
            &pool,
            &f,
            f.platform,
            f.approved,
            f.lin,
            Validity::default().attested(Some(t("2024-02-20T00:00:00Z"))),
        )
        .await?;
        assert!(created);
        assert!(!edges_at(&pool, &f, Some("2023-12-01T00:00:00Z"))
            .await?
            .contains(&b));

        // 同一断言，出自一份更早的文档（回填语料乱序进来是常态）：同一行，锚点前移
        let (again, created) = fact(
            &pool,
            &f,
            f.platform,
            f.approved,
            f.lin,
            Validity::default().attested(Some(t("2023-12-01T00:00:00Z"))),
        )
        .await?;
        assert_eq!(again, b, "同一断言复用同一行");
        assert!(!created);
        assert_eq!(attested_at(&pool, b).await?, t("2023-12-01T00:00:00Z"));
        assert!(edges_at(&pool, &f, Some("2023-12-01T00:00:00Z"))
            .await?
            .contains(&b));

        // 更晚的文档再提一次：什么也不动
        fact(
            &pool,
            &f,
            f.platform,
            f.approved,
            f.lin,
            Validity::default().attested(Some(t("2025-01-01T00:00:00Z"))),
        )
        .await?;
        assert_eq!(
            attested_at(&pool, b).await?,
            t("2023-12-01T00:00:00Z"),
            "只往早挪"
        );

        // 人给它一个结束：修正行继承锚点，而不是锚在修正的此刻
        let corrected = utopia_store::temporal::correct_interval(
            &pool,
            b,
            Validity {
                from: None,
                from_precision: None,
                to: Some(t("2025-06-01T00:00:00Z")),
                to_precision: Some("day"),
                attested_at: None,
            },
        )
        .await?
        .expect("改得动");
        assert_eq!(
            attested_at(&pool, corrected).await?,
            t("2023-12-01T00:00:00Z")
        );
        let (_, edges) =
            utopia_store::graph::neighborhood(&pool, f.kb, f.lin, 1, None, None).await?;
        let row = edges.iter().find(|e| e.id == corrected).unwrap();
        assert_eq!(row.holds_from, Some(t("2023-12-01T00:00:00Z")));
        assert_eq!(
            row.holds_to,
            Some(t("2025-06-01T00:00:00Z")),
            "原文给了终点就用终点"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 「不再担任，日期未记录」撞上「2023-06-01 起担任」的开放行：关上它，而不是另立
/// 一行让两条各说各话。#345 那道题挂的正是这个——结束的那行读对了，开放的那行
/// 照旧说「至今仍是」。
#[tokio::test]
async fn an_ending_without_a_date_closes_the_dated_row_it_ends() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 2023-06-01 的 offer：Staff Engineer，仍在继续
        let (open, _) = fact(
            &pool,
            &f,
            f.lin,
            f.holds_title,
            f.staff_engineer,
            Validity::starting(Some(t("2023-06-01T00:00:00Z")), Some("day"))
                .attested(Some(t("2023-06-01T00:00:00Z"))),
        )
        .await?;
        assert!(edges_at(&pool, &f, Some("2026-01-01T00:00:00Z"))
            .await?
            .contains(&open));

        // 2025-10-15 的角色说明：不再担任，日期未记录——同一断言，没起点
        let (closed, created) = fact(
            &pool,
            &f,
            f.lin,
            f.holds_title,
            f.staff_engineer,
            Validity::default()
                .ended_when_unknown()
                .attested(Some(t("2025-10-15T00:00:00Z"))),
        )
        .await?;
        assert!(created, "关上是一条修正行，不是并进去");
        assert_ne!(closed, open);

        // 旧行作废、修正行链上：起点是原文给的，终点空、精度 unknown，锚在结束的那份文档
        let (invalidated, supersedes): (Option<chrono::DateTime<chrono::Utc>>, Option<Uuid>) =
            sqlx::query_as("SELECT invalidated_at, supersedes FROM facts WHERE id = $1")
                .bind(open)
                .fetch_one(&pool)
                .await?;
        assert!(invalidated.is_some(), "旧行作废，不删");
        assert!(supersedes.is_none());
        let row: ClosedRow = sqlx::query_as(
            "SELECT valid_from, valid_to, valid_to_precision, supersedes, attested_at
               FROM facts WHERE id = $1",
        )
        .bind(closed)
        .fetch_one(&pool)
        .await?;
        assert_eq!(row.0, Some(t("2023-06-01T00:00:00Z")), "起点照旧");
        assert_eq!(row.1, None);
        assert_eq!(row.2.as_deref(), Some("unknown"));
        assert_eq!(row.3, Some(open), "supersedes 链上");
        assert_eq!(row.4, t("2025-10-15T00:00:00Z"), "锚在说出结束的那份文档");

        // #345 那道题：2026 年 1 月不该再有 Staff Engineer；2024 年 6 月还有
        assert!(!edges_at(&pool, &f, Some("2026-01-01T00:00:00Z"))
            .await?
            .contains(&closed));
        assert!(edges_at(&pool, &f, Some("2024-06-01T00:00:00Z"))
            .await?
            .contains(&closed));
        let (_, panel) = utopia_store::graph::entity_detail(
            &pool,
            f.kb,
            f.lin,
            Some(t("2026-01-01T00:00:00Z")),
            None,
        )
        .await?;
        assert!(panel.is_empty(), "面板在 2026 年也不再列它：{panel:?}");

        // 同一断言再说一次「结束了」：已经关上的行不再被关，也不另立
        let (again, created) = fact(
            &pool,
            &f,
            f.lin,
            f.holds_title,
            f.staff_engineer,
            Validity::default()
                .ended_when_unknown()
                .attested(Some(t("2025-12-01T00:00:00Z"))),
        )
        .await?;
        assert!(!created, "没有开放行可关，也不是新信息");
        assert_eq!(again, closed);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 派生读前提**读出来的**区间（0022 第 4 条）：一条 part_of 链里，没起点的那一环从它
/// 的证据日期起算，结束了不知哪天的那一环到说出它的那份文档为止。派生行的两端因此
/// 是前提共同支撑的那一段；来自锚点的那一端没有精度。
#[tokio::test]
async fn a_derivation_reads_no_further_than_its_premises() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // part_of 传递；四个组织 a ⊂ b ⊂ c ⊂ d
        let part_of = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, is_transitive)
             VALUES ($1, $2, 'part_of', 'part of', TRUE)",
        )
        .bind(part_of)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        let org_type: Uuid = sqlx::query_scalar("SELECT type_id FROM entities WHERE id = $1")
            .bind(f.meridian)
            .fetch_one(&pool)
            .await?;
        let mut ids = Vec::new();
        for name in ["Team A", "Group B", "Division C", "Company D"] {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(f.kb)
            .bind(org_type)
            .bind(name)
            .execute(&pool)
            .await?;
            ids.push(id);
        }
        let (a, b, c, d) = (ids[0], ids[1], ids[2], ids[3]);
        // a ⊂ b：原文给了起点 2022-01-01
        fact(
            &pool,
            &f,
            a,
            part_of,
            b,
            Validity::starting(Some(t("2022-01-01T00:00:00Z")), Some("day"))
                .attested(Some(t("2022-01-01T00:00:00Z"))),
        )
        .await?;
        // b ⊂ c：没起点，出自 2024-02-20 的文档
        fact(
            &pool,
            &f,
            b,
            part_of,
            c,
            Validity::default().attested(Some(t("2024-02-20T00:00:00Z"))),
        )
        .await?;
        // c ⊂ d：2020-01-01 起，2025-10-15 的文档说已不再（日期未记录）
        fact(
            &pool,
            &f,
            c,
            part_of,
            d,
            Validity::starting(Some(t("2020-01-01T00:00:00Z")), Some("day"))
                .ended_when_unknown()
                .attested(Some(t("2025-10-15T00:00:00Z"))),
        )
        .await?;

        utopia_store::reasoning::materialize(&pool, f.kb).await?;

        type DerivedRow = (
            Option<chrono::DateTime<chrono::Utc>>,
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<String>,
        );
        let derived = |s: Uuid, o: Uuid| {
            let pool = pool.clone();
            async move {
                let row: Option<DerivedRow> = sqlx::query_as(
                    "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision
                       FROM derived_facts
                      WHERE kb_id = $1 AND subject_id = $2 AND object_id = $3
                        AND invalidated_at IS NULL",
                )
                .bind(f.kb)
                .bind(s)
                .bind(o)
                .fetch_optional(&pool)
                .await?;
                Ok::<_, anyhow::Error>(row)
            }
        };

        // a ⊂ c：起点是 b ⊂ c 的证据日期（锚点），没有精度；终点开放
        let ac = derived(a, c).await?.expect("a ⊂ c 推得出");
        assert_eq!(
            ac.0,
            Some(t("2024-02-20T00:00:00Z")),
            "从没起点那一环的证据起算"
        );
        assert_eq!(ac.1, None, "来自锚点的一端没有精度");
        assert_eq!(ac.2, None);

        // a ⊂ d：起点同上；终点是 c ⊂ d 的锚点（说出结束的那份文档），也没有精度
        let ad = derived(a, d).await?.expect("a ⊂ d 推得出");
        assert_eq!(ad.0, Some(t("2024-02-20T00:00:00Z")));
        assert_eq!(ad.1, None);
        assert_eq!(
            ad.2,
            Some(t("2025-10-15T00:00:00Z")),
            "到结束的那份文档为止"
        );
        assert_eq!(ad.3, None, "'unknown' 不是粒度，锚点顶上来的一端没有精度");

        // b ⊂ d：起点是 b ⊂ c 的锚点、终点是 c ⊂ d 的锚点——两端都没有精度
        let bd = derived(b, d).await?.expect("b ⊂ d 推得出");
        assert_eq!(
            (bd.0, bd.2),
            (
                Some(t("2024-02-20T00:00:00Z")),
                Some(t("2025-10-15T00:00:00Z"))
            )
        );
        assert_eq!((bd.1, bd.3), (None, None));

        // 图上按读出来的区间亮灭：2023 年 a ⊂ c 还不存在；2024 年中在；2026 年 a ⊂ d 已灭
        // 从 b 看两跳：a ⊂ b 与 c ⊂ d 把 a、d 都收进来，派生边才都在「这些节点之间」
        let at = |when: &str| {
            let pool = pool.clone();
            let when = when.to_string();
            async move {
                let (_, edges) =
                    utopia_store::graph::neighborhood(&pool, f.kb, b, 2, Some(t(&when)), None)
                        .await?;
                Ok::<_, anyhow::Error>(
                    edges
                        .into_iter()
                        .filter(|e| e.derived)
                        .map(|e| (e.source, e.target))
                        .collect::<HashSet<_>>(),
                )
            }
        };
        assert!(!at("2023-06-01T00:00:00Z").await?.contains(&(a, c)));
        assert!(at("2024-06-01T00:00:00Z").await?.contains(&(a, c)));
        assert!(at("2024-06-01T00:00:00Z").await?.contains(&(a, d)));
        assert!(
            !at("2026-01-01T00:00:00Z").await?.contains(&(a, d)),
            "链经过已结束的一环，之后不成立"
        );
        assert!(
            at("2026-01-01T00:00:00Z").await?.contains(&(a, c)),
            "另一条链还开着"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
