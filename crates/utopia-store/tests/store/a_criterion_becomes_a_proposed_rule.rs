//! 把来自语料的判据候选落地为 `attribute_rules` 的一行提案
//!（#507 cut 4 / 0064 cut 3）。
//!
//! 三种结局：
//!   - **Inserted**：subject / predicate / conclude 三件措辞**全**在本库
//!     找到唯一匹配的 `label`——INSERT 一行 `state='proposed'`、conditions
//!     写好。Review 队列里出现这条提案。
//!   - **Unresolved**：任一措辞找不到或多解——返回 `Unresolved` 让调用方写
//!     `drop_signal` 让 Review 看见。**不**落 `attribute_rules`（0064 d3
//!     的「hold」需要 `pending_rule_drafts`，那一档是另一档 PR）。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。

use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    kb: Uuid,
    doc: Uuid,
    chunk: Uuid,
    user: Uuid,
    /// 「井」类的 entity_type（解析时按 label 找到，存这里只为给后续测试留接口）
    _well: Uuid,
    /// 「优秀井」类的 entity_type
    _good_well: Uuid,
    /// 「全烃」属性谓词（kind='attribute'）
    thc: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
    let user = Uuid::now_v7();
    let (well, good_well) = (Uuid::now_v7(), Uuid::now_v7());
    let thc = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'p507-c4-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'p507-c4-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'p507-c4-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    let src = Uuid::now_v7();
    sqlx::query("INSERT INTO sources (id, kb_id, kind) VALUES ($1, $2, 'upload')")
        .bind(src)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, source_id, filename, mime, size_bytes, sha256)
         VALUES ($1, $2, $3, 'criteria.txt', 'text/plain', 100, 'deadbeef')",
    )
    .bind(doc)
    .bind(kb)
    .bind(src)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text)
         VALUES ($1, $2, $3, 0, '当全烃大于 8 时，可判定为优秀井。')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO users (id, email) VALUES ($1, 'p507-c4@example.com')")
        .bind(user)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'well', '井')")
        .bind(well)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'good_well', '优秀井')",
    )
    .bind(good_well)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype)
         VALUES ($1, $2, 'thc', '全烃', 'attribute', 'number')",
    )
    .bind(thc)
    .bind(kb)
    .execute(pool)
    .await?;
    Ok(Fx {
        kb,
        doc,
        chunk,
        user,
        _well: well,
        _good_well: good_well,
        thc,
    })
}

/// 关键 happy path：subject / predicate / conclude 三个 label 都对应到
/// 本库的 UUID → 写一行 `state='proposed'`，写好 conditions，落
/// source_*，写好 `proposed_by`。
#[tokio::test]
async fn a_fully_resolved_criterion_becomes_a_proposed_rule() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (outcome, id) = utopia_store::business_rules::insert_proposal_from_criterion(
            &pool,
            f.kb,
            f.chunk,
            f.doc,
            f.user,
            "全烃大于 8 为优秀井",
            "criteria from corpus",
            "井",
            "全烃",
            "gt",
            "8",
            "优秀井",
            "class",
        )
        .await?;
        anyhow::ensure!(outcome == utopia_store::business_rules::InsertOutcome::Inserted);
        let id = id.expect("rule id expected");
        // 1. attribute_rules 行：state='proposed'、source_* 写好
        let row: (String, String, Uuid, Uuid, Uuid) = sqlx::query_as(
            "SELECT source_kind, state, source_chunk_id, source_document_id, proposed_by
               FROM attribute_rules WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(row.0 == "text", "source_kind: {row:?}");
        anyhow::ensure!(row.1 == "proposed", "state: {row:?}");
        anyhow::ensure!(row.2 == f.chunk);
        anyhow::ensure!(row.3 == f.doc);
        anyhow::ensure!(row.4 == f.user);
        // 2. conditions 行：predicate=thc、op=gt、operand=8
        let cond: (Uuid, String, Option<serde_json::Value>) = sqlx::query_as(
            "SELECT predicate_id, op, operand
               FROM attribute_rule_conditions WHERE rule_id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(cond.0 == f.thc, "predicate_id should be 全烃");
        anyhow::ensure!(cond.1 == "gt");
        anyhow::ensure!(cond.2 == Some(serde_json::json!(8)));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    result
}

/// subject class 没解析到 → Unresolved，不 INSERT
#[tokio::test]
async fn an_unknown_subject_class_is_unresolved() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (outcome, id) = utopia_store::business_rules::insert_proposal_from_criterion(
            &pool,
            f.kb,
            f.chunk,
            f.doc,
            f.user,
            "n/a",
            "n/a",
            "设备", // 不在本库
            "全烃",
            "gt",
            "8",
            "优秀井",
            "class",
        )
        .await?;
        anyhow::ensure!(outcome == utopia_store::business_rules::InsertOutcome::Unresolved);
        anyhow::ensure!(id.is_none());
        // 库里不该有这条
        let count: i64 =
            sqlx::query_scalar("SELECT count(*)::bigint FROM attribute_rules WHERE kb_id = $1")
                .bind(f.kb)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(count == 0, "no rule row should be inserted");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    result
}

