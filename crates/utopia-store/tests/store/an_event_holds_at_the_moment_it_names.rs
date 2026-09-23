//! 事件在它命名的那一刻成立，恒常每一刻都成立（0031 / #486），打在真库上。
//!
//! 为什么非要连库：写入侧的归一（`Validity::under`）和读出侧的谓词（`world_axis`）
//! 各在一处，两处必须说同一句话；而读出侧整个活在 SQL 字符串里——一个写反的 CASE、
//! 一个拼错的 interval 字面量，`cargo check` 都看不见。
//!
//! 钉住的事：
//! - 事件写进库是两端同一个值；在它命名的那个桶里成立（那一天、那个月），桶外不成立
//! - 给事件一段、只给终点、说它「结束了不知哪天」——落下的仍是一刻或没有日期
//! - 没日期的事件任何时刻都不成立，但实体上仍列着它；之后来了日期就精化，再来一句
//!   没日期的并进去
//! - 恒常抹掉原文里的日期，证据日期也不闸它——1800 年和 2100 年都成立
//! - 0031 之前写下的事件行（终点空）按起点那个桶读，不必回填
//! - 人改一个事件的区间，落下的仍是一刻
//! - 派生经过事件取它的桶（两端带精度），经过恒常两端开放
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use std::collections::HashSet;

use sqlx::PgPool;
use utopia_store::graph::Validity;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

/// 一行事实的两端与精度
type Span = (
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
);

struct Fixture {
    org: Uuid,
    kb: Uuid,
    nova: Uuid,
    orion: Uuid,
    vega: Uuid,
    paris: Uuid,
    france: Uuid,
    /// 状态：领导某组织
    leads: Uuid,
    /// 事件：收购
    acquired: Uuid,
    /// 事件，对称：合并
    merged_with: Uuid,
    /// 恒常：首都
    capital_of: Uuid,
    /// 恒常，对称：接壤
    borders: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (organization, city, country) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (nova, orion, vega) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (paris, france) = (Uuid::now_v7(), Uuid::now_v7());
    let (leads, acquired, merged_with) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (capital_of, borders) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'event-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'event-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'event-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (organization, "organization", "Organization"),
        (city, "city", "City"),
        (country, "country", "Country"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, key, temporal, symmetric) in [
        (leads, "leads", "state", false),
        (acquired, "acquired", "event", false),
        (merged_with, "merged_with", "event", true),
        (capital_of, "capital_of", "eternal", false),
        (borders, "borders", "eternal", true),
    ] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, is_symmetric)
             VALUES ($1, $2, $3, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .bind(temporal)
        .bind(symmetric)
        .execute(pool)
        .await?;
    }
    for (id, ty, name) in [
        (nova, organization, "Nova Systems"),
        (orion, organization, "Orion Labs"),
        (vega, organization, "Vega Analytics"),
        (paris, city, "Paris"),
        (france, country, "France"),
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
        nova,
        orion,
        vega,
        paris,
        france,
        leads,
        acquired,
        merged_with,
        capital_of,
        borders,
    })
}

