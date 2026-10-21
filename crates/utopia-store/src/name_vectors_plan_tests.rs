//! 补向量每篇文档都跑一次 `pending`（#1296）：它得以谓词为索引条件走 `facts_live_predicate_idx`，
//! 只读这个库的名字事实、按 id 顺序读，不用排序。改回 JOIN 时规划器会沿 `facts_kb_id_key` 读整个库。
//!
//! 规划器按统计选索引：库是空的时候几个索引代价一样，选哪个看运气（别的测试同时在写 facts，
//! 选择就会翻）。所以这里在回滚的事务里造一个两千条事实、十分之一是名字的库并 ANALYZE，
//! 比的是有数据时它走哪条路。
use super::PENDING_SQL;
use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn pending_reads_the_name_facts_through_the_predicate_index() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (named, other, thing) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let mut tx = pool.begin().await?;
    for (sql, ids) in [
        ("SET LOCAL enable_seqscan = off", vec![]),
        (
            "INSERT INTO organizations (id, name) VALUES ($1, 'plan')",
            vec![org],
        ),
        (
            "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'plan')",
            vec![ws, org],
        ),
        (
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'plan')",
            vec![kb, ws],
        ),
        (
            "INSERT INTO relation_types (id, kb_id, key, label, builtin)
             VALUES ($1, $3, 'known_as', 'known as', true), ($2, $3, 'mentions', 'mentions', false)",
            vec![named, other, kb],
        ),
        (
            "INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'thing')",
            vec![thing, kb],
        ),
        (
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value)
             SELECT gen_random_uuid(), $1, $2, CASE WHEN n % 10 = 0 THEN $3 ELSE $4 END,
                    jsonb_build_object('value', 'v' || n)
               FROM generate_series(1, 2000) n",
            vec![kb, thing, named, other],
        ),
        ("ANALYZE facts", vec![]),
    ] {
        let mut q = sqlx::query(sql);
        for id in ids {
            q = q.bind(id);
        }
        q.execute(&mut *tx).await?;
    }
    let plan: Vec<String> = sqlx::query_scalar(&format!("EXPLAIN {PENDING_SQL}"))
        .bind(kb)
        .bind(crate::names::KNOWN_AS)
        .bind(64_i64)
        .fetch_all(&mut *tx)
        .await?;
    tx.rollback().await?;

    let plan = plan.join("\n");
    assert!(plan.contains("facts_live_predicate_idx"), "{plan}");
    assert!(!plan.contains("Sort"), "{plan}");
    Ok(())
}
