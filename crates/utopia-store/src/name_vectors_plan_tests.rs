//! 补向量每篇文档都跑一次 `pending`（#1296）：它得以谓词为索引条件走 `facts_live_predicate_idx`，
//! 只读这个库的名字事实、按 id 顺序读，不用排序。改回 JOIN 时规划器会沿 `facts_kb_id_key` 读整个库。
//!
//! 规划器按统计选索引：库是空的时候几个索引代价一样，选哪个看运气（别的测试同时在写 facts，
//! 选择就会翻）。所以这里在回滚的事务里造一个两千条事实、十分之一是名字的库并 ANALYZE，
//! 比的是有数据时它走哪条路。
use super::{DROP_FOR_DOCUMENT_SQL, DROP_FOR_SOURCE_SQL, PENDING_SQL};
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

/// 0071 的清向量每篇身份文档都走一次，不能读完别的文档的证据才挑中这一篇（#1408）。
/// 临时表复制迁移后的列和索引，统计只来自本例，避免别的测试的写入改变查询计划。
/// 这里约束选择性高的小来源；一次覆盖很多文档时，顺扫可能更便宜，应由规划器选择。
#[tokio::test]
async fn dropping_vectors_reads_only_the_selected_documents_evidence() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(
        "CREATE TEMP TABLE documents (LIKE public.documents INCLUDING ALL) ON COMMIT DROP;
         CREATE TEMP TABLE fact_evidence (LIKE public.fact_evidence INCLUDING ALL) ON COMMIT DROP;
         CREATE TEMP TABLE name_vectors (LIKE public.name_vectors INCLUDING ALL) ON COMMIT DROP;
         INSERT INTO documents (id, kb_id, source_id, filename, sha256, external_key)
         SELECT gen_random_uuid(), md5('kb')::uuid,
                md5(CASE WHEN n <= 2 THEN 'selected' ELSE 'other' END)::uuid,
                'document-' || n, md5(n::text), n::text
           FROM generate_series(1, 1000) n;
         INSERT INTO fact_evidence (fact_id, chunk_id, document_id)
         SELECT gen_random_uuid(), gen_random_uuid(), d.id
           FROM documents d CROSS JOIN generate_series(1, 20) n;
         INSERT INTO name_vectors (fact_id, kb_id, entity_id, embedding)
         SELECT fact_id, md5('kb')::uuid, md5('entity')::uuid, '[1,0,0]'::vector
           FROM fact_evidence;
         ANALYZE pg_temp.documents;
         ANALYZE pg_temp.fact_evidence;
         ANALYZE pg_temp.name_vectors;",
    )
    .execute(&mut *tx)
    .await?;
    let (document, source): (Uuid, Uuid) =
        sqlx::query_as("SELECT id, source_id FROM documents WHERE external_key = '1'")
            .fetch_one(&mut *tx)
            .await?;

    for (query, id, expected_rows) in [
        (DROP_FOR_DOCUMENT_SQL, document, 20),
        (DROP_FOR_SOURCE_SQL, source, 40),
    ] {
        // 真正执行删除，但每次都回到相同的向量集合。比访问行数，不把机器快慢变成断言。
        sqlx::query("SAVEPOINT plan_case").execute(&mut *tx).await?;
        let plan: serde_json::Value =
            sqlx::query_scalar(&format!("EXPLAIN (ANALYZE, FORMAT JSON) {query}"))
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        sqlx::query("ROLLBACK TO SAVEPOINT plan_case")
            .execute(&mut *tx)
            .await?;
        let visited = evidence_rows_visited(&plan[0]["Plan"]);
        assert!(visited > 0 && visited <= expected_rows, "{plan}");
    }
    tx.rollback().await?;
    Ok(())
}

fn evidence_rows_visited(node: &serde_json::Value) -> u64 {
    let own_rows = if node["Relation Name"] == "fact_evidence"
        && node["Node Type"]
            .as_str()
            .is_some_and(|kind| kind.ends_with("Scan"))
    {
        (node["Actual Rows"].as_u64().unwrap_or(0)
            + node["Rows Removed by Filter"].as_u64().unwrap_or(0)
            + node["Rows Removed by Index Recheck"].as_u64().unwrap_or(0))
            * node["Actual Loops"].as_u64().unwrap_or(0)
    } else {
        0
    };
    own_rows
        + node["Plans"]
            .as_array()
            .map(|children| children.iter().map(evidence_rows_visited).sum::<u64>())
            .unwrap_or(0)
}
