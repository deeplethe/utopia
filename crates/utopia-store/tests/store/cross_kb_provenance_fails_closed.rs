//! 出处链不许跨库（0070）：新行被触发器挡下，存量坏行让导出整份拒绝。
//!
//! `fact_evidence`/`chunks` 的外键只认 id 不认库——原生写入路径碰巧全是同库
//! 构造，但 schema 什么也不拦。两层防：
//!   1. 触发器（0070）挡在一切写入路径下游，包括绕过 store 层的 SQL；
//!   2. 导出侧体检 + 逐页校验——存量坏行与绕过触发器进来的行，宁可整份拒导，
//!      也不能把别库对象的 id 铸进本库 IRI。
//!
//! 坏行在测试里靠 `SET LOCAL session_replication_role='replica'` 制造：只关
//! 本事务的触发器，不碰 catalog——`DISABLE TRIGGER` 是全局的，并行测试会把
//! 对方断言的拒绝窗口撞没。行指着的东西都真实存在，只是不在同一个库——
//! 正是线上会遇到的形态（比如从 0070 之前的备份恢复进来的旧行）。

use sqlx::{Acquire, PgPool};
use uuid::Uuid;

struct TwoKbs {
    org: Uuid,
    a: Uuid,
    b: Uuid,
    doc_a: Uuid,
    doc_b: Uuid,
    chunk_a: Uuid,
    chunk_b: Uuid,
    fact_a: Uuid,
    fact_b: Uuid,
}

/// 两个库、每库一份文档一段一实体一事实——跨库引用需要的合法零件
async fn seed(pool: &PgPool) -> anyhow::Result<TwoKbs> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc_a, doc_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (chunk_a, chunk_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (ent_a, ent_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (fact_a, fact_b) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'crosskb-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'crosskb-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for kb in [a, b] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'crosskb-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    }
    for (id, kb, name) in [(doc_a, a, "a.md"), (doc_b, b, "b.md")] {
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, status, external_key)
             VALUES ($1, $2, $3, $4, 'ready', $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .bind(format!("sha-{id}"))
        .bind(format!("file:///{name}"))
        .execute(pool)
        .await?;
    }
    for (id, kb, doc) in [(chunk_a, a, doc_a), (chunk_b, b, doc_b)] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'x')",
        )
        .bind(id)
        .bind(kb)
        .bind(doc)
        .execute(pool)
        .await?;
    }
    for (id, kb) in [(ent_a, a), (ent_b, b)] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'e')")
            .bind(id)
            .bind(kb)
            .execute(pool)
            .await?;
    }
    for (id, kb, subject, object) in [(fact_a, a, ent_a, ent_a), (fact_b, b, ent_b, ent_b)] {
        sqlx::query("INSERT INTO facts (id, kb_id, subject_id, object_id, confidence) VALUES ($1, $2, $3, $4, 0.9)")
            .bind(id)
            .bind(kb)
            .bind(subject)
            .bind(object)
            .execute(pool)
            .await?;
    }
    Ok(TwoKbs {
        org,
        a,
        b,
        doc_a,
        doc_b,
        chunk_a,
        chunk_b,
        fact_a,
        fact_b,
    })
}

