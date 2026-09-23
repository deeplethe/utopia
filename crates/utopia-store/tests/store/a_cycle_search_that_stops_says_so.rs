//! 环没搜完要说出来，而且没搜到的不算没了（#642），打在真库上。
//!
//! 一团稠密的强连通分量里，深度 12 以内的简单环是天文数字，找环撞上
//! `MAX_CYCLES_PER_PREDICATE` / `MAX_CYCLE_STEPS` 就停。钉三样：
//!
//! 1. **报告里说出来**：`cycles_capped` 数到这个谓词；落库的环不超过上限；同一份数据
//!    重跑不插不清——停在哪儿只取决于数据
//! 2. **没搜完的谓词，上一轮的 open 环不清**：先有一个环，后来同一谓词上长出一团
//!    稠密的，搜索在够到那个环之前就停了。那一行留着——搜不到不等于不存在
//! 3. **没截断的谓词照常清**：另一个谓词上的环，撤掉一条边，那一行照样被清掉
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_reason::MAX_CYCLES_PER_PREDICATE;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    etype: Uuid,
    /// eternal + transitive：稠密的那一团长在它上面
    part_of: Uuid,
    /// eternal + transitive：只有一个小环，从不截断
    located_in: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, part_of, located_in) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'cycle-cap-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'cycle-cap-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'cycle-cap-test')",
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
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal, is_transitive)
         VALUES ($1, $2, 'part_of', 'part_of', 'eternal', TRUE),
                ($3, $2, 'located_in', 'located_in', 'eternal', TRUE)",
    )
    .bind(part_of)
    .bind(kb)
    .bind(located_in)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        etype,
        part_of,
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

/// open 的环行，按谓词分
async fn open_cycles(pool: &PgPool, f: &Fixture, predicate: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT v.id FROM axiom_violations v JOIN facts lf ON lf.id = v.left_fact
          WHERE v.kb_id = $1 AND v.kind = 'cycle' AND v.status = 'open'
            AND lf.predicate_id = $2
          ORDER BY v.id",
    )
    .bind(f.kb)
    .bind(predicate)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn a_cycle_search_that_stops_says_so_and_keeps_what_it_did_not_reach() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        // 稠密那一团的节点先建：id 更小，搜索从它们起步，先撞上上限
        let mut clique = Vec::new();
        for i in 0..12 {
            clique.push(entity(&pool, &f, &format!("C{i}")).await?);
        }
        // ---- 一、先只有一个小环（两个 id 更大的节点）；另一个谓词上也有一个
        let (x, y) = (entity(&pool, &f, "X").await?, entity(&pool, &f, "Y").await?);
        fact(&pool, &f, x, f.part_of, y).await?;
        fact(&pool, &f, y, f.part_of, x).await?;
        let (p, q) = (entity(&pool, &f, "P").await?, entity(&pool, &f, "Q").await?);
        let pq = fact(&pool, &f, p, f.located_in, q).await?;
        fact(&pool, &f, q, f.located_in, p).await?;

        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!((r.found, r.cycles_capped), (2, 0));
        let small = open_cycles(&pool, &f, f.part_of).await?;
        assert_eq!(small.len(), 1);

        // ---- 二、part_of 上长出一团 12 个节点两两互指的
        for &s in &clique {
            for &o in &clique {
                if s != o {
                    fact(&pool, &f, s, f.part_of, o).await?;
                }
            }
        }
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.cycles_capped, 1, "part_of 的环没搜完，要说出来");
        let open = open_cycles(&pool, &f, f.part_of).await?;
        assert!(
            open.contains(&small[0]),
            "搜索在够到 X↔Y 之前就停了——那一行留着，搜不到不等于不存在"
        );
        assert!(
            open.len() <= MAX_CYCLES_PER_PREDICATE + 1,
            "落库的环不超过上限（加上留着的那一行）：{}",
            open.len()
        );
        assert_eq!(r.cleared, 0);

        // 同一份数据重跑：停在同一处，不插不清
        for _ in 0..2 {
            let again = reasoning::run(&pool, f.kb).await?;
            assert_eq!(
                (again.inserted, again.cleared, again.cycles_capped),
                (0, 0, 1)
            );
            assert_eq!(open_cycles(&pool, &f, f.part_of).await?, open);
        }

        // ---- 三、没截断的谓词照常清
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(pq)
            .execute(&pool)
            .await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(
            r.cleared, 1,
            "located_in 没截断，撤掉一条边，那个环的行被清"
        );
        assert!(open_cycles(&pool, &f, f.located_in).await?.is_empty());
        assert_eq!(open_cycles(&pool, &f, f.part_of).await?, open);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    result
}
