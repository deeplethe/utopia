//! 0121：角色词搬到它事实的每一块证据上（0070 决定 1，#1267）。
//!
//! 开一个隔离库迁到 0120，种下旧形状的行，再跑 0121：
//!   - 每块证据（被取代的也算）都拿到陈述的角色词，读出来仍是一个值；
//!   - 类型化行只在说过这个词、且值与它相同的来源陈述的块上拿到它；
//!   - 已失效、没有证据的事实（purge 留下的）的角色词被删掉；
//!   - 活着的事实有角色词却没有证据，迁移整份中止。
//!
//! 建库失败（角色没权限的环境）与没有 UTOPIA_DATABASE_URL 一样处理：跳过。

use sqlx::{Acquire, PgPool};
use uuid::Uuid;

/// 维护库的地址：`…/utopia` → `…/postgres`
fn admin_url() -> Option<String> {
    let url = utopia_store::test_db::url()?;
    let (head, _) = url.rsplit_once('/')?;
    Some(format!("{head}/postgres"))
}

/// 按文件顺序跑 `from..=through` 的迁移，各自一个事务（与 sqlx::migrate 同一形状）
async fn migrate(pool: &PgPool, from: i64, through: i64) -> Result<(), sqlx::Error> {
    let migrator = sqlx::migrate!("../../migrations");
    let mut conn = pool.acquire().await?;
    for m in migrator
        .iter()
        .filter(|m| (from..=through).contains(&m.version))
    {
        let mut tx = conn.begin().await?;
        sqlx::raw_sql(&m.sql).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}

/// 隔离库：建 → 迁到 0120 → 返回（库名, 连接池）。失败就地跳过
async fn scratch(suffix: &str) -> Option<(String, PgPool)> {
    let admin_pool = PgPool::connect(&admin_url()?).await.ok()?;
    let name = format!("sq121_{}_{}", suffix, Uuid::now_v7().simple());
    let created = sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
    if created.is_err() {
        eprintln!("跳过：建不了隔离库（角色没有 CREATEDB）");
        return None;
    }
    let url = utopia_store::test_db::url()?;
    let (head, _) = url.rsplit_once('/')?;
    let pool = PgPool::connect(&format!("{head}/{name}")).await.ok()?;
    if let Err(e) = migrate(&pool, 0, 120).await {
        eprintln!("跳过：迁到 0120 失败（迁移链自身的问题）: {e}");
        drop_scratch(&name).await;
        return None;
    }
    Some((name, pool))
}

async fn drop_scratch(name: &str) {
    let Some(admin) = admin_url() else { return };
    if let Ok(pool) = PgPool::connect(&admin).await {
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
            .execute(&pool)
            .await;
        pool.close().await;
    }
}

struct Ledger {
    kb: Uuid,
    entity: Uuid,
    superseded_chunk: Uuid,
    live_chunk: Uuid,
}

/// 一个库、一个实体、一份文档的两块：旧的那块已被取代
async fn seed_ledger(pool: &PgPool) -> anyhow::Result<Ledger> {
    let (org, ws, kb, entity, doc) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (superseded_chunk, live_chunk) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'm121-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'm121-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'm121-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'parse.rs')")
        .bind(entity)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents (id, kb_id, filename, sha256, status)
         VALUES ($1, $2, 'ci.json', 'm121', 'ready')",
    )
    .bind(doc)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text, superseded_at)
         VALUES ($1, $3, $4, 0, 'lines 40-52', now()), ($2, $3, $4, 1, 'lines 58-70', NULL)",
    )
    .bind(superseded_chunk)
    .bind(live_chunk)
    .bind(kb)
    .bind(doc)
    .execute(pool)
    .await?;
    Ok(Ledger {
        kb,
        entity,
        superseded_chunk,
        live_chunk,
    })
}

/// 一条事实；`open` 是开放陈述，否则是类型化行
async fn fact(pool: &PgPool, ledger: &Ledger, open: bool, live: bool) -> anyhow::Result<Uuid> {
    let fact = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence, invalidated_at, layer, phrase)
         VALUES ($1, $2, $3, $3, 0.9, CASE WHEN $4 THEN NULL ELSE now() END,
                 CASE WHEN $5 THEN 'open' ELSE 'typed' END, CASE WHEN $5 THEN 'covers' END)",
    )
    .bind(fact)
    .bind(ledger.kb)
    .bind(ledger.entity)
    .bind(live)
    .bind(open)
    .execute(pool)
    .await?;
    Ok(fact)
}

/// 按 0120 的形状写一个 `lines` 角色词（没有 chunk_id）
async fn says_lines(pool: &PgPool, fact: Uuid) -> anyhow::Result<()> {
    says_lines_as(pool, fact, "40-52").await
}

async fn says_lines_as(pool: &PgPool, fact: Uuid, lines: &str) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO statement_qualifiers (fact_id, role, value) VALUES ($1, 'lines', $2)")
        .bind(fact)
        .bind(serde_json::json!(lines))
        .execute(pool)
        .await?;
    Ok(())
}

