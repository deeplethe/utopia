//! 传递环要整条路径同一时刻成立（#636），打在真库上。
//!
//! 纯逻辑那部分——两两重叠而三者无交、剪枝丢不了真环、随机图对拍——在
//! `utopia-reason` 里跑。这里钉的是只有过一遍取数与落库才看得见的：
//!
//! 1. **时间上错开的环不进 Review；改对了时间，环就消失。** 人去把其中一条的结束
//!    日期补上（302 的 `correct_interval`，作废 + 改写），重跑那一行被清掉。从前改完
//!    照样算出同一个环——人按提示修了数据，队列却纹丝不动
//! 2. **区间是按库里的写法读出来的**（`read_span`），每一种写法都要走到：
//!    - 结束了不知哪天的，到说出它的那份证据为止（0022）——读成开放的，"former"
//!      一句话就能凭空凑出一个今天还成立的环
//!    - 没有起点的，从最早的证据起
//!    - 事件在它命名的那个桶里成立：同一天与同一个月相交，相邻两天不交；没日期的
//!      事件哪一刻都不成立（0031）
//!    - 恒常谓词没有时间可错开，照报；把谓词改成状态，重跑就清掉
//! 3. **同一对节点之间有两条边，一条从没与别的边同时成立**：环照样经由另一条被找到，
//!    路径上是那一条
//! 4. **裁决照旧**：公理放宽了而环仍同时成立，那行重开；认可过的重跑沉默；撤掉环上
//!    一条，环不再算出来，裁决留着
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::graph::Validity;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    etype: Uuid,
    user: Uuid,
    /// state + transitive
    part_of: Uuid,
    /// event + transitive
    merged_into: Uuid,
    /// eternal + transitive
    located_in: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, user) = (Uuid::now_v7(), Uuid::now_v7());
    let (part_of, merged_into, located_in) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'cycle-time-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'cycle-time-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'cycle-time-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, password_hash, display_name)
         VALUES ($1, $2, $3, '', 'Cycle Reviewer')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("cycle-{user}@test.local"))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, key, temporal) in [
        (part_of, "part_of", "state"),
        (merged_into, "merged_into", "event"),
        (located_in, "located_in", "eternal"),
    ] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, is_transitive)
             VALUES ($1, $2, $3, $3, $4, TRUE)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .bind(temporal)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        kb,
        etype,
        user,
        part_of,
        merged_into,
        located_in,
    })
}

async fn entity(pool: &PgPool, f: &Fixture, name: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.etype)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn entities<const N: usize>(pool: &PgPool, f: &Fixture) -> anyhow::Result<[Uuid; N]> {
    let mut out = [Uuid::nil(); N];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = entity(pool, f, &format!("E{i}")).await?;
    }
    Ok(out)
}

fn ts(d: &str) -> chrono::DateTime<chrono::Utc> {
    format!("{d}T00:00:00Z").parse().unwrap()
}

/// 一条事实在库里怎么写时间。日期给到精度对齐的那一天（CHECK 要求截断过）
#[derive(Clone, Copy, Default)]
struct When<'a> {
    /// (日期, 精度)
    from: Option<(&'a str, &'a str)>,
    to: Option<(&'a str, &'a str)>,
    /// 结束了，不知哪天：`valid_to_precision = 'unknown'`，锚在这份证据的日期
    ended_unknown_as_of: Option<&'a str>,
    /// 证据最早是哪天（`attested_from`）；不给就是此刻
    attested: Option<&'a str>,
}

fn span<'a>(from: &'a str, to: Option<&'a str>) -> When<'a> {
    When {
        from: Some((from, "day")),
        to: to.map(|t| (t, "day")),
        ..Default::default()
    }
}

async fn fact(
    pool: &PgPool,
    f: &Fixture,
    (s, p, o): (Uuid, Uuid, Uuid),
    w: When<'_>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    let to_precision = match (w.to, w.ended_unknown_as_of) {
        (Some((_, p)), _) => Some(p),
        (None, Some(_)) => Some("unknown"),
        (None, None) => None,
    };
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                            valid_from, valid_from_precision, valid_to, valid_to_precision,
                            attested_from, attested_to)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, COALESCE($10, now()), $11)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(s)
    .bind(p)
    .bind(o)
    .bind(w.from.map(|(d, _)| ts(d)))
    .bind(w.from.map(|(_, p)| p))
    .bind(w.to.map(|(d, _)| ts(d)))
    .bind(to_precision)
    .bind(w.attested.or(w.ended_unknown_as_of).map(ts))
    .bind(w.ended_unknown_as_of.map(ts))
    .execute(pool)
    .await?;
    Ok(id)
}

