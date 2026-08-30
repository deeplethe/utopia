//! `proposed_predicates` 的两个计数，打在真库上。
//!
//! 同 [`graph_changes`] 那个测试的理由：这段逻辑整个活在 SQL 字符串里，
//! `cargo check` 和 clippy 一个字都看不见。这一条尤其需要——它区分的两个数
//! 长得一模一样（都是 `count(DISTINCT …)`），读代码看不出差别，
//! 只有在「一部分事实已经离开积压」的数据形状下才分得开。
//!
//! 要钉住的行为：
//!
//! - **`fact_count` 从积压数** —— 采纳真正会改写的就是那些，多数一条就是承诺
//!   「将重新归类 N 条」时说了谎
//! - **`doc_count` 从全量证据数** —— 它回答的是「这个说法在语料里有多普遍」。
//!   拿积压去数会系统性偏低，而且越用越低：说法被采纳、被谓词匹配接住、
//!   或被修正作废之后，它的行就离开积压了。一篇一篇往里灌的库因此永远
//!   攒不够两篇，本体就此冻死
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

/// 造一个刚好能分辨两种口径的账本：
///
/// 说法 `acquired` 在**两篇**文档里都出现过，但只有一篇的那条事实**还没有谓词**
/// ——另一篇那条已经落到真关系上了（谓词匹配接住，或上一轮采纳搬走）。
///
/// 于是：积压口径 doc_count = 1（会被 `>=2` 门槛误杀），全量口径 = 2。
async fn seed(pool: &PgPool) -> anyhow::Result<Uuid> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let real = Uuid::now_v7();
    let (subj, obj) = (Uuid::now_v7(), Uuid::now_v7());
    let (doc1, doc2) = (Uuid::now_v7(), Uuid::now_v7());
    let (chunk1, chunk2) = (Uuid::now_v7(), Uuid::now_v7());
    let (f_backlog, f_moved) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'proposal-counts-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'proposal-counts-test')",
    )
    .bind(ws)
    .bind(org)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name)
         VALUES ($1, $2, 'proposal-counts-test')",
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
    // 积压的那条不挂任何关系——查询按 predicate_id IS NULL 筛（见 `facts.predicate_id`）
    sqlx::query("INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, $3, $3)")
        .bind(real)
        .bind(kb)
        .bind("acquired_rel")
        .execute(pool)
        .await?;
    for (id, name) in [(subj, "Anthropic"), (obj, "Humanloop")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(etype)
        .bind(name)
        .execute(pool)
        .await?;
    }
    for (id, name) in [(doc1, "one.txt"), (doc2, "two.txt")] {
        sqlx::query("INSERT INTO documents (id, kb_id, filename, sha256) VALUES ($1, $2, $3, $3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(pool)
            .await?;
    }
    for (id, doc) in [(chunk1, doc1), (chunk2, doc2)] {
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'x')",
        )
        .bind(id)
        .bind(kb)
        .bind(doc)
        .execute(pool)
        .await?;
    }

    // 一条还没有谓词，一条已经落到真关系上
    for (id, pred) in [(f_backlog, None), (f_moved, Some(real))] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(subj)
        .bind(pred)
        .bind(obj)
        .execute(pool)
        .await?;
    }
    // 两条证据都记着同一个原始说法——抽取无条件记它，不只是降级时
    for (fact, chunk, doc) in [(f_backlog, chunk1, doc1), (f_moved, chunk2, doc2)] {
        sqlx::query(
            "INSERT INTO fact_evidence (fact_id, chunk_id, document_id, proposed_predicate)
             VALUES ($1, $2, $3, 'acquired')",
        )
        .bind(fact)
        .bind(chunk)
        .bind(doc)
        .execute(pool)
        .await?;
    }

    Ok(kb)
}

#[tokio::test]
async fn spread_counts_all_evidence_while_rewrite_count_stays_on_the_backlog() -> anyhow::Result<()>
{
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = seed(&pool).await?;

    let got = utopia_store::graph::proposed_predicates(&pool, kb).await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;

    let rows = got?;
    let row = rows
        .iter()
        .find(|r| r.form == "acquired")
        .expect("acquired 应该在提案里");

    // 只有一条还没有谓词——采纳会改写的就这一条
    assert_eq!(row.fact_count, 1, "fact_count 应该只数积压");
    // 但这个说法跨了两篇文档。按积压数会得到 1，于是被 `>=2` 的门槛误杀，
    // 而它明明是这个语料的共同词汇
    assert_eq!(row.doc_count, 2, "doc_count 应该数全量证据");
    Ok(())
}

/// 采纳那条路要按屈折基归并，归并后的篇数得算**并集**——所以它不看
/// `doc_count`，改问「这个说法出现在哪些文档里」。那是另一条 SQL，
/// 于是也得另有一个真跑一遍的测试：它一度把 `'relation_type'` 的引号
/// 丢了（`m.kind = relation_type`），clippy 全绿，任务在运行时才炸。
#[tokio::test]
async fn document_ids_come_back_for_each_wording() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = seed(&pool).await?;

    let got = utopia_store::graph::proposed_predicate_documents(&pool, kb).await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;

    let rows = got?;
    let docs: std::collections::HashSet<_> = rows
        .iter()
        .filter(|(form, _)| form == "acquired")
        .map(|(_, doc)| *doc)
        .collect();
    // 两篇文档都用过这个说法，尽管只有一篇那条还没有谓词
    assert_eq!(docs.len(), 2, "两篇都该回来，跟事实落在哪个谓词上无关");
    Ok(())
}
