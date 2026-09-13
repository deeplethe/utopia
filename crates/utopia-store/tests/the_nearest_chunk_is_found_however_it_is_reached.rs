//! 向量召回的答案不随计划变（0035 / #512），打在真库上。
//!
//! 索引对使用者是看不见的：契约是召回和记录轴。所以这里每一问都问两遍——
//! 没有索引时一遍，`vector_index::build` 之后再一遍——两遍必须一致。走不走索引
//! 由规划器定，这个测试不假装能替它选；它钉的是**答案**，改了答案就红，只改计划就静。
//!
//! 六个问题各自会以不同的方式坏：
//! - 最近的那一条要回来（表达式索引与 `ORDER BY` 写得不一样，索引就悄悄不生效）
//! - 大库旁边的小库 `LIMIT 10` 要回满（HNSW 后置过滤会把小库回成零行，`relaxed_order` 救的正是它）
//! - 不串库（租户，不是性能）
//! - 另一维度的分块不是候选，也不报错（换过嵌入模型的库两种维度并存）
//! - 顶掉的分块不是命中
//! - 带时刻的检索看到的是当时活着的（0019 对着索引再说一遍）

use pgvector::Vector;
use sqlx::PgPool;
use utopia_store::vector_index::{self, Target};
use uuid::Uuid;

/// 这个测试独占的维度：其它连库测试用 3 维，索引按维度分开，互不打扰
const DIMS: usize = 7;
const CROWD: usize = 2000;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

/// 可复现的伪随机向量：同一粒种子同一批向量，失败了能重放
fn lcg(seed: &mut u64) -> f32 {
    *seed = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    ((*seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
}

struct Fixture {
    org: Uuid,
    big: Uuid,
    small: Uuid,
    /// 大库里与查询向量重合的那一条
    planted: Uuid,
    /// 与查询重合但五月被顶掉的
    superseded: Uuid,
    /// 小库里唯一的一条
    only: Uuid,
    /// 大库里一条 5 维的（换过模型）
    other_dims: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, big, small) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc_big, doc_small) = (Uuid::now_v7(), Uuid::now_v7());
    let (planted, superseded, only, other_dims) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let tag = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'plan-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'plan-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for (kb, name, doc) in [(big, "big", doc_big), (small, "small", doc_small)] {
        sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, $3)")
            .bind(kb)
            .bind(ws)
            .bind(name)
            .execute(pool)
            .await?;
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, status, created_at)
             VALUES ($1, $2, $3, $4, 'ready', $5)",
        )
        .bind(doc)
        .bind(kb)
        .bind(format!("{name}.md"))
        .bind(format!("sha-{tag}-{name}"))
        .bind(t("2026-02-01T00:00:00Z"))
        .execute(pool)
        .await?;
    }

    let mut tx = pool.begin().await?;
    let mut seed = 0x5eed_u64;
    for i in 0..CROWD {
        let v: Vec<f32> = (0..DIMS).map(|_| lcg(&mut seed)).collect();
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(Uuid::now_v7())
        .bind(big)
        .bind(doc_big)
        .bind(i as i32)
        .bind(format!("crowd {i}"))
        .bind(Vector::from(v))
        .bind(t("2026-02-01T00:00:00Z"))
        .execute(&mut *tx)
        .await?;
    }
    // 与查询重合的一条：活着的、五月被顶掉的各一
    for (id, seq, text, superseded_at) in [
        (planted, CROWD as i32, "the planted one", None),
        (
            superseded,
            CROWD as i32 + 1,
            "the one a reparse displaced",
            Some("2026-05-01T00:00:00Z"),
        ),
    ] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding, created_at,
                                 superseded_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(id)
        .bind(big)
        .bind(doc_big)
        .bind(seq)
        .bind(text)
        .bind(Vector::from(query()))
        .bind(t("2026-02-01T00:00:00Z"))
        .bind(superseded_at.map(t))
        .execute(&mut *tx)
        .await?;
    }
    // 换过模型留下的 5 维一条
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(other_dims)
    .bind(big)
    .bind(doc_big)
    .bind(CROWD as i32 + 2)
    .bind("five dims")
    .bind(Vector::from(vec![1.0, 0.0, 0.0, 0.0, 0.0]))
    .bind(t("2026-02-01T00:00:00Z"))
    .execute(&mut *tx)
    .await?;
    // 小库：一条，离查询不近不远
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding, created_at)
         VALUES ($1, $2, $3, 0, 'the only one', $4, $5)",
    )
    .bind(only)
    .bind(small)
    .bind(doc_small)
    .bind(Vector::from(vec![0.3, -0.2, 0.1, 0.4, -0.1, 0.2, 0.0]))
    .bind(t("2026-02-01T00:00:00Z"))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Fixture {
        org,
        big,
        small,
        planted,
        superseded,
        only,
        other_dims,
    })
}

