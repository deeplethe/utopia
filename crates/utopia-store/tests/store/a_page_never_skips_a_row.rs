//! 分页翻完，每一行恰好出现一次（#646），打在真库上。
//!
//! Review 的各档与台账都是 `LIMIT/OFFSET` 分页。排序键若不唯一，并列的行每次查询的
//! 先后可以不同——翻到第二页时第一页见过的又出来一次，另一行一页也没出现。时间戳
//! 作排序键时，逐条写入的行很少并列；**一个事务里写下的行全都并列**（`now()` 在事务
//! 里是同一个值），而 Review 的队列正是成批写的。量过：一次检查插下 572 个环，按 200 条
//! 一页翻，翻出 557 个。
//!
//! 三份成批写下的数据，各翻一遍：
//!
//! 1. **公理违规**：一次 `reasoning::run` 插下几百个环，`open_violations` 翻
//! 2. **本体缺陷**：一个语句插下几百行，`open_defects` 翻
//! 3. **审核台账**：一个语句写下几百条审计事件，`review_history` 翻
//!
//! 断言的不只是「每行一次」，而是**翻出来的序列恰好是按时间倒序、再按 id 倒序排好的
//! 全部行**。只断言每行一次是靠运气的：并列行的先后取决于执行计划，旧的排序在这份
//! 数据上三次里只错一次。行的 id 是随机的，与写入的物理顺序无关，所以只要排序在并列
//! 处没有定下来，序列就对不上——每次都对不上。
//!
//! 页长取质数（37），页边界落不到整齐的地方。没有 `UTOPIA_DATABASE_URL` 时跳过而不是
//! 失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use std::future::Future;
use utopia_store::reasoning;
use uuid::Uuid;

const PAGE: i64 = 37;

struct Fixture {
    org: Uuid,
    kb: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'paging-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'paging-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'paging-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(Fixture { org, kb })
}

/// 一页一页翻到底，按翻到的先后接起来
async fn page_through<F, Fut>(mut page: F) -> anyhow::Result<Vec<Uuid>>
where
    F: FnMut(i64) -> Fut,
    Fut: Future<Output = anyhow::Result<Vec<Uuid>>>,
{
    let mut seen = Vec::new();
    let mut offset = 0;
    loop {
        let ids = page(offset).await?;
        let n = ids.len() as i64;
        seen.extend(ids);
        if n < PAGE {
            return Ok(seen);
        }
        offset += PAGE;
    }
}

/// 翻出来的就是全部行，一行一次，并且时间戳并列时按 id 倒序——排序是全序
async fn assert_total_order(
    pool: &PgPool,
    what: &str,
    seen: &[Uuid],
    all_ids: &str,
    expected: usize,
) -> anyhow::Result<()> {
    let mut want: Vec<Uuid> = sqlx::query_scalar(all_ids).fetch_all(pool).await?;
    assert_eq!(want.len(), expected, "{what}: 写下的行数");
    want.sort_by(|a, b| b.cmp(a));
    let mut distinct = seen.to_vec();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "{what}: {} 行翻到了不止一次",
        seen.len() - distinct.len()
    );
    assert_eq!(seen.len(), expected, "{what}: 翻完只见到 {} 行", seen.len());
    assert!(
        seen == want,
        "{what}: 并列的行没有按 id 定下先后，翻页之间会漂"
    );
    Ok(())
}

#[tokio::test]
async fn paging_through_a_batch_sees_every_row_exactly_once() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        // ---- 一、公理违规：一个中心节点与 300 个节点两两互指，恒常谓词上就是 300 个环，
        // 一次检查、一个事务插下
        const CYCLES: usize = 300;
        let (etype, part_of) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 't', 'T')")
            .bind(etype)
            .bind(f.kb)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, temporal, is_transitive)
             VALUES ($1, $2, 'part_of', 'part_of', 'eternal', TRUE)",
        )
        .bind(part_of)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        sqlx::query(
            "WITH n AS (
                 INSERT INTO entities (id, kb_id, type_id, canonical_name)
                 SELECT gen_random_uuid(), $1, $2, 'N' || i FROM generate_series(0, $4) AS i
                 RETURNING id, canonical_name
             ), hub AS (SELECT id FROM n WHERE canonical_name = 'N0'),
                spokes AS (SELECT id FROM n WHERE canonical_name <> 'N0')
             INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
             SELECT gen_random_uuid(), $1, hub.id, $3, spokes.id FROM hub, spokes
             UNION ALL
             SELECT gen_random_uuid(), $1, spokes.id, $3, hub.id FROM hub, spokes",
        )
        .bind(f.kb)
        .bind(etype)
        .bind(part_of)
        .bind(CYCLES as i32)
        .execute(&pool)
        .await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.inserted, CYCLES, "一次检查插下这些环");
        let seen = page_through(|offset| {
            let pool = pool.clone();
            async move {
                Ok(reasoning::open_violations(&pool, f.kb, PAGE, offset)
                    .await?
                    .into_iter()
                    .map(|v| v.id)
                    .collect())
            }
        })
        .await?;
        let ids = format!("SELECT id FROM axiom_violations WHERE kb_id = '{}'", f.kb);
        assert_total_order(&pool, "公理违规", &seen, &ids, CYCLES).await?;

        // ---- 二、本体缺陷：一个语句插下 250 行，detected_at 全相同
        const DEFECTS: usize = 250;
        sqlx::query(
            "INSERT INTO ontology_defects (id, kb_id, kind, subject, other)
             SELECT gen_random_uuid(), $1, 'rules_disagree', gen_random_uuid(), gen_random_uuid()
               FROM generate_series(1, $2)",
        )
        .bind(f.kb)
        .bind(DEFECTS as i32)
        .execute(&pool)
        .await?;
        let seen = page_through(|offset| {
            let pool = pool.clone();
            async move {
                Ok(reasoning::open_defects(&pool, f.kb, PAGE, offset)
                    .await?
                    .into_iter()
                    .map(|d| d.id)
                    .collect())
            }
        })
        .await?;
        let ids = format!("SELECT id FROM ontology_defects WHERE kb_id = '{}'", f.kb);
        assert_total_order(&pool, "本体缺陷", &seen, &ids, DEFECTS).await?;

        // ---- 三、审核台账：一个语句写下 250 条，created_at 全相同
        const EVENTS: usize = 250;
        sqlx::query(
            "INSERT INTO audit_events (id, kb_id, action, target_kind)
             SELECT gen_random_uuid(), $1, 'review.decided', 'violation'
               FROM generate_series(1, $2)",
        )
        .bind(f.kb)
        .bind(EVENTS as i32)
        .execute(&pool)
        .await?;
        let seen = page_through(|offset| {
            let pool = pool.clone();
            async move {
                Ok(
                    utopia_store::audit::review_history(&pool, f.kb, PAGE, offset)
                        .await?
                        .0
                        .into_iter()
                        .map(|e| e.id)
                        .collect(),
                )
            }
        })
        .await?;
        let ids = format!("SELECT id FROM audit_events WHERE kb_id = '{}'", f.kb);
        assert_total_order(&pool, "审核台账", &seen, &ids, EVENTS).await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    // 审计事件只许追加（删不掉），留在测试库里；它们挂在这个自建的库名下，不串到别处
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    result
}