async fn evidence(pool: &PgPool, fact: Uuid, chunks: &[Uuid]) -> anyhow::Result<()> {
    for chunk in chunks {
        sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id) VALUES ($1, $2)")
            .bind(fact)
            .bind(chunk)
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn role_word_chunks(pool: &PgPool, facts: Vec<Uuid>) -> anyhow::Result<Vec<(Uuid, Uuid)>> {
    let rows = sqlx::query_as(
        "SELECT fact_id, chunk_id FROM statement_qualifiers
          WHERE fact_id = ANY($1) ORDER BY fact_id, chunk_id",
    )
    .bind(facts)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

#[tokio::test]
async fn role_words_move_onto_every_evidence_chunk_and_leave_a_purged_fact() -> anyhow::Result<()> {
    let Some((name, pool)) = scratch("chunks").await else {
        return Ok(());
    };
    let ledger = seed_ledger(&pool).await?;
    let stated = fact(&pool, &ledger, true, true).await?;
    says_lines(&pool, stated).await?;
    evidence(&pool, stated, &[ledger.superseded_chunk, ledger.live_chunk]).await?;
    let purged = fact(&pool, &ledger, true, false).await?;
    says_lines(&pool, purged).await?;

    migrate(&pool, 121, 121).await?;

    let mut expected = vec![
        (stated, ledger.superseded_chunk),
        (stated, ledger.live_chunk),
    ];
    expected.sort();
    assert_eq!(
        role_word_chunks(&pool, vec![stated, purged]).await?,
        expected,
        "每块证据一行，没有证据的已失效事实一行不留"
    );
    let read = utopia_store::graph::statement_qualifiers_for(&pool, &[stated], None).await?;
    assert_eq!(
        serde_json::to_value(&read[&stated])?,
        serde_json::json!([{ "role": "lines", "value": "40-52", "entity_id": null,
            "entity_name": null, "chunk_ids": [ledger.live_chunk] }])
    );
    pool.close().await;
    drop_scratch(&name).await;
    Ok(())
}

#[tokio::test]
async fn a_typed_row_takes_a_role_word_only_on_the_chunk_of_a_statement_that_said_the_same_value(
) -> anyhow::Result<()> {
    let Some((name, pool)) = scratch("typed").await else {
        return Ok(());
    };
    let ledger = seed_ledger(&pool).await?;
    let (said, silent, disagreed) = (
        fact(&pool, &ledger, true, true).await?,
        fact(&pool, &ledger, true, true).await?,
        fact(&pool, &ledger, true, true).await?,
    );
    says_lines(&pool, said).await?;
    evidence(&pool, said, &[ledger.superseded_chunk]).await?;
    evidence(&pool, silent, &[ledger.live_chunk]).await?;
    // 说了另一个值的来源：它的块不替类型化行说话，类型化行仍只读出原来的值
    says_lines_as(&pool, disagreed, "58-70").await?;
    evidence(&pool, disagreed, &[ledger.live_chunk]).await?;
    let typed = fact(&pool, &ledger, false, true).await?;
    says_lines(&pool, typed).await?;
    evidence(&pool, typed, &[ledger.superseded_chunk, ledger.live_chunk]).await?;
    sqlx::query("INSERT INTO typed_fact_sources (fact_id, statement_id) VALUES ($1, $2), ($1, $3), ($1, $4)",
        )
        .bind(typed)
        .bind(said)
        .bind(silent)
        .bind(disagreed)
        .execute(&pool)
        .await?;
    // 来源已经不在的类型化行：角色词照旧抄到它的证据上，读出来不少
    let sourceless = fact(&pool, &ledger, false, true).await?;
    says_lines(&pool, sourceless).await?;
    evidence(&pool, sourceless, &[ledger.live_chunk]).await?;

    migrate(&pool, 121, 121).await?;

    let mut expected = vec![
        (typed, ledger.superseded_chunk),
        (sourceless, ledger.live_chunk),
    ];
    expected.sort();
    assert_eq!(
        role_word_chunks(&pool, vec![typed, sourceless]).await?,
        expected
    );
    pool.close().await;
    drop_scratch(&name).await;
    Ok(())
}

#[tokio::test]
async fn a_live_fact_with_role_words_and_no_evidence_stops_the_migration() -> anyhow::Result<()> {
    let Some((name, pool)) = scratch("orphan").await else {
        return Ok(());
    };
    let ledger = seed_ledger(&pool).await?;
    let orphan = fact(&pool, &ledger, true, true).await?;
    says_lines(&pool, orphan).await?;

    let err = migrate(&pool, 121, 121).await.unwrap_err().to_string();

    assert!(
        err.contains("live facts with role words and no evidence (x1)"),
        "{err}"
    );
    pool.close().await;
    drop_scratch(&name).await;
    Ok(())
}
