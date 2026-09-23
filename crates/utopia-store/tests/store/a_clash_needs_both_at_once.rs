//! 互斥违规（asymmetry、functional、inverse_functional）打在真库上（#624、#634）。
//!
//! 纯逻辑那部分——全排列下同一个键、首尾相接不算、连通块成组——在 `utopia-reason`
//! 里跑。这里钉的是只有过一遍取数与落库才看得见的四样：
//!
//! 1. **接任不进 Review。** 从前 `run` 把区间剥掉再查：一次调薪、一次换负责人、一次
//!    上下级对调，各进一行。区间是 `timed_edges` 从库里读出来的，这一段纯逻辑测不到
//! 2. **宾语侧记成自己的种类。** 从前落库是 `functional`，人照着去主语那一侧找
//! 3. **换一个取数顺序，裁决照样接得上。** 这是 #624 报的那个序列：裁成
//!    `axiom_relaxed`，公理与事实都没变，只是堆里的顺序变了，重跑——从前插了一行新的
//!    open、原来那行不重开
//! 4. **认可管的是它见过的那几条。** 组里多一条，要人再看；那条撤了，认可照样管着
//!    剩下的，哪怕组的首尾（也就是键）已经变了
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    etype: Uuid,
    /// state + functional
    salary: Uuid,
    /// state + inverse_functional
    leads: Uuid,
    /// state + asymmetric
    reports_to: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb, etype) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (salary, leads, reports_to) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'clash-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'clash-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'clash-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, key, functional, inverse, asymmetric) in [
        (salary, "salary", true, false, false),
        (leads, "leads", false, true, false),
        (reports_to, "reports_to", false, false, true),
    ] {
        sqlx::query(
            "INSERT INTO relation_types
                 (id, kb_id, key, label, temporal, functional, inverse_functional, is_asymmetric)
             VALUES ($1, $2, $3, $3, 'state', $4, $5, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .bind(functional)
        .bind(inverse)
        .bind(asymmetric)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        kb,
        etype,
        salary,
        leads,
        reports_to,
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

/// 一条关系事实，区间 `[from, to)`，按天。两端都不给就是「从证据那天起一直成立」
async fn fact(
    pool: &PgPool,
    f: &Fixture,
    (s, p, o): (Uuid, Uuid, Uuid),
    from: Option<&str>,
    to: Option<&str>,
) -> anyhow::Result<Uuid> {
    let day = |d: Option<&str>| {
        d.map(|d| {
            format!("{d}T00:00:00Z")
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap()
        })
    };
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                            valid_from, valid_from_precision, valid_to, valid_to_precision)
         VALUES ($1, $2, $3, $4, $5,
                 $6, CASE WHEN $6 IS NULL THEN NULL ELSE 'day' END,
                 $7, CASE WHEN $7 IS NULL THEN NULL ELSE 'day' END)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(s)
    .bind(p)
    .bind(o)
    .bind(day(from))
    .bind(day(to))
    .execute(pool)
    .await?;
    Ok(id)
}

/// open 的违规：(kind, left, right, path)
async fn open(pool: &PgPool, f: &Fixture) -> anyhow::Result<Vec<(String, Uuid, Uuid, Vec<Uuid>)>> {
    Ok(sqlx::query_as(
        "SELECT kind, left_fact, right_fact, path FROM axiom_violations
          WHERE kb_id = $1 AND status = 'open' ORDER BY kind, left_fact",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_succession_stays_out_of_review_and_an_overlap_comes_in() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let lin = entity(&pool, &f, "Lin Zhao").await?;
        let (k28, k32, k40) = (
            entity(&pool, &f, "28000").await?,
            entity(&pool, &f, "32000").await?,
            entity(&pool, &f, "40000").await?,
        );
        let aurora = entity(&pool, &f, "Project Aurora").await?;
        let (zhang, zhou, kim) = (
            entity(&pool, &f, "Zhang San").await?,
            entity(&pool, &f, "Zhou Qi").await?,
            entity(&pool, &f, "Kim").await?,
        );
        let (a, b) = (entity(&pool, &f, "A").await?, entity(&pool, &f, "B").await?);

        // ---- 一、三种接任：调薪首尾相接，换负责人中间空了一年，上下级前后对调
        fact(
            &pool,
            &f,
            (lin, f.salary, k28),
            Some("2023-06-01"),
            Some("2024-02-20"),
        )
        .await?;
        let raised = fact(&pool, &f, (lin, f.salary, k32), Some("2024-02-20"), None).await?;
        fact(
            &pool,
            &f,
            (zhang, f.leads, aurora),
            Some("2023-02-01"),
            Some("2024-07-05"),
        )
        .await?;
        let took_over = fact(&pool, &f, (zhou, f.leads, aurora), Some("2025-09-01"), None).await?;
        fact(
            &pool,
            &f,
            (a, f.reports_to, b),
            Some("2020-01-01"),
            Some("2022-01-01"),
        )
        .await?;
        fact(&pool, &f, (b, f.reports_to, a), Some("2023-01-01"), None).await?;

        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.edges, 6);
        assert_eq!(r.found, 0, "三次接任，没有一处矛盾");
        assert!(open(&pool, &f).await?.is_empty());

        // ---- 二、重叠才是矛盾。宾语侧记成 inverse_functional
        let doubled = fact(&pool, &f, (lin, f.salary, k40), Some("2024-06-01"), None).await?;
        let second_lead = fact(&pool, &f, (kim, f.leads, aurora), Some("2025-10-01"), None).await?;
        reasoning::run(&pool, f.kb).await?;
        let rows = open(&pool, &f).await?;
        assert_eq!(rows.len(), 2, "{rows:?}");

        let func = rows
            .iter()
            .find(|r| r.0 == "functional")
            .expect("functional row");
        let pair = sorted(vec![raised, doubled]);
        assert_eq!(
            (func.1, func.2),
            (pair[0], pair[1]),
            "首尾是组里最小与最大的 id"
        );
        assert_eq!(func.3, pair, "path 是整组，按 id 排序");

        let inv = rows
            .iter()
            .find(|r| r.0 == "inverse_functional")
            .expect("宾语侧违规记成 inverse_functional，不冒充 functional");
        assert_eq!(inv.3, sorted(vec![took_over, second_lead]));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &f).await?;
    result
}

#[tokio::test]
async fn a_decision_follows_the_clash_not_the_scan_order() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (a, b) = (entity(&pool, &f, "A").await?, entity(&pool, &f, "B").await?);
        let ab = fact(&pool, &f, (a, f.reports_to, b), None, None).await?;
        let ba = fact(&pool, &f, (b, f.reports_to, a), None, None).await?;

        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.inserted, 1);
        sqlx::query(
            "UPDATE axiom_violations SET status = 'resolved', resolution = 'axiom_relaxed'
              WHERE kb_id = $1 AND kind = 'asymmetry'",
        )
        .bind(f.kb)
        .execute(&pool)
        .await?;

        // 公理没放宽、事实没动，只把先读到的那条挪到堆的后面：一次什么都不改的
        // UPDATE 会写一个新的行版本，顺序扫描于是先读到另一条。从前这就换了键——
        // 插一行新的 open，裁过的那行不重开（#624 的复现序列）
        sqlx::query("UPDATE facts SET confidence = confidence WHERE id = $1")
            .bind(ab)
            .execute(&pool)
            .await?;

        let again = reasoning::run(&pool, f.kb).await?;
        assert_eq!(
            again.inserted, 0,
            "同一处冲突不因为读的顺序变了就成了另一行"
        );
        assert_eq!(again.reopened, 1, "放宽公理的承诺没兑现，那一行回到 open");
        let rows = open(&pool, &f).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].3, sorted(vec![ab, ba]));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &f).await?;
    result
}

#[tokio::test]
async fn an_acceptance_covers_the_facts_it_saw() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let lin = entity(&pool, &f, "Lin Zhao").await?;
        let (x, y, z) = (
            entity(&pool, &f, "base").await?,
            entity(&pool, &f, "bonus").await?,
            entity(&pool, &f, "stock").await?,
        );
        let p = fact(&pool, &f, (lin, f.salary, x), Some("2024-01-01"), None).await?;
        let q = fact(&pool, &f, (lin, f.salary, y), Some("2024-01-01"), None).await?;
        reasoning::run(&pool, f.kb).await?;
        sqlx::query(
            "UPDATE axiom_violations SET status = 'resolved', resolution = 'accepted'
              WHERE kb_id = $1 AND kind = 'functional'",
        )
        .bind(f.kb)
        .execute(&pool)
        .await?;

        let quiet = reasoning::run(&pool, f.kb).await?;
        assert_eq!(
            (quiet.inserted, quiet.reopened),
            (0, 0),
            "认可过的并存重跑沉默"
        );
        assert!(open(&pool, &f).await?.is_empty());

        // ---- 组里多了一条认可时没见过的：那是新情况，要人再看
        let r = fact(&pool, &f, (lin, f.salary, z), Some("2024-03-01"), None).await?;
        reasoning::run(&pool, f.kb).await?;
        let rows = open(&pool, &f).await?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].3, sorted(vec![p, q, r]));

        // ---- 那条撤了：剩下的仍是认可过的那两条。组的首尾可能已经换了，键跟着换，
        // 认可照样管着——按事实比，不按键比
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(r)
            .execute(&pool)
            .await?;
        let settled = reasoning::run(&pool, f.kb).await?;
        assert!(
            open(&pool, &f).await?.is_empty(),
            "撤掉新来的那条，回到认可过的状态"
        );
        assert_eq!(settled.inserted, 0);
        let accepted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM axiom_violations
              WHERE kb_id = $1 AND status = 'resolved' AND resolution = 'accepted'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(accepted, 1, "人的认可活过这一整串重跑");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    cleanup(&pool, &f).await?;
    result
}
