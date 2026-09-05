//! 检索也倒得回去（0019 开放问题②），打在真库上。
//!
//! 两件事在这里被钉住：
//!
//! 1. **重解析不再丢掉旧块的向量。** 从前落选的块 `embedding = NULL`，省掉的是
//!    两者里更小的一半——正文本来就留着——而代价是历史整段搜不出来。
//! 2. **带时刻的检索按当时的活性走**：当时活着的块、当时还没被删的文档。
//!    删除留墓碑（#268），所以"上周删掉的文档"在上上周的检索里照常命中。
//!
//! 全文那一路不在这里：Tantivy 的索引只有"现在"一个版本，带时刻检索时它
//! 找回来的是对的、但找不全（模块头里写了）。这个测试打的是向量与取块两路。

use pgvector::Vector;
use sqlx::PgPool;
use utopia_ingest::ChunkPiece;
use uuid::Uuid;

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

struct Fixture {
    org: Uuid,
    kb: Uuid,
    /// 五月被重解析顶掉的旧块
    old_chunk: Uuid,
    /// 顶替它的新块
    new_chunk: Uuid,
    /// 属于一篇六月被删掉的文档
    deleted_doc_chunk: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (doc, deleted_doc) = (Uuid::now_v7(), Uuid::now_v7());
    let (old_chunk, deleted_doc_chunk) = (Uuid::now_v7(), Uuid::now_v7());
    let tag = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'as-of-search-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'as-of-search-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name)
         VALUES ($1, $2, 'as-of-search-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, name, deleted) in [
        (doc, "handbook.md", None),
        (deleted_doc, "retired.md", Some("2026-06-01T00:00:00Z")),
    ] {
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, status, created_at, deleted_at)
             VALUES ($1, $2, $3, $4, 'ready', $5, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .bind(format!("sha-{tag}-{name}"))
        .bind(t("2026-02-01T00:00:00Z"))
        .bind(deleted.map(t))
        .execute(pool)
        .await?;
    }

    let embed = |v: [f32; 3]| Vector::from(v.to_vec());
    for (id, document, text, vector) in [
        (
            old_chunk,
            doc,
            "The 2025 handbook says the office is in Shanghai.",
            embed([1.0, 0.0, 0.0]),
        ),
        (
            deleted_doc_chunk,
            deleted_doc,
            "A retired note about the Shanghai office.",
            embed([0.9, 0.1, 0.0]),
        ),
    ] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding, created_at)
             VALUES ($1, $2, $3, 0, $4, $5, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(document)
        .bind(text)
        .bind(vector)
        .bind(t("2026-02-01T00:00:00Z"))
        .execute(pool)
        .await?;
    }

    // 五月：重解析。旧块落选被软删，新块顶上——**旧块的向量必须还在**
    let pieces = vec![ChunkPiece {
        seq: 0,
        text: "The 2026 handbook says the office moved to Shenzhen.".into(),
        char_start: 0,
        char_end: 51,
    }];
    utopia_store::documents::replace_chunks(pool, kb, doc, &pieces).await?;
    let new_chunk: (Uuid,) = sqlx::query_as(
        "SELECT id FROM chunks WHERE document_id = $1 AND superseded_at IS NULL LIMIT 1",
    )
    .bind(doc)
    .fetch_one(pool)
    .await?;
    // 新块的向量由摄入管道后补，这里直接给一个，好让它进得了向量召回
    sqlx::query("UPDATE chunks SET embedding = $2, created_at = $3 WHERE id = $1")
        .bind(new_chunk.0)
        .bind(embed([0.0, 1.0, 0.0]))
        .bind(t("2026-05-01T00:00:00Z"))
        .execute(pool)
        .await?;
    sqlx::query("UPDATE chunks SET superseded_at = $2 WHERE id = $1")
        .bind(old_chunk)
        .bind(t("2026-05-01T00:00:00Z"))
        .execute(pool)
        .await?;

    Ok(Fixture {
        org,
        kb,
        old_chunk,
        new_chunk: new_chunk.0,
        deleted_doc_chunk,
    })
}

#[tokio::test]
async fn a_superseded_chunk_keeps_its_vector_and_answers_as_of_then() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    // 1. 顶掉的块还带着向量。丢了向量，下面每一条都无从谈起
    let kept: (bool,) = sqlx::query_as("SELECT embedding IS NOT NULL FROM chunks WHERE id = $1")
        .bind(f.old_chunk)
        .fetch_one(&pool)
        .await?;
    assert!(
        kept.0,
        "重解析不该顺手丢掉旧块的向量——历史就是靠它才搜得出来"
    );

    // 2. 现在：向量召回给出新块，给不出被顶掉的旧块
    let query = vec![1.0_f32, 0.0, 0.0];
    let now_hits = utopia_store::documents::vector_search(&pool, f.kb, &query, 10, None).await?;
    assert!(!now_hits.contains(&f.old_chunk), "现行检索不该翻出旧版本");
    assert!(now_hits.contains(&f.new_chunk));

    // 3. 三月：那时旧块是现行的，新块还不存在
    let then = Some(t("2026-03-01T00:00:00Z"));
    let then_hits = utopia_store::documents::vector_search(&pool, f.kb, &query, 10, then).await?;
    assert!(
        then_hits.contains(&f.old_chunk),
        "三月该搜得到当时那一版——这一条在丢向量的实现上不可能成立"
    );
    assert!(
        !then_hits.contains(&f.new_chunk),
        "五月才有的块不该出现在三月"
    );

    // 4. 取块也倒：六月删掉的文档，在五月的检索里照常带回来（#268 留了墓碑）
    let live_now = utopia_store::documents::chunks_by_ids(
        &pool,
        f.kb,
        &[f.deleted_doc_chunk, f.new_chunk],
        None,
    )
    .await?;
    let ids: Vec<Uuid> = live_now.iter().map(|c| c.id).collect();
    assert!(
        !ids.contains(&f.deleted_doc_chunk),
        "删掉的文档不出现在现在"
    );
    assert!(ids.contains(&f.new_chunk));

    let live_then = utopia_store::documents::chunks_by_ids(
        &pool,
        f.kb,
        &[f.deleted_doc_chunk, f.old_chunk],
        Some(t("2026-05-15T00:00:00Z")),
    )
    .await?;
    let ids: Vec<Uuid> = live_then.iter().map(|c| c.id).collect();
    assert!(
        ids.contains(&f.deleted_doc_chunk),
        "五月中那篇文档还没被删，它的块该拿得回来"
    );
    assert!(
        !ids.contains(&f.old_chunk),
        "五月初已被顶掉的块不属于五月中"
    );

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    let gone = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
