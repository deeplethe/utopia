//! 出处与所属判据同库（0098 / 0064 cut 1–2）。
//!
//! 一条从语料读出的判据挂着一段（chunk）与一份文档（document）——
//! 这两件引用都必须落在规则的库上，越库行由复合外键挡在写入路径下游；
//! 现有规则按现状补 `source_kind = 'hand'` 与 `state = 'nodded'`，加列与
//! 复合键都不应当把已有数据顶出来。state 与 source_kind 的一致由 CHECK
//! 守住：手写一律 nodded，语料读出可以 proposed/nodded/declined；text
//! 出处不写齐两条就拒。
//!
//! 同库家族的部分：test_db 跑过 `migrate`，触发器函数、复合外键、CHECK、
//! 索引都在场；本测试只验行为，不重数 catalog（catalog 体检由
//! `migration_0070_runs_under_any_search_path` 的 `assert_0098_installed`
//! 接手）。

use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    ws: Uuid,
    a: Uuid,
    b: Uuid,
    doc_a: Uuid,
    doc_b: Uuid,
    chunk_a: Uuid,
    chunk_b: Uuid,
    type_a: Uuid,
    type_b: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (doc_a, doc_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (chunk_a, chunk_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (type_a, type_b) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'sourced-rule-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'sourced-rule-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for kb in [a, b] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'sourced-rule-test')",
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
    // 两条库 A、库 B 上的实体类型：attribute_rules.subject_type_id 要的就是它
    for (id, kb, name) in [(type_a, a, "well_a"), (type_b, b, "well_b")] {
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, name, kind) VALUES ($1, $2, $3, 'class')",
        )
        .bind(id)
        .bind(kb)
        .bind(name)
        .execute(pool)
        .await?;
    }
    Ok(Fx {
        org,
        ws,
        a,
        b,
        doc_a,
        doc_b,
        chunk_a,
        chunk_b,
        type_a,
        type_b,
    })
}

async fn cleanup(pool: &PgPool, f: &Fx) -> anyhow::Result<()> {
    // attribute_rules 有 chunk/document 复合键反向引用：先把规则清掉
    sqlx::query("DELETE FROM attribute_rules WHERE kb_id IN ($1, $2)")
        .bind(f.a)
        .bind(f.b)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM entity_types WHERE kb_id IN ($1, $2)")
        .bind(f.a)
        .bind(f.b)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM chunks WHERE id IN ($1, $2)")
        .bind(f.chunk_a)
        .bind(f.chunk_b)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM documents WHERE id IN ($1, $2)")
        .bind(f.doc_a)
        .bind(f.doc_b)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM knowledge_bases WHERE id IN ($1, $2)")
        .bind(f.a)
        .bind(f.b)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(f.ws)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn existing_rules_get_hand_and_nodded_backfill() -> anyhow::Result<()> {
    // 加列带 DEFAULT，存量行的 source_kind / state 应当回填好——不能因为
    // 加了一列就要求人把 1000 行规则挨个点过一遍。
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let row = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id)
         VALUES ($1, $2, 'existing-rule', $3, 'typing', $3)
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .fetch_one(&pool)
    .await?;

    let (source_kind, state): (String, String) =
        sqlx::query_as("SELECT source_kind, state FROM attribute_rules WHERE id = $1")
            .bind(row.0)
            .fetch_one(&pool)
            .await?;
    assert_eq!(source_kind, "hand", "存量行应当回填 hand");
    assert_eq!(state, "nodded", "存量行应当回填 nodded（手写即 nod）");

    cleanup(&pool, &f).await?;
    Ok(())
}