fn query() -> Vec<f32> {
    vec![0.11, -0.42, 0.37, 0.05, -0.28, 0.19, 0.33]
}

/// 六个问题的答案，一次问完；两种计划下各取一份来比
#[derive(Debug, PartialEq)]
struct Answers {
    nearest_in_big: Option<Uuid>,
    small_limit_10: Vec<Uuid>,
    other_dims_query: Vec<Uuid>,
    big_top_10_has_superseded: bool,
    big_top_10_has_other_dims: bool,
    as_of_march_first: Option<Uuid>,
}

async fn ask(pool: &PgPool, f: &Fixture) -> anyhow::Result<Answers> {
    use utopia_store::documents::vector_search;
    let big_now = vector_search(pool, f.big, &query(), 10, None).await?;
    let small_now = vector_search(pool, f.small, &query(), 10, None).await?;
    let five = vector_search(pool, f.big, &[1.0, 0.0, 0.0, 0.0, 0.0], 10, None).await?;
    let big_then =
        vector_search(pool, f.big, &query(), 10, Some(t("2026-03-01T00:00:00Z"))).await?;
    Ok(Answers {
        nearest_in_big: big_now.first().copied(),
        small_limit_10: small_now,
        other_dims_query: five,
        big_top_10_has_superseded: big_now.contains(&f.superseded),
        big_top_10_has_other_dims: big_now.contains(&f.other_dims),
        as_of_march_first: big_then.first().copied(),
    })
}

#[tokio::test]
async fn the_answers_agree_with_and_without_the_index() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // ---- 一、没有索引：精确路径给出的答案
        vector_index::drop(&pool, Target::Chunks, DIMS).await?;
        assert_eq!(
            vector_index::status(&pool, Target::Chunks, DIMS).await?,
            None
        );
        let exact = ask(&pool, &f).await?;
        assert_eq!(exact.nearest_in_big, Some(f.planted), "重合的那一条最近");
        assert_eq!(
            exact.small_limit_10,
            vec![f.only],
            "小库只有一条，LIMIT 10 就回这一条——不多不少，也不串大库"
        );
        assert_eq!(
            exact.other_dims_query,
            vec![f.other_dims],
            "5 维的查询只找 5 维的"
        );
        assert!(!exact.big_top_10_has_superseded, "顶掉的不是命中");
        assert!(!exact.big_top_10_has_other_dims, "7 维的查询看不见 5 维的");
        // 三月一日：顶掉的那条还活着，它与查询重合——两条向量一样，距离并列，由 id 定。
        // 不定的话精确路径和 HNSW 各排各的，下面「索引改了答案」就会时红时绿（#652）
        assert_eq!(
            exact.as_of_march_first,
            Some(f.planted.min(f.superseded)),
            "距离并列由 id 定"
        );
        let then = utopia_store::documents::vector_search(
            &pool,
            f.big,
            &query(),
            2,
            Some(t("2026-03-01T00:00:00Z")),
        )
        .await?;
        assert!(
            then.contains(&f.superseded),
            "三月一日它还活着，带时刻的检索该看见它"
        );

        // ---- 二、建索引：任务本体，事务外，两次幂等
        let built = vector_index::build(&pool, Target::Chunks, DIMS).await?;
        assert!(built.created, "第一次建");
        assert_eq!(built.name, "chunks_embedding_hnsw_7");
        assert_eq!(
            vector_index::status(&pool, Target::Chunks, DIMS).await?,
            Some(true)
        );
        let again = vector_index::build(&pool, Target::Chunks, DIMS).await?;
        assert!(!again.created, "第二次是空操作");
        let def: (String,) = sqlx::query_as(
            "SELECT pg_get_indexdef(indexrelid) FROM pg_index
              WHERE indexrelid = 'chunks_embedding_hnsw_7'::regclass",
        )
        .fetch_one(&pool)
        .await?;
        assert!(def.0.contains("USING hnsw"), "{}", def.0);
        assert!(def.0.contains("vector_cosine_ops"), "{}", def.0);
        assert!(
            def.0.contains("WHERE (vector_dims(embedding) = 7)"),
            "{}",
            def.0
        );

        // ---- 三、有索引：同样六问，答案必须一样
        let indexed = ask(&pool, &f).await?;
        assert_eq!(indexed, exact, "索引改了答案");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = vector_index::drop(&pool, Target::Chunks, DIMS).await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = ANY($1)")
        .bind(vec![f.big, f.small])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