/// predicate 不解析 → Unresolved
#[tokio::test]
async fn an_unknown_predicate_is_unresolved() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (outcome, _) = utopia_store::business_rules::insert_proposal_from_criterion(
            &pool,
            f.kb,
            f.chunk,
            f.doc,
            f.user,
            "n/a",
            "n/a",
            "井",
            "电阻率", // 不在本库
            "gt",
            "10",
            "优秀井",
            "class",
        )
        .await?;
        anyhow::ensure!(outcome == utopia_store::business_rules::InsertOutcome::Unresolved);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    result
}

/// `conclude_kind = "attribute"` 时，conclude_label 应解析成
/// relation_type（kind='attribute'），不是 entity_type
#[tokio::test]
async fn an_attribute_conclusion_resolves_to_a_relation_type() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    // 给一个「评价」属性谓词当结论
    let eval = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype)
         VALUES ($1, $2, 'eval', '评价', 'attribute', 'text')",
    )
    .bind(eval)
    .bind(f.kb)
    .execute(&pool)
    .await?;

    let result = async {
        let (outcome, id) = utopia_store::business_rules::insert_proposal_from_criterion(
            &pool,
            f.kb,
            f.chunk,
            f.doc,
            f.user,
            "评价为优秀",
            "n/a",
            "井",
            "全烃",
            "gt",
            "8",
            "评价",
            "attribute",
        )
        .await?;
        anyhow::ensure!(outcome == utopia_store::business_rules::InsertOutcome::Inserted);
        let id = id.unwrap();
        let row: (
            String,
            Option<Uuid>,
            Option<Uuid>,
            Option<serde_json::Value>,
        ) = sqlx::query_as(
            "SELECT conclusion, conclude_type_id, conclude_predicate_id, conclude_value
               FROM attribute_rules WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(row.0 == "attribute");
        anyhow::ensure!(
            row.1.is_none(),
            "conclude_type_id must be NULL for attribute"
        );
        anyhow::ensure!(row.2 == Some(eval));
        // conclude_value 留空——值的来源是「判定」过程，不是「定义」过程
        anyhow::ensure!(row.3.is_none());
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    result
}

/// `op = "eq"` 视同 unresolved（Op 不接 eq——留待后续 PR 扩）
#[tokio::test]
async fn an_unsupported_op_is_unresolved() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (outcome, _) = utopia_store::business_rules::insert_proposal_from_criterion(
            &pool,
            f.kb,
            f.chunk,
            f.doc,
            f.user,
            "n/a",
            "n/a",
            "井",
            "全烃",
            "eq", // 不在 Op 里
            "8",
            "优秀井",
            "class",
        )
        .await?;
        anyhow::ensure!(outcome == utopia_store::business_rules::InsertOutcome::Unresolved);
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    result
}

/// `op = "between"` 把 value "lo to hi" 拆成 JSONB 数组
#[tokio::test]
async fn between_value_is_split_into_lo_hi_array() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let result = async {
        let (outcome, id) = utopia_store::business_rules::insert_proposal_from_criterion(
            &pool,
            f.kb,
            f.chunk,
            f.doc,
            f.user,
            "between",
            "n/a",
            "井",
            "全烃",
            "between",
            "3 to 5",
            "优秀井",
            "class",
        )
        .await?;
        anyhow::ensure!(outcome == utopia_store::business_rules::InsertOutcome::Inserted);
        let id = id.unwrap();
        let cond: (String, Option<serde_json::Value>) =
            sqlx::query_as("SELECT op, operand FROM attribute_rule_conditions WHERE rule_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await?;
        anyhow::ensure!(cond.0 == "between");
        anyhow::ensure!(cond.1 == Some(serde_json::json!(["3", "5"])));
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    result
}
