//! `paths_between`（#558）：两点之间的链，短的在前，每条边都要在 `at` 成立，
//! 枢纽不穿越，记录轴照走。

use sqlx::PgPool;
use utopia_store::graph::Validity;
use utopia_store::paths::{paths_between, Limits, Path};
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

/// Acme 与 Beta 之间四条路：直连（2023 起）、经 Bob（Acme 雇员 2016–2020-12，
/// 2021 起创办 Beta）、经 Dan（雇员 2016–2019，2021 起创办 Beta）、经 Carol 和
/// Gamma（Carol 2018 起在 Acme，2019 起为 Gamma 顾问，Gamma 2020 起与 Beta 合作）
struct Fixture {
    org: Uuid,
    kb: Uuid,
    acme: Uuid,
    beta: Uuid,
    gamma: Uuid,
    bob: Uuid,
    carol: Uuid,
    dan: Uuid,
    employee: Uuid,
    founder: Uuid,
    partner: Uuid,
    advises: Uuid,
    direct: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (organization, person) = (Uuid::now_v7(), Uuid::now_v7());
    let (acme, beta, gamma) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (bob, carol, dan) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (employee, founder, partner, advises) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'path-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'path-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'path-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (organization, "organization", "Organization"),
        (person, "person", "Person"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, key) in [
        (employee, "employee"),
        (founder, "founder"),
        (partner, "partner"),
        (advises, "advises"),
    ] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal) VALUES ($1, $2, $3, $3, 'state')",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .execute(pool)
        .await?;
    }
    for (id, ty, name) in [
        (acme, organization, "Acme"),
        (beta, organization, "Beta"),
        (gamma, organization, "Gamma"),
        (bob, person, "Bob"),
        (carol, person, "Carol"),
        (dan, person, "Dan"),
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
    let span = |from: &str, to: Option<&str>| Validity {
        from: Some(t(from)),
        from_precision: Some("month"),
        to: to.map(t),
        to_precision: to.map(|_| "month"),
        attested_at: None,
        from_grade: None,
    };
    let mut direct = Uuid::nil();
    for (s, p, o, v) in [
        (
            acme,
            employee,
            bob,
            span("2016-01-01T00:00:00Z", Some("2020-12-01T00:00:00Z")),
        ),
        (bob, founder, beta, span("2021-01-01T00:00:00Z", None)),
        (acme, partner, beta, span("2023-01-01T00:00:00Z", None)),
        (acme, employee, carol, span("2018-01-01T00:00:00Z", None)),
        (carol, advises, gamma, span("2019-01-01T00:00:00Z", None)),
        (gamma, partner, beta, span("2020-01-01T00:00:00Z", None)),
        (
            acme,
            employee,
            dan,
            span("2016-01-01T00:00:00Z", Some("2019-01-01T00:00:00Z")),
        ),
        (dan, founder, beta, span("2021-01-01T00:00:00Z", None)),
        // 同一条边的第二份事实（只有锚点、没日期的那种）：路径不该因此多出一条
        (
            bob,
            founder,
            beta,
            Validity::starting(None, None).attested(Some(t("2024-05-01T00:00:00Z"))),
        ),
    ] {
        let (id, _) = utopia_store::graph::insert_fact(pool, kb, s, Some(p), o, v, 0.9).await?;
        if s == acme && p == partner {
            direct = id;
        }
    }
    Ok(Fixture {
        org,
        kb,
        acme,
        beta,
        gamma,
        bob,
        carol,
        dan,
        employee,
        founder,
        partner,
        advises,
        direct,
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

/// 路径读成「途经谁」，好断言
fn via(p: &Path) -> Vec<Uuid> {
    p.nodes[1..p.nodes.len() - 1].to_vec()
}

#[tokio::test]
async fn paths_come_shortest_first_and_stop_at_max_hops() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let run = async {
        let all = paths_between(&pool, f.kb, f.acme, f.beta, None, None, Limits::default()).await?;
        let hops: Vec<usize> = all.iter().map(Path::hops).collect();
        assert_eq!(
            hops,
            vec![1, 2, 2, 3],
            "短的在前：直连、两条两跳、一条三跳；Bob 那条边有两份事实，仍是一条路"
        );
        assert!(via(&all[0]).is_empty());
        let two: Vec<Vec<Uuid>> = all[1..3].iter().map(via).collect();
        assert!(two.contains(&vec![f.bob]) && two.contains(&vec![f.dan]));
        assert_eq!(via(&all[3]), vec![f.carol, f.gamma]);
        // 边自带两端与谓词，顺着走的边主语就是上一个节点
        let chain = &all[3];
        assert_eq!(chain.edges[0].subject_id, f.acme);
        assert_eq!(chain.edges[0].object_name, "Carol");
        assert_eq!(chain.edges[1].predicate.as_deref(), Some("advises"));
        assert_eq!(chain.edges[2].object_id, f.beta);

        let limits = |max_hops| Limits {
            max_hops,
            ..Limits::default()
        };
        let two = paths_between(&pool, f.kb, f.acme, f.beta, None, None, limits(2)).await?;
        assert_eq!(two.len(), 3, "两跳以内没有 Carol 那条");
        let one = paths_between(&pool, f.kb, f.acme, f.beta, None, None, limits(1)).await?;
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].edges[0].fact_id, f.direct);
        // 反向问同样走得通
        let back =
            paths_between(&pool, f.kb, f.beta, f.acme, None, None, Limits::default()).await?;
        assert_eq!(back.len(), 4);
        assert_eq!(back[0].nodes, vec![f.beta, f.acme]);
        anyhow::Ok(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 带 `at` 时每一条边都得在那一刻成立：2021 年中，Acme 到 Beta 只剩 Carol–Gamma 那条
#[tokio::test]
async fn a_path_holds_only_when_every_edge_holds() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let run = async {
        let mid_2021 = Some(t("2021-06-15T00:00:00Z"));
        let paths = paths_between(
            &pool,
            f.kb,
            f.acme,
            f.beta,
            mid_2021,
            None,
            Limits::default(),
        )
        .await?;
        assert_eq!(paths.len(), 1, "Bob 已离开、Dan 已离开、直连还没签");
        assert_eq!(via(&paths[0]), vec![f.carol, f.gamma]);

        let in_2018 = Some(t("2018-06-15T00:00:00Z"));
        let none = paths_between(
            &pool,
            f.kb,
            f.acme,
            f.beta,
            in_2018,
            None,
            Limits::default(),
        )
        .await?;
        assert!(none.is_empty(), "2018 年 Beta 还没被谁创办，什么都连不上");
        anyhow::Ok(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 枢纽不往下展：Carol 一多几条边就不再是三跳路径的中转
#[tokio::test]
async fn a_hub_is_not_walked_through() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let run = async {
        let tight = Limits {
            hub_degree: 2,
            ..Limits::default()
        };
        let before = paths_between(&pool, f.kb, f.acme, f.beta, None, None, tight).await?;
        assert_eq!(before.len(), 4, "Carol 两条边，不算枢纽");
        // Carol 再顾问三家：度数 5，超过上限
        for _ in 0..3 {
            let other = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'Other')",
            )
            .bind(other)
            .bind(f.kb)
            .execute(&pool)
            .await?;
            utopia_store::graph::insert_fact(
                &pool,
                f.kb,
                f.carol,
                Some(f.advises),
                other,
                Validity::starting(Some(t("2020-01-01T00:00:00Z")), Some("year")),
                0.9,
            )
            .await?;
        }
        let after = paths_between(&pool, f.kb, f.acme, f.beta, None, None, tight).await?;
        assert_eq!(after.len(), 3, "经 Carol 的三跳没了");
        assert!(after.iter().all(|p| !via(p).contains(&f.carol)));
        let loose =
            paths_between(&pool, f.kb, f.acme, f.beta, None, None, Limits::default()).await?;
        assert_eq!(loose.len(), 4, "默认上限下她还只是个普通节点");
        // 两跳路径不受枢纽限制：Bob 和 Dan 的度数不进这条判断，两端更不进
        let _ = (f.employee, f.founder, f.partner, f.bob, f.dan);
        anyhow::Ok(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

/// 记录轴：直连那条边今天才录入，其余前天就在；问「昨天的记录」走不到它，
/// 不带 as_of 是此刻的记录（记录轴上 NULL 即现在，0019），四条都在
#[tokio::test]
async fn a_path_reads_the_base_as_it_was() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let run = async {
        let two_days_ago = chrono::Utc::now() - chrono::Duration::days(2);
        sqlx::query("UPDATE facts SET recorded_at = $3 WHERE kb_id = $1 AND id <> $2")
            .bind(f.kb)
            .bind(f.direct)
            .bind(two_days_ago)
            .execute(&pool)
            .await?;
        let yesterday = Some(chrono::Utc::now() - chrono::Duration::days(1));
        let then = paths_between(
            &pool,
            f.kb,
            f.acme,
            f.beta,
            None,
            yesterday,
            Limits::default(),
        )
        .await?;
        assert_eq!(then.len(), 3, "直连那条今天才进账本");
        assert!(then.iter().all(|p| p.hops() >= 2));
        let today =
            paths_between(&pool, f.kb, f.acme, f.beta, None, None, Limits::default()).await?;
        assert_eq!(today.len(), 4, "不带 as_of 是此刻的记录");
        anyhow::Ok(())
    }
    .await;
    teardown(&pool, &f).await?;
    run
}

#[tokio::test]
async fn opposite_directions_remain_distinct_at_every_hop_count() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let result = async {
        let nodes = [Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7()];
        for node in nodes {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name,created_at) VALUES ($1,$2,$1::text,'2020-01-01')")
                .bind(node).bind(f.kb).execute(&pool).await?;
        }
        let mut facts = Vec::new();
        for (s, o, year, confidence) in [
            (nodes[0], nodes[1], "2021-01-01T00:00:00Z", 0.9),
            (nodes[0], nodes[1], "2022-01-01T00:00:00Z", 0.7),
            (nodes[1], nodes[0], "2021-01-01T00:00:00Z", 0.8),
            (nodes[1], nodes[2], "2021-01-01T00:00:00Z", 0.9),
            (nodes[2], nodes[3], "2021-01-01T00:00:00Z", 0.9),
        ] {
            facts.push(utopia_store::graph::insert_fact(&pool, f.kb, s, Some(f.partner), o,
                Validity::starting(Some(t(year)), Some("day")), confidence).await?.0);
        }
        sqlx::query("UPDATE facts SET recorded_at='2022-01-01' WHERE id=ANY($1)")
            .bind(&facts).execute(&pool).await?;
        let snapshot_sql = "SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM facts f WHERE kb_id=$1";
        let before: serde_json::Value = sqlx::query_scalar(snapshot_sql).bind(f.kb).fetch_one(&pool).await?;
        let at = Some(t("2023-01-01T00:00:00Z"));
        for hops in 1..=3 {
            for (from, to) in [(nodes[0], nodes[hops]), (nodes[hops], nodes[0])] {
                let limits = Limits { max_hops: hops, ..Limits::default() };
                let paths = paths_between(&pool, f.kb, from, to, at, at, limits).await?;
                anyhow::ensure!(paths.len() == 2, "{hops}-hop query lost an opposite direction: {paths:?}");
                let signatures: std::collections::HashSet<_> = paths.iter().map(|p|
                    p.edges.iter().map(|e| (e.subject_id, e.object_id)).collect::<Vec<_>>()
                ).collect();
                anyhow::ensure!(signatures.len() == 2, "duplicate direction survived");
                anyhow::ensure!(paths.iter().all(|p| !p.edges.iter().any(|e| e.fact_id == facts[1])),
                    "lower-confidence duplicate displaced the preferred observation");
                let capped = paths_between(&pool, f.kb, from, to, at, at,
                    Limits { max_paths: 1, ..limits }).await?;
                anyhow::ensure!(capped.len() == 1 && capped[0].edges.iter().map(|e| e.fact_id).collect::<Vec<_>>()
                    == paths[0].edges.iter().map(|e| e.fact_id).collect::<Vec<_>>(), "cap/ranking changed");
                anyhow::ensure!(paths_between(&pool, Uuid::now_v7(), from, to, at, at, limits).await?.is_empty(),
                    "cross-KB path escaped isolation");
            }
        }
        let after: serde_json::Value = sqlx::query_scalar(snapshot_sql).bind(f.kb).fetch_one(&pool).await?;
        anyhow::ensure!(before == after, "path reads changed facts");
        sqlx::query("UPDATE facts SET invalidated_at='2024-01-01' WHERE id=$1")
            .bind(facts[2]).execute(&pool).await?;
        let historic = paths_between(&pool, f.kb, nodes[0], nodes[1], at, at, Limits::default()).await?;
        let current = paths_between(&pool, f.kb, nodes[0], nodes[1], at,
            Some(t("2025-01-01T00:00:00Z")), Limits::default()).await?;
        anyhow::ensure!(historic.len() == 2 && current.len() == 1, "record-axis retraction changed");
        anyhow::ensure!(paths_between(&pool, f.kb, nodes[0], nodes[1],
            Some(t("2019-01-01T00:00:00Z")), at, Limits::default()).await?.is_empty(), "world-axis filter changed");
        Ok(())
    }.await;
    let cleanup = teardown(&pool, &f).await;
    result.and(cleanup)
}