/// open 的环，每个是排过序的事实集合
async fn open_cycles(pool: &PgPool, f: &Fixture) -> anyhow::Result<Vec<Vec<Uuid>>> {
    let rows: Vec<Vec<Uuid>> = sqlx::query_scalar(
        "SELECT path FROM axiom_violations
          WHERE kb_id = $1 AND kind = 'cycle' AND status = 'open'",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<Vec<Uuid>> = rows.into_iter().map(sorted).collect();
    out.sort();
    Ok(out)
}

fn sorted(mut v: Vec<Uuid>) -> Vec<Uuid> {
    v.sort();
    v
}

/// 起库、跑、不管成败都拆
macro_rules! with_fixture {
    (|$pool:ident, $f:ident| $body:block) => {{
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(());
        };
        let $pool = PgPool::connect(&url).await?;
        let fixture = seed(&$pool).await?;
        let result = {
            let $f = &fixture;
            let $pool = &$pool;
            async move { $body }.await
        };
        // 先拆库：裁决行的 decided_by 指着这个组织的用户，而删组织时用户与库的
        // 级联谁先谁后不保证
        sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
            .bind(fixture.kb)
            .execute(&$pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(fixture.org)
            .execute(&$pool)
            .await?;
        result
    }};
}

#[tokio::test]
async fn a_cycle_is_one_moment_and_fixing_the_time_clears_it() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        // ---- 一、时间上错开的环：两段前后接上，三段两两重叠而三者无交
        let [a, b, d, e, g] = entities(pool, f).await?;
        fact(
            pool,
            f,
            (a, f.part_of, b),
            span("2019-01-01", Some("2021-01-01")),
        )
        .await?;
        fact(pool, f, (b, f.part_of, a), span("2022-01-01", None)).await?;
        fact(
            pool,
            f,
            (d, f.part_of, e),
            span("2020-01-01", Some("2022-01-01")),
        )
        .await?;
        fact(
            pool,
            f,
            (e, f.part_of, g),
            span("2021-01-01", Some("2023-01-01")),
        )
        .await?;
        fact(
            pool,
            f,
            (g, f.part_of, d),
            span("2022-01-01", Some("2024-01-01")),
        )
        .await?;

        let r = reasoning::run(pool, f.kb).await?;
        assert_eq!(r.edges, 5);
        assert!(
            open_cycles(pool, f).await?.is_empty(),
            "没有哪一刻整条成立的环不该进 Review"
        );

        // ---- 二、真正同时成立的环照报
        let [x, y] = entities(pool, f).await?;
        let xy = fact(pool, f, (x, f.part_of, y), span("2020-01-01", None)).await?;
        let yx = fact(pool, f, (y, f.part_of, x), span("2021-06-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        assert_eq!(open_cycles(pool, f).await?, vec![sorted(vec![xy, yx])]);

        // ---- 三、人去把 X part_of Y 的结束日期补上：它 2021-06-01 结束，正是
        // Y part_of X 开始的那天。改完重跑，那一行被清掉
        utopia_store::temporal::correct_interval(
            pool,
            xy,
            Validity {
                from: Some(ts("2020-01-01")),
                from_precision: Some("day"),
                to: Some(ts("2021-06-01")),
                to_precision: Some("day"),
                ..Default::default()
            },
        )
        .await?
        .expect("这条还活着，应当改得动");
        let after = reasoning::run(pool, f.kb).await?;
        assert!(
            open_cycles(pool, f).await?.is_empty(),
            "改对了时间，环就不在了"
        );
        assert_eq!(after.cleared, 1, "上一轮那一行是陈的");
        Ok::<_, anyhow::Error>(())
    })
}

/// "former" 的那条读到证据为止。读成开放的话，一条早已结束、只是没写哪天结束的边，
/// 会与今天才开始的反向边凑成一个环
#[tokio::test]
async fn an_unknown_end_stops_where_the_evidence_does() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        let former = When {
            from: Some(("2019-01-01", "day")),
            ended_unknown_as_of: Some("2020-06-01"),
            ..Default::default()
        };

        // 证据 2020-06 说它已经结束；反向边 2021 年才开始
        let [a, b] = entities(pool, f).await?;
        fact(pool, f, (a, f.part_of, b), former).await?;
        fact(pool, f, (b, f.part_of, a), span("2021-01-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        assert!(
            open_cycles(pool, f).await?.is_empty(),
            "结束了不知哪天的不是还开着"
        );

        // 反向边 2020 年初就开始了：在证据之前，两条确实同时成立过
        let [c, d] = entities(pool, f).await?;
        let cd = fact(pool, f, (c, f.part_of, d), former).await?;
        let dc = fact(pool, f, (d, f.part_of, c), span("2020-01-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        assert_eq!(open_cycles(pool, f).await?, vec![sorted(vec![cd, dc])]);
        Ok::<_, anyhow::Error>(())
    })
}

/// 没写起点的从最早的证据起，不是从来如此
#[tokio::test]
async fn a_missing_start_reads_from_the_evidence() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        let since = |d| When {
            attested: Some(d),
            ..Default::default()
        };

        // 2023 年的证据说 A 属于 B（没说从哪天起）；B 属于 A 在 2022 年就结束了
        let [a, b] = entities(pool, f).await?;
        fact(pool, f, (a, f.part_of, b), since("2023-01-01")).await?;
        fact(
            pool,
            f,
            (b, f.part_of, a),
            span("2020-01-01", Some("2022-01-01")),
        )
        .await?;
        reasoning::run(pool, f.kb).await?;
        assert!(open_cycles(pool, f).await?.is_empty());

        // 证据是 2021 年的：落在反向边的区间里
        let [c, d] = entities(pool, f).await?;
        let cd = fact(pool, f, (c, f.part_of, d), since("2021-01-01")).await?;
        let dc = fact(
            pool,
            f,
            (d, f.part_of, c),
            span("2020-01-01", Some("2022-01-01")),
        )
        .await?;
        reasoning::run(pool, f.kb).await?;
        assert_eq!(open_cycles(pool, f).await?, vec![sorted(vec![cd, dc])]);
        Ok::<_, anyhow::Error>(())
    })
}

/// 事件在它命名的桶里成立：2024-03-05 那天与 2024 年 3 月相交，与 3 月 6 日不交。
/// 没日期的事件哪一刻都不成立
#[tokio::test]
async fn an_event_holds_in_the_bucket_it_names() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        let on = |d, p| When {
            from: Some((d, p)),
            ..Default::default()
        };

        let [a, b] = entities(pool, f).await?;
        let ab = fact(pool, f, (a, f.merged_into, b), on("2024-03-05", "day")).await?;
        let ba = fact(pool, f, (b, f.merged_into, a), on("2024-03-01", "month")).await?;

        let [c, d] = entities(pool, f).await?;
        fact(pool, f, (c, f.merged_into, d), on("2024-03-05", "day")).await?;
        fact(pool, f, (d, f.merged_into, c), on("2024-03-06", "day")).await?;

        let [e, g] = entities(pool, f).await?;
        fact(pool, f, (e, f.merged_into, g), When::default()).await?;
        fact(pool, f, (g, f.merged_into, e), When::default()).await?;

        reasoning::run(pool, f.kb).await?;
        assert_eq!(
            open_cycles(pool, f).await?,
            vec![sorted(vec![ab, ba])],
            "只有同一天落在同一个月里的那一对成环"
        );
        Ok::<_, anyhow::Error>(())
    })
}

/// 恒常谓词没有时间可以错开：日期写了也不看，照报。把谓词改成状态，重跑就清掉；
/// 改回来，环又回来
#[tokio::test]
async fn an_eternal_predicate_has_no_time_to_tell_apart() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        let [a, b] = entities(pool, f).await?;
        let ab = fact(
            pool,
            f,
            (a, f.located_in, b),
            span("2019-01-01", Some("2021-01-01")),
        )
        .await?;
        let ba = fact(pool, f, (b, f.located_in, a), span("2022-01-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        assert_eq!(open_cycles(pool, f).await?, vec![sorted(vec![ab, ba])]);

        let set = |t: &'static str| {
            sqlx::query("UPDATE relation_types SET temporal = $1 WHERE id = $2")
                .bind(t)
                .bind(f.located_in)
        };
        set("state").execute(pool).await?;
        let r = reasoning::run(pool, f.kb).await?;
        assert!(open_cycles(pool, f).await?.is_empty(), "成了状态，就看日期");
        assert_eq!(r.cleared, 1);

        set("eternal").execute(pool).await?;
        reasoning::run(pool, f.kb).await?;
        assert_eq!(open_cycles(pool, f).await?, vec![sorted(vec![ab, ba])]);
        Ok::<_, anyhow::Error>(())
    })
}

/// 同一对节点之间两条边：旧的那条从没与环上别的边同时成立，新的那条成立。环经由
/// 新的那条被找到——先碰到旧的那条、被剪掉，不能让整个环跟着丢
#[tokio::test]
async fn a_parallel_edge_that_never_met_does_not_hide_the_one_that_did() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        let [a, b, c] = entities(pool, f).await?;
        // 先插旧的，顺序扫描先读到它
        fact(
            pool,
            f,
            (a, f.part_of, b),
            span("2019-01-01", Some("2020-01-01")),
        )
        .await?;
        let ab = fact(pool, f, (a, f.part_of, b), span("2021-01-01", None)).await?;
        let bc = fact(pool, f, (b, f.part_of, c), span("2021-01-01", None)).await?;
        let ca = fact(pool, f, (c, f.part_of, a), span("2021-01-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        assert_eq!(open_cycles(pool, f).await?, vec![sorted(vec![ab, bc, ca])]);
        Ok::<_, anyhow::Error>(())
    })
}

/// 裁决照旧：放宽了公理而环仍同时成立，重开；认可过的沉默；撤掉一条，环不再算出来
#[tokio::test]
async fn decisions_on_a_cycle_still_hold() -> anyhow::Result<()> {
    with_fixture!(|pool, f| {
        let [x, y] = entities(pool, f).await?;
        let xy = fact(pool, f, (x, f.part_of, y), span("2020-01-01", None)).await?;
        fact(pool, f, (y, f.part_of, x), span("2021-01-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        let id = || async {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM axiom_violations WHERE kb_id = $1 AND kind = 'cycle'",
            )
            .bind(f.kb)
            .fetch_one(pool)
            .await
        };

        reasoning::decide(pool, f.kb, id().await?, "axiom_relaxed", f.user).await?;
        let r = reasoning::run(pool, f.kb).await?;
        assert_eq!(r.reopened, 1, "公理没真放宽，环还同时成立，那一行回到 open");

        reasoning::decide(pool, f.kb, id().await?, "accepted", f.user).await?;
        let r = reasoning::run(pool, f.kb).await?;
        assert_eq!((r.inserted, r.reopened), (0, 0), "认可过的重跑沉默");
        assert!(open_cycles(pool, f).await?.is_empty());

        // 另一个环：撤掉其中一条
        let [p, q] = entities(pool, f).await?;
        let pq = fact(pool, f, (p, f.part_of, q), span("2020-01-01", None)).await?;
        fact(pool, f, (q, f.part_of, p), span("2020-06-01", None)).await?;
        reasoning::run(pool, f.kb).await?;
        let open: Uuid = sqlx::query_scalar(
            "SELECT id FROM axiom_violations WHERE kb_id = $1 AND kind = 'cycle' AND status = 'open'",
        )
        .bind(f.kb)
        .fetch_one(pool)
        .await?;
        reasoning::retract_from_violation(pool, f.kb, open, Some(pq), f.user).await?;
        let r = reasoning::run(pool, f.kb).await?;
        assert_eq!(r.reopened, 0, "撤掉之后环不再算出来，承诺兑现了");
        assert!(open_cycles(pool, f).await?.is_empty());
        let resolved: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'cycle' AND status = 'resolved'",
        )
        .bind(f.kb)
        .fetch_one(pool)
        .await?;
        assert_eq!(resolved, 2, "两次裁决都留着");
        let _ = xy;
        Ok::<_, anyhow::Error>(())
    })
}