async fn cleanup(pool: &PgPool, f: &TwoKbs) -> anyhow::Result<()> {
    for kb in [f.a, f.b] {
        sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
            .bind(kb)
            .execute(pool)
            .await?;
    }
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn new_cross_kb_writes_are_rejected() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    // 触发器是 0070 带来的：测试库可能还没迁移，这里先保证约束在场
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 事实引用别库段落：原生路径碰巧不会这么写，但 schema 从前不拦——现在拦
    let err = sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         VALUES ($1, $2, 'x', $3, 1)",
    )
    .bind(f.fact_a)
    .bind(f.chunk_b)
    .bind(f.doc_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "fact→foreign chunk 必须被拒");

    // 只坏冗余文档指针那一头：段落同库、文档别库
    let err = sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         VALUES ($1, $2, 'x', $3, 1)",
    )
    .bind(f.fact_a)
    .bind(f.chunk_a)
    .bind(f.doc_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "evidence.document→foreign 必须被拒");

    // 段落挂在别库文档下
    let err = sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text)
         VALUES ($1, $2, $3, 0, 'x')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.doc_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "chunk→foreign document 必须被拒");

    // 权威写入路径同一条约束：add_evidence 走 store API 也一样被拒
    let err = utopia_store::graph::add_evidence(&pool, f.fact_a, f.chunk_b, Some("x"), None).await;
    assert!(err.is_err(), "add_evidence 的跨库配对必须被拒");

    // 过户：把文档挪到别的库，等于把指着它的行一次全变坏行
    let err = sqlx::query("UPDATE documents SET kb_id = $2 WHERE id = $1")
        .bind(f.doc_a)
        .bind(f.b)
        .execute(&pool)
        .await;
    assert!(err.is_err(), "documents.kb_id 过户必须被拒");
    let err = sqlx::query("UPDATE facts SET kb_id = $2 WHERE id = $1")
        .bind(f.fact_a)
        .bind(f.b)
        .execute(&pool)
        .await;
    assert!(err.is_err(), "facts.kb_id 过户必须被拒");

    // 同库的正常写入不受影响（防误伤）
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, quote, document_id, doc_version)
         VALUES ($1, $2, 'ok', $3, 1)",
    )
    .bind(f.fact_a)
    .bind(f.chunk_a)
    .bind(f.doc_a)
    .execute(&pool)
    .await?;
    utopia_store::graph::add_evidence(&pool, f.fact_b, f.chunk_b, Some("ok"), None).await?;

    cleanup(&pool, &f).await
}

#[tokio::test]
async fn malformed_existing_rows_fail_the_export_closed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 存量坏行：触发器只拦落在它之后的写，这种行只能绕过它造——导出侧要接住。
    // SET LOCAL 只在本事务内关触发器，提交即恢复，不会撞掉并行测试的断言窗口
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await?;

    // 只坏段落那一头：fact_a → chunk_b（文档指针留 NULL，隔离变量）
    sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id) VALUES ($1, $2)")
        .bind(f.fact_a)
        .bind(f.chunk_b)
        .execute(&mut *tx)
        .await?;
    // 只坏冗余文档指针：fact_b → chunk_b 本身同库，指针却指 a 的文档
    sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id, document_id) VALUES ($1, $2, $3)")
        .bind(f.fact_b)
        .bind(f.chunk_b)
        .bind(f.doc_a)
        .execute(&mut *tx)
        .await?;
    // 段落挂在别库文档下
    let stray_chunk = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 9, 'stray')",
    )
    .bind(stray_chunk)
    .bind(f.b)
    .bind(f.doc_a)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(conn);

    // 体检：库 A 坏在 evidence.chunk，库 B 坏在 evidence.document 与 chunk.document
    let err_a = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.a).await;
    let msg_a = format!("{err_a:?}");
    assert!(err_a.is_err(), "库 A 的体检必须拒导");
    assert!(
        msg_a.contains("evidence.chunk"),
        "库 A 该报 evidence.chunk: {msg_a}"
    );

    let err_b = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.b).await;
    let msg_b = format!("{err_b:?}");
    assert!(err_b.is_err(), "库 B 的体检必须拒导");
    assert!(
        msg_b.contains("evidence.document"),
        "库 B 该报 evidence.document: {msg_b}"
    );

    // 逐页校验同样fail-closed：体检之后的 TOCTOU 坏行也不能漏出伪造 IRI。
    // 坏段落指针会顺着 facts_page 的 quote_origins 出去，坏文档指针顺着
    // documents[] 出去——两路都在事实页上拦
    assert!(
        utopia_store::export::facts_page(&mut pool.begin().await?, f.a, None)
            .await
            .is_err(),
        "fact_a 的 evidence.chunk 别库：事实页要拦"
    );
    assert!(
        utopia_store::export::facts_page(&mut pool.begin().await?, f.b, None)
            .await
            .is_err(),
        "fact_b 的 evidence.document 别库：事实页要拦"
    );

    // 清掉坏行，导出立刻恢复——拒的是坏行，不是库本身
    sqlx::query("DELETE FROM fact_evidence WHERE fact_id IN ($1, $2)")
        .bind(f.fact_a)
        .bind(f.fact_b)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM chunks WHERE id = $1")
        .bind(stray_chunk)
        .execute(&pool)
        .await?;
    utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.a).await?;
    utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.b).await?;

    cleanup(&pool, &f).await
}