#[tokio::test]
async fn text_source_requires_chunk_and_document() -> anyhow::Result<()> {
    // source_kind = 'text' 时 chunk 与 document 都得出处——只填一个被 CHECK 拒。
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 只填 chunk，document 留空——拒
    let err = sqlx::query(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, source_chunk_id)
         VALUES ($1, $2, 'half-sourced', $3, 'typing', $3, 'text', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .bind(f.chunk_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "text 缺 document 必须被拒");

    // 只填 document，chunk 留空——拒
    let err = sqlx::query(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, source_document_id)
         VALUES ($1, $2, 'half-sourced', $3, 'typing', $3, 'text', $4)",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .bind(f.doc_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "text 缺 chunk 必须被拒");

    cleanup(&pool, &f).await?;
    Ok(())
}

#[tokio::test]
async fn hand_written_rule_cannot_be_left_proposed() -> anyhow::Result<()> {
    // 手写规则的 state 永远是 nodded——状态机的入口检查。
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let err = sqlx::query(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, state)
         VALUES ($1, $2, 'hand-pending', $3, 'typing', $3, 'hand', 'proposed')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .execute(&pool)
    .await;
    assert!(
        err.is_err(),
        "hand + proposed 必须被拒：人手按了提交键才入库，没有「待审」一档"
    );

    cleanup(&pool, &f).await?;
    Ok(())
}

#[tokio::test]
async fn text_source_cross_kb_is_rejected_by_composite_fk() -> anyhow::Result<()> {
    // 0098 把 `source_chunk_id` / `source_document_id` 的单列外键换成
    // 复合键 `(kb_id, source_chunk_id) REFERENCES chunks (kb_id, id)`——
    // 别库的 chunk 引用应当被拒。跨库引用需要的不变量与 #901 / #507 同款。
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 库 A 的规则指向库 B 的 chunk
    let err = sqlx::query(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, source_chunk_id, source_document_id, state)
         VALUES ($1, $2, 'a-rule-on-b-chunk', $3, 'typing', $3,
                 'text', $4, $5, 'proposed')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .bind(f.chunk_b)
    .bind(f.doc_b)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "库 A 规则挂库 B chunk 必须被复合外键拒");

    // 同库 OK——库 A 挂库 A 的块和文档
    sqlx::query(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, source_chunk_id, source_document_id, state)
         VALUES ($1, $2, 'a-rule-on-a-chunk', $3, 'typing', $3,
                 'text', $4, $5, 'proposed')",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .bind(f.chunk_a)
    .bind(f.doc_a)
    .execute(&pool)
    .await?;

    // 库 B 的规则指向库 A 的 document（chunk 留同库让它到 document 这一头才挡）
    let err = sqlx::query(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, source_chunk_id, source_document_id, state)
         VALUES ($1, $2, 'b-rule-on-a-doc', $3, 'typing', $3,
                 'text', $4, $5, 'proposed')",
    )
    .bind(Uuid::now_v7())
    .bind(f.b)
    .bind(f.type_b)
    .bind(f.chunk_b)
    .bind(f.doc_a)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "库 B 规则挂库 A document 必须被拒");

    cleanup(&pool, &f).await?;
    Ok(())
}

#[tokio::test]
async fn retract_a_chunk_only_nulls_the_source_column() -> anyhow::Result<()> {
    // `ON DELETE SET NULL (source_chunk_id)`：块被撤，规则行还在；state 与
    // source_kind 都不动（human decision 保留，origin 标空）。这是 0064
    // 「源头改了规则不静默撤销」一档的行为基础。
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let rule: (Uuid,) = sqlx::query_as(
        "INSERT INTO attribute_rules
           (id, kb_id, name, subject_type_id, conclusion, conclude_type_id,
            source_kind, source_chunk_id, source_document_id, state)
         VALUES ($1, $2, 'withdrawn-source', $3, 'typing', $3,
                 'text', $4, $5, 'nodded')
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(f.a)
    .bind(f.type_a)
    .bind(f.chunk_a)
    .bind(f.doc_a)
    .fetch_one(&pool)
    .await?;

    sqlx::query("DELETE FROM chunks WHERE id = $1")
        .bind(f.chunk_a)
        .execute(&pool)
        .await?;

    let (source_kind, state, chunk_after, doc_after): (String, String, Option<Uuid>, Option<Uuid>) =
        sqlx::query_as(
            "SELECT source_kind, state, source_chunk_id, source_document_id
               FROM attribute_rules WHERE id = $1",
        )
        .bind(rule.0)
        .fetch_one(&pool)
        .await?;
    assert_eq!(source_kind, "text", "源头撤了 source_kind 不应当动");
    assert_eq!(state, "nodded", "源头撤了 state 不应当动——human decision 保留");
    assert!(chunk_after.is_none(), "source_chunk_id 应当被 SET NULL");
    assert!(doc_after.is_some(), "source_document_id 没动");

    cleanup(&pool, &f).await?;
    Ok(())
}