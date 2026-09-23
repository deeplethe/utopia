//! 导出取数面：单事务快照与游标边界。
//!
//! 判定在导出取数层：`provenance_integrity` 与各 page 函数吃调用方的事务——
//! 同一条连接里种行、读页、断言。除「中途提交」那条探针外（它要两条连接），
//! 所有种子与坏行都在一个**回滚的事务**里造：快照库上跑这组测试不会留下
//! 任何一行。
//!
//! 坏行靠 `SET LOCAL session_replication_role='replica'` 造：只关本事务的
//! 触发器（0070 装上的那些也一并关），回滚即恢复——要模拟的正是绕过触发器
//! 进来的存量坏行。

use sqlx::{Acquire, PgPool, Postgres, Transaction};
use utopia_store::export;
use uuid::Uuid;

struct Fixture {
    a: Uuid,
    b: Uuid,
    doc_a: Uuid,
    attr_a: Uuid,
}

/// 两个库；A 库一份文档一段一实体一事实一条证据一属性，B 库空着备查。
/// 全部在调用方的事务里落——回滚即清场，一个 DELETE 都不用
async fn seed_tx(tx: &mut Transaction<'_, Postgres>) -> anyhow::Result<Fixture> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc_a, doc_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (chunk_a, ent_a, ent_b, fact_a, attr_a) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'export-test')")
        .bind(org)
        .execute(&mut **tx)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'export-test')")
        .bind(ws)
        .bind(org)
        .execute(&mut **tx)
        .await?;
    for kb in [a, b] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'export-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(&mut **tx)
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
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text, doc_version)
         VALUES ($1, $2, $3, 0, 'x', 1)",
    )
    .bind(chunk_a)
    .bind(a)
    .bind(doc_a)
    .execute(&mut **tx)
    .await?;
    for (id, kb) in [(ent_a, a), (ent_b, b)] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'e')")
            .bind(id)
            .bind(kb)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, 0.9)",
    )
    .bind(fact_a)
    .bind(a)
    .bind(ent_a)
    .bind(ent_a)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO fact_evidence (fact_id, chunk_id, document_id, doc_version)
         VALUES ($1, $2, $3, 1)",
    )
    .bind(fact_a)
    .bind(chunk_a)
    .bind(doc_a)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype)
         VALUES ($1, $2, 'headcount', 'headcount', 'attribute', 'number')",
    )
    .bind(attr_a)
    .bind(a)
    .execute(&mut **tx)
    .await?;
    Ok(Fixture {
        a,
        b,
        doc_a,
        attr_a,
    })
}

/// 文档过户的极端形态（replica）：文档挪到 B 之后，A 的证据行还指着它——
/// A 的导出宁拒也不能把别库文档铸进本库 IRI；B 没指着它，照常放行
#[tokio::test]
async fn a_reassigned_document_breaks_the_edges_that_point_at_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;

    let mut tx = pool.begin().await?;
    let f = seed_tx(&mut tx).await?;
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE documents SET kb_id = $2 WHERE id = $1")
        .bind(f.doc_a)
        .bind(f.b)
        .execute(&mut *tx)
        .await?;

    // A 的导出整体拒：evidence.document 还指着那份已经不属于本库的文档
    let err = export::provenance_integrity(&mut tx, f.a).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "文档过户后 A 的出处链必须拒导");
    assert!(
        msg.contains("evidence.document"),
        "该报 evidence.document: {msg}"
    );
    assert!(export::facts_page(&mut tx, f.a, None).await.is_err());

    // B 收下了文档，但它没有任何指着 A 的行：它的体检照样过
    export::provenance_integrity(&mut tx, f.b).await?;
    tx.rollback().await?;
    Ok(())
}

