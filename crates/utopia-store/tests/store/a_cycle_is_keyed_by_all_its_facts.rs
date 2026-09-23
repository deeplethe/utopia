//! 环按整条路径定键（#641），打在真库上。
//!
//! 环的 left 是最小那条事实、right 是转到它起头之后的最后一条。两个环共用这两条、
//! 中间走的不同时，从前在 `(kb_id, kind, left_fact, right_fact)` 上撞成一行：后算出来的
//! 覆盖先算出来的 path，另一个环在 Review 里消失，而留下哪一个取决于遍历顺序。钉四样：
//!
//! 1. **两个环两行**，重跑不插不清、行的 id 不变
//! 2. **裁决各管各的**：认可其中一个，另一个放宽公理后重开——被认可的那一行 path
//!    仍是它自己的环，没有被另一个环的路径盖掉
//! 3. **撤掉只在一个环上的边**，只影响那一个
//! 4. **别的种类照旧按首尾两条**：反对称那一对重跑仍是同一行
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    etype: Uuid,
    user: Uuid,
    /// state + transitive
    part_of: Uuid,
    /// state + asymmetric
    reports_to: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, user, part_of, reports_to) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'cycle-key-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'cycle-key-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'cycle-key-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, password_hash, display_name)
         VALUES ($1, $2, $3, '', 'Cycle Key Reviewer')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("cycle-key-{user}@test.local"))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal, is_transitive, is_asymmetric)
         VALUES ($1, $2, 'part_of', 'part_of', 'state', TRUE, FALSE),
                ($3, $2, 'reports_to', 'reports_to', 'state', FALSE, TRUE)",
    )
    .bind(part_of)
    .bind(kb)
    .bind(reports_to)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        etype,
        user,
        part_of,
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

/// 一条从此刻起一直成立的事实：大家同时成立，这里只问形状
async fn fact(pool: &PgPool, f: &Fixture, s: Uuid, p: Uuid, o: Uuid) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(s)
    .bind(p)
    .bind(o)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 环行：(id, path, status, resolution)，按 path 排
async fn cycle_rows(
    pool: &PgPool,
    f: &Fixture,
) -> anyhow::Result<Vec<(Uuid, Vec<Uuid>, String, Option<String>)>> {
    Ok(sqlx::query_as(
        "SELECT id, path, status, resolution FROM axiom_violations
          WHERE kb_id = $1 AND kind = 'cycle' ORDER BY path",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn two_cycles_sharing_their_ends_are_two_rows() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let [a, b, c, d, x] = [
            entity(&pool, &f, "A").await?,
            entity(&pool, &f, "B").await?,
            entity(&pool, &f, "C").await?,
            entity(&pool, &f, "D").await?,
            entity(&pool, &f, "X").await?,
        ];
        // 按 id 先后插：A→B 最小，X→A 收尾最后一条。两个环的首尾都是这两条
        let ab = fact(&pool, &f, a, f.part_of, b).await?;
        let bc = fact(&pool, &f, b, f.part_of, c).await?;
        let bd = fact(&pool, &f, b, f.part_of, d).await?;
        let cx = fact(&pool, &f, c, f.part_of, x).await?;
        let dx = fact(&pool, &f, d, f.part_of, x).await?;
        let xa = fact(&pool, &f, x, f.part_of, a).await?;
        let (via_c, via_d) = (vec![ab, bc, cx, xa], vec![ab, bd, dx, xa]);

        // ---- 一、两个环两行；重跑稳定
        let first = reasoning::run(&pool, f.kb).await?;
        assert_eq!(first.found, 2);
        assert_eq!(first.inserted, 2, "两个环各插一行，不是一行被另一行覆盖");
        let rows = cycle_rows(&pool, &f).await?;
        let paths: Vec<Vec<Uuid>> = rows.iter().map(|r| r.1.clone()).collect();
        assert_eq!(paths, vec![via_c.clone(), via_d.clone()]);
        for _ in 0..3 {
            let again = reasoning::run(&pool, f.kb).await?;
            assert_eq!((again.inserted, again.cleared), (0, 0));
            let ids: Vec<Uuid> = cycle_rows(&pool, &f).await?.iter().map(|r| r.0).collect();
            assert_eq!(
                ids,
                rows.iter().map(|r| r.0).collect::<Vec<_>>(),
                "行不换 id"
            );
        }
        let (id_c, id_d) = (rows[0].0, rows[1].0);

        // ---- 二、裁决各管各的
        reasoning::decide(&pool, f.kb, id_c, "accepted", f.user).await?;
        reasoning::decide(&pool, f.kb, id_d, "axiom_relaxed", f.user).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.reopened, 1, "放宽公理的那个环还在，重开它——只重开它");
        let rows = cycle_rows(&pool, &f).await?;
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (
                rows[0].0,
                &rows[0].1,
                rows[0].2.as_str(),
                rows[0].3.as_deref()
            ),
            (id_c, &via_c, "resolved", Some("accepted")),
            "认可的那一行仍是经 C 的环，没被经 D 的路径盖掉"
        );
        assert_eq!((rows[1].0, rows[1].2.as_str()), (id_d, "open"));

        // ---- 三、撤掉只在经 D 那个环上的 B→D
        reasoning::retract_from_violation(&pool, f.kb, id_d, Some(bd), f.user).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(
            (r.found, r.reopened, r.inserted),
            (1, 0, 0),
            "只剩经 C 的环，而它认可过"
        );
        let rows = cycle_rows(&pool, &f).await?;
        assert_eq!(rows[0].3.as_deref(), Some("accepted"));
        assert_eq!(rows[1].3.as_deref(), Some("fact_retracted"));

        // ---- 四、别的种类照旧按首尾两条
        let boss = fact(&pool, &f, c, f.reports_to, d).await?;
        let back = fact(&pool, &f, d, f.reports_to, c).await?;
        reasoning::run(&pool, f.kb).await?;
        let again = reasoning::run(&pool, f.kb).await?;
        assert_eq!(again.inserted, 0);
        let pair: Vec<(Uuid, Uuid)> = sqlx::query_as(
            "SELECT left_fact, right_fact FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'asymmetry'",
        )
        .bind(f.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(pair, vec![(boss.min(back), boss.max(back))]);
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
    result
}