async fn teardown(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
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

async fn span(pool: &PgPool, id: Uuid) -> anyhow::Result<Span> {
    Ok(sqlx::query_as(
        "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision FROM facts WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

/// T 时刻某实体面板上的事实（按 id）。`None` = 每一刻
async fn facts_at(
    pool: &PgPool,
    f: &Fixture,
    entity: Uuid,
    at: Option<&str>,
) -> anyhow::Result<HashSet<Uuid>> {
    let (_, facts) =
        utopia_store::graph::entity_detail(pool, f.kb, entity, at.map(t), None).await?;
    Ok(facts.into_iter().map(|x| x.id).collect())
}

async fn holds(
    pool: &PgPool,
    f: &Fixture,
    entity: Uuid,
    id: Uuid,
    at: &str,
) -> anyhow::Result<bool> {
    Ok(facts_at(pool, f, entity, Some(at)).await?.contains(&id))
}

#[tokio::test]
async fn an_event_is_one_moment_and_holds_through_the_bucket_it_names() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 2024-04-01 的文档说：Nova 于 2024-03-15 收购 Orion
        let (day, _) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.orion,
            Validity::starting(Some(t("2024-03-15T00:00:00Z")), Some("day"))
                .attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;
        assert_eq!(
            span(&pool, day).await?,
            (
                Some(t("2024-03-15T00:00:00Z")),
                Some("day".into()),
                Some(t("2024-03-15T00:00:00Z")),
                Some("day".into()),
            ),
            "事件写进库是两端同一个值、同一个精度"
        );
        // 那一天里的每一刻成立；前一天、后一天都不；证据日期（4 月）更不
        assert!(!holds(&pool, &f, f.nova, day, "2024-03-14T23:59:59Z").await?);
        assert!(holds(&pool, &f, f.nova, day, "2024-03-15T00:00:00Z").await?);
        assert!(holds(&pool, &f, f.nova, day, "2024-03-15T23:59:59Z").await?);
        assert!(!holds(&pool, &f, f.nova, day, "2024-03-16T00:00:00Z").await?);
        assert!(
            !holds(&pool, &f, f.nova, day, "2024-04-01T00:00:00Z").await?,
            "证据日期不是事件的日期，状态那套「从证据起」不适用"
        );
        assert!(
            facts_at(&pool, &f, f.nova, None).await?.contains(&day),
            "不限时刻时列出"
        );

        // 读出来的区间跟着桶走：从那一天起，到下一天为止
        let (_, all) = utopia_store::graph::entity_detail(&pool, f.kb, f.nova, None, None).await?;
        let read = all.iter().find(|x| x.id == day).expect("在面板上");
        assert_eq!(read.holds_from, Some(t("2024-03-15T00:00:00Z")));
        assert_eq!(read.holds_to, Some(t("2024-03-16T00:00:00Z")));
        assert_eq!(read.temporal.as_deref(), Some("event"));

        // 月精度的桶是整个月
        let (month, _) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.vega,
            Validity::starting(Some(t("2024-06-01T00:00:00Z")), Some("month")),
        )
        .await?;
        assert!(holds(&pool, &f, f.nova, month, "2024-06-30T12:00:00Z").await?);
        assert!(!holds(&pool, &f, f.nova, month, "2024-07-01T00:00:00Z").await?);

        // 状态照旧：领导从 3 月起、开放——4 月也成立
        let (lead, _) = fact(
            &pool,
            &f,
            f.nova,
            f.leads,
            f.orion,
            Validity::starting(Some(t("2024-03-15T00:00:00Z")), Some("day")),
        )
        .await?;
        assert!(holds(&pool, &f, f.nova, lead, "2024-04-01T00:00:00Z").await?);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_span_an_ending_or_an_unknown_end_given_to_an_event_collapses() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 模型给了一段：取起点
        let (ranged, _) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.orion,
            Validity {
                from: Some(t("2024-03-15T00:00:00Z")),
                from_precision: Some("day"),
                to: Some(t("2025-01-01T00:00:00Z")),
                to_precision: Some("day"),
                attested_at: None,
                from_grade: None,
            },
        )
        .await?;
        let s = span(&pool, ranged).await?;
        assert_eq!(
            (s.0, s.2),
            (
                Some(t("2024-03-15T00:00:00Z")),
                Some(t("2024-03-15T00:00:00Z"))
            )
        );

        // 只给了终点：那就是它发生的时候
        let (ended, _) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.vega,
            Validity {
                from: None,
                from_precision: None,
                to: Some(t("2024-05-01T00:00:00Z")),
                to_precision: Some("month"),
                attested_at: None,
                from_grade: None,
            },
        )
        .await?;
        assert_eq!(
            span(&pool, ended).await?,
            (
                Some(t("2024-05-01T00:00:00Z")),
                Some("month".into()),
                Some(t("2024-05-01T00:00:00Z")),
                Some("month".into()),
            )
        );

        // 「结束了不知哪天」对一刻没有意义：落下的是没日期的事件，不是待关的行
        let (unknown, _) = fact(
            &pool,
            &f,
            f.orion,
            f.acquired,
            f.vega,
            Validity::default()
                .ended_when_unknown()
                .attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;
        assert_eq!(span(&pool, unknown).await?, (None, None, None, None));
        let attested_to: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT attested_to FROM facts WHERE id = $1")
                .bind(unknown)
                .fetch_one(&pool)
                .await?;
        assert_eq!(attested_to, None, "没有「结束」这回事，也就没有结束的锚点");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn an_undated_event_holds_at_no_moment_and_takes_a_date_when_one_arrives(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 2024-04-01 的文档说 Nova 收购了 Orion，没说哪天
        let (bare, created) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.orion,
            Validity::default().attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;
        assert!(created);
        assert!(
            facts_at(&pool, &f, f.nova, None).await?.contains(&bare),
            "实体上列着"
        );
        for at in [
            "2024-04-01T00:00:00Z",
            "2024-04-02T00:00:00Z",
            "2030-01-01T00:00:00Z",
        ] {
            assert!(
                !holds(&pool, &f, f.nova, bare, at).await?,
                "不知何时发生的事件，{at} 不算成立（状态会从证据起算，事件不）"
            );
        }

        // 后来的文档给了日期：精化——裸行作废，新行两端是那一刻
        let (dated, created) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.orion,
            Validity::starting(Some(t("2024-03-15T00:00:00Z")), Some("day"))
                .attested(Some(t("2024-05-01T00:00:00Z"))),
        )
        .await?;
        assert!(created);
        assert_ne!(dated, bare);
        let (supersedes, old_dead): (Option<Uuid>, bool) = sqlx::query_as(
            "SELECT supersedes, (SELECT invalidated_at IS NOT NULL FROM facts WHERE id = $2)
               FROM facts WHERE id = $1",
        )
        .bind(dated)
        .bind(bare)
        .fetch_one(&pool)
        .await?;
        assert_eq!(supersedes, Some(bare));
        assert!(old_dead);
        assert!(holds(&pool, &f, f.nova, dated, "2024-03-15T12:00:00Z").await?);

        // 再听到一句没日期的「收购了」：并进已有的那一刻，不是第二次收购
        let (again, created) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.orion,
            Validity::default().attested(Some(t("2024-06-01T00:00:00Z"))),
        )
        .await?;
        assert!(!created);
        assert_eq!(again, dated);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn an_eternal_relation_drops_its_dates_and_holds_at_every_moment() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 原文：「1990 年起巴黎是法国首都」——日期说的是别的事；文档是 2024 年的
        let (cap, _) = fact(
            &pool,
            &f,
            f.paris,
            f.capital_of,
            f.france,
            Validity::starting(Some(t("1990-01-01T00:00:00Z")), Some("year"))
                .attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;
        assert_eq!(
            span(&pool, cap).await?,
            (None, None, None, None),
            "恒常不存日期"
        );
        for at in [
            "1800-01-01T00:00:00Z",
            "2023-12-31T00:00:00Z",
            "2100-01-01T00:00:00Z",
        ] {
            assert!(
                holds(&pool, &f, f.paris, cap, at).await?,
                "{at} 成立——证据日期不闸恒常"
            );
        }
        let (_, all) = utopia_store::graph::entity_detail(&pool, f.kb, f.paris, None, None).await?;
        let read = all.iter().find(|x| x.id == cap).expect("在面板上");
        assert_eq!((read.holds_from, read.holds_to), (None, None), "两端开放");

        // 再说一遍并进同一行
        let (again, created) = fact(
            &pool,
            &f,
            f.paris,
            f.capital_of,
            f.france,
            Validity::starting(Some(t("2000-01-01T00:00:00Z")), Some("year")),
        )
        .await?;
        assert!(!created);
        assert_eq!(again, cap);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_row_written_before_the_rule_reads_as_its_start_bucket() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 0031 之前的形状：事件谓词、起点有、终点空——当年被读成「从那天起一直如此」
        let legacy = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                valid_from, valid_from_precision, confidence, attested_from)
             VALUES ($1, $2, $3, $4, $5, '2024-03-15T00:00:00Z', 'day', 0.9, '2024-04-01T00:00:00Z')",
        )
        .bind(legacy)
        .bind(f.kb)
        .bind(f.nova)
        .bind(f.acquired)
        .bind(f.orion)
        .execute(&pool)
        .await?;
        assert!(holds(&pool, &f, f.nova, legacy, "2024-03-15T12:00:00Z").await?);
        assert!(!holds(&pool, &f, f.nova, legacy, "2024-03-16T00:00:00Z").await?);
        assert!(!holds(&pool, &f, f.nova, legacy, "2025-01-01T00:00:00Z").await?, "不再是开放的");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_person_correcting_an_event_leaves_it_one_moment() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let (original, _) = fact(
            &pool,
            &f,
            f.nova,
            f.acquired,
            f.orion,
            Validity::starting(Some(t("2024-03-15T00:00:00Z")), Some("day")),
        )
        .await?;
        // 人在面板上填了一段 3-20 ~ 6-01：落下的是 3-20 这一刻
        let corrected = utopia_store::temporal::correct_interval(
            &pool,
            original,
            Validity {
                from: Some(t("2024-03-20T00:00:00Z")),
                from_precision: Some("day"),
                to: Some(t("2024-06-01T00:00:00Z")),
                to_precision: Some("day"),
                attested_at: None,
                from_grade: None,
            },
        )
        .await?
        .expect("改得动");
        assert_eq!(
            span(&pool, corrected).await?,
            (
                Some(t("2024-03-20T00:00:00Z")),
                Some("day".into()),
                Some(t("2024-03-20T00:00:00Z")),
                Some("day".into()),
            )
        );
        assert!(holds(&pool, &f, f.nova, corrected, "2024-03-20T08:00:00Z").await?);
        assert!(!holds(&pool, &f, f.nova, corrected, "2024-04-15T00:00:00Z").await?);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_derivation_reads_an_event_as_its_bucket_and_an_eternal_as_open() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // merged_with 对称、事件：Nova 与 Orion 于 2024-03 合并
        fact(
            &pool,
            &f,
            f.nova,
            f.merged_with,
            f.orion,
            Validity::starting(Some(t("2024-03-01T00:00:00Z")), Some("month"))
                .attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;
        // borders 对称、恒常：法国与……巴黎接壤（测试用的荒唐事实，文档是 2024 年的）
        fact(
            &pool,
            &f,
            f.france,
            f.borders,
            f.paris,
            Validity::starting(Some(t("2020-01-01T00:00:00Z")), Some("year"))
                .attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;
        // 没日期的事件：Orion 与 Vega 合并，不知何时——推不出反向那条
        fact(
            &pool,
            &f,
            f.orion,
            f.merged_with,
            f.vega,
            Validity::default().attested(Some(t("2024-04-01T00:00:00Z"))),
        )
        .await?;

        utopia_store::reasoning::materialize(&pool, f.kb).await?;

        let derived = |s: Uuid, p: Uuid, o: Uuid| {
            let pool = pool.clone();
            async move {
                let row: Option<Span> = sqlx::query_as(
                    "SELECT valid_from, valid_from_precision, valid_to, valid_to_precision
                       FROM derived_facts
                      WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND object_id = $4
                        AND invalidated_at IS NULL",
                )
                .bind(f.kb)
                .bind(s)
                .bind(p)
                .bind(o)
                .fetch_optional(&pool)
                .await?;
                Ok::<_, anyhow::Error>(row)
            }
        };

        // 反向的合并：那一个月，两端带月精度
        let back = derived(f.orion, f.merged_with, f.nova)
            .await?
            .expect("对称推得出");
        assert_eq!(
            back,
            (
                Some(t("2024-03-01T00:00:00Z")),
                Some("month".into()),
                Some(t("2024-04-01T00:00:00Z")),
                Some("month".into()),
            ),
            "派生取事件的桶：从 3 月起，到 4 月为止"
        );
        // 反向的接壤：两端开放，证据日期不进来
        let back = derived(f.paris, f.borders, f.france)
            .await?
            .expect("对称推得出");
        assert_eq!(back, (None, None, None, None));
        // 不知何时的合并：空区间，不推
        assert!(
            derived(f.vega, f.merged_with, f.orion).await?.is_none(),
            "没日期的事件任何时刻都不成立，链经过它推不出东西"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    teardown(&pool, &f).await?;
    run
}