/// 导出中途落下的写进不了这一份。tx 起 REPEATABLE READ 快照后，
/// 另一条连接提交一条新谓词+引用它的派生——本事务的词汇表页与派生页
/// 都看不见它：没有半个进来的引用，也没有悬空的 wasGeneratedBy。
/// 这条要两条连接，种子必须提交——清场照常走
#[tokio::test]
async fn a_mid_stream_commit_stays_outside_the_snapshot() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;

    let (org, ws, a) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let ent_a = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'export-race')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'export-race')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'export-race')",
    )
    .bind(a)
    .bind(ws)
    .execute(&pool)
    .await?;
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'e')")
        .bind(ent_a)
        .bind(a)
        .execute(&pool)
        .await?;

    // 先埋一条谓词和一条引用它的派生，作为「快照内」基线
    let (rule0, pred0) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind)
         VALUES ($1, $2, 'p0', 'p0', 'relation')",
    )
    .bind(pred0)
    .bind(a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule0)
    .bind(a)
    .bind(pred0)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(a)
    .bind(ent_a)
    .bind(pred0)
    .bind(ent_a)
    .bind(rule0)
    .execute(&pool)
    .await?;

    // 导出事务：只读 REPEATABLE READ，快照从第一条语句起钉死
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    export::provenance_integrity(&mut tx, a).await?;
    let _ = export::entities_page(&mut tx, a, None).await?;

    // 中途：另一条连接提交一条新谓词+引用它的派生事实
    let (rule_late, pred_late, derived_late) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind)
         VALUES ($1, $2, 'p_late', 'p_late', 'relation')",
    )
    .bind(pred_late)
    .bind(a)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule_late)
    .bind(a)
    .bind(pred_late)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(derived_late)
    .bind(a)
    .bind(ent_a)
    .bind(pred_late)
    .bind(ent_a)
    .bind(rule_late)
    .execute(&pool)
    .await?;

    // 快照里：词汇表页、派生页都不该有中途进来的行——一致性是整份的
    let relations = export::relations(&mut tx, a).await?;
    assert!(
        !relations.iter().any(|r| r.id == pred_late),
        "中途提交的谓词不许进这份导出"
    );
    let derived = export::derived_page(&mut tx, a, None).await?;
    assert!(
        !derived.iter().any(|d| d.id == derived_late),
        "中途提交的派生不许进这份导出——也就不会有悬空的 wasGeneratedBy"
    );
    assert_eq!(derived.len(), 1, "快照内的那条还在");
    tx.rollback().await?;
    drop(conn);

    // 新事务是新的快照：两条都该在
    let mut tx2 = pool.begin().await?;
    let relations = export::relations(&mut tx2, a).await?;
    assert!(relations.iter().any(|r| r.id == pred_late));
    let derived = export::derived_page(&mut tx2, a, None).await?;
    assert_eq!(derived.len(), 2);
    tx2.rollback().await?;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(a)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}

/// NIL 是合法 uuid——schema 不拦它当主键。按序它排在最前；首页谓词若是
/// `id > 哨兵`，这一行就永远进不了任何一页。而指向它的引用照样解析过去：
/// 节点缺席、边在场，导出里就悬一条没有本体的引用。所有按 id 翻页的
/// 取数口——实体、事实、派生、文档——第一页都得把它翻出来
#[tokio::test]
async fn a_nil_id_row_still_reaches_the_first_page() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;

    let mut tx = pool.begin().await?;
    let f = seed_tx(&mut tx).await?;
    let nil = Uuid::nil();

    // 每张走 id 游标的表各埋一行 NIL 主键；引用一律指回这些 NIL 行自己，
    // 外键不因 NIL 失效——它们跟其他行一样合法
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'nil-e')")
        .bind(nil)
        .bind(f.a)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status, external_key)
         VALUES ($1, $2, 'nil.md', 'nil-sha', 'ready', 'file:///nil.md')",
    )
    .bind(nil)
    .bind(f.a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $3, 0.9)",
    )
    .bind(nil)
    .bind(f.a)
    .bind(nil)
    .bind(f.attr_a)
    .execute(&mut *tx)
    .await?;
    let (pred_r, rule_r) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind)
         VALUES ($1, $2, 'rel_p', 'rel_p', 'relation')",
    )
    .bind(pred_r)
    .bind(f.a)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule_r)
    .bind(f.a)
    .bind(pred_r)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
         VALUES ($1, $2, $3, $4, $3, $5)",
    )
    .bind(nil)
    .bind(f.a)
    .bind(nil)
    .bind(pred_r)
    .bind(rule_r)
    .execute(&mut *tx)
    .await?;

    assert!(
        export::entities_page(&mut tx, f.a, None)
            .await?
            .iter()
            .any(|e| e.id == nil),
        "entities 首页漏掉 NIL 行"
    );
    assert!(
        export::documents_page(&mut tx, f.a, None)
            .await?
            .iter()
            .any(|d| d.id == nil),
        "documents 首页漏掉 NIL 行"
    );
    assert!(
        export::facts_page(&mut tx, f.a, None)
            .await?
            .iter()
            .any(|x| x.id == nil),
        "facts 首页漏掉 NIL 行"
    );
    assert!(
        export::derived_page(&mut tx, f.a, None)
            .await?
            .iter()
            .any(|d| d.id == nil),
        "derived 首页漏掉 NIL 行"
    );
    tx.rollback().await?;
    Ok(())
}
