//! 0070 的 §0 前置检查：往一个**已经有**跨库坏行的库上装
//! 「出处不许跨库」的不变量，迁移必须确定性中止——报出是哪条边、坏了几行，
//! 而不是装上之后替坏数据背书。
//!
//! 做法：开一个隔离库，按迁移文件顺序把 ≤0069 的逐个跑掉（每个一个事务，
//! 与 sqlx::migrate 同一语义），手工塞进一条 0070 之前合法、之后非法的行，
//! 再跑 0070 本体：
//!   - 干净的 0069 库 → 0070 成功，触发器在场；
//!   - 脏的 0069 库 → 0070 报错且报对边名，整个迁移随事务回滚——触发器一个不留。
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

/// 按文件顺序跑 ≤ `through` 的迁移，各自一个事务（与 sqlx::migrate 同一形状）
async fn migrate_to(pool: &PgPool, through: i64) -> anyhow::Result<()> {
    let migrator = sqlx::migrate!("../../migrations");
    let mut conn = pool.acquire().await?;
    for m in migrator.iter().filter(|m| m.version <= through) {
        let mut tx = conn.begin().await?;
        sqlx::raw_sql(&m.sql).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}

async fn migration_70(pool: &PgPool) -> Result<(), sqlx::Error> {
    let migrator = sqlx::migrate!("../../migrations");
    let m = migrator
        .iter()
        .find(|m| m.version == 70)
        .expect("0070 必须在迁移集里");
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    let r = sqlx::raw_sql(&m.sql).execute(&mut *tx).await;
    match r {
        Ok(_) => tx.commit().await,
        Err(e) => {
            let _ = tx.rollback().await;
            Err(e)
        }
    }
}

/// 隔离库：建 → 迁到 0069 → 返回（库名, 连接池）。失败就地跳过
async fn scratch(suffix: &str) -> Option<(String, PgPool)> {
    let admin = admin_url()?;
    let admin_pool = PgPool::connect(&admin).await.ok()?;
    let name = format!("xkb70_{}_{}", suffix, Uuid::now_v7().simple());
    let created = sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&admin_pool)
        .await;
    if created.is_err() {
        eprintln!("跳过：建不了隔离库（角色没有 CREATEDB）");
        admin_pool.close().await;
        return None;
    }
    let url = utopia_store::test_db::url()?;
    let (head, _) = url.rsplit_once('/')?;
    let pool = PgPool::connect(&format!("{head}/{name}")).await.ok()?;
    if let Err(e) = migrate_to(&pool, 69).await {
        eprintln!("跳过：迁到 0069 失败（迁移链自身的问题）: {e}");
        drop_scratch(&name).await;
        return None;
    }
    Some((name, pool))
}

async fn drop_scratch(name: &str) {
    if let Some(admin) = admin_url() {
        if let Ok(pool) = PgPool::connect(&admin).await {
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                .execute(&pool)
                .await;
            pool.close().await;
        }
    }
}

/// 0070 之前合法的最小坏账：库 A 的文档+段落+实体+事实，库 B 的实体与段落——
/// 证据行把 A 的事实配到 B 的段落上
async fn seed_dirty(pool: &PgPool) -> anyhow::Result<()> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'm70-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'm70-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for kb in [a, b] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'm70-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    }
    let (doc_a, doc_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (chunk_b, ent_a, ent_b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    for (id, kb) in [(ent_a, a), (ent_b, b)] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'e')")
            .bind(id)
            .bind(kb)
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
        .bind(format!("sha-{name}"))
        .bind(format!("file:///{name}"))
        .execute(pool)
        .await?;
    }
    // B 库自己的段落——0070 之前把它配到 A 的事实上，什么都不会拦
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'x')",
    )
    .bind(chunk_b)
    .bind(b)
    .bind(doc_b)
    .execute(pool)
    .await?;
    let fact_a = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, 0.9)",
    )
    .bind(fact_a)
    .bind(a)
    .bind(ent_a)
    .bind(ent_a)
    .execute(pool)
    .await?;
    // 坏行：A 的事实配 B 的段落
    sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id) VALUES ($1, $2)")
        .bind(fact_a)
        .bind(chunk_b)
        .execute(pool)
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_clean_ledger_takes_the_invariant() -> anyhow::Result<()> {
    let Some((name, pool)) = scratch("clean").await else {
        return Ok(());
    };
    let r = migration_70(&pool).await;
    assert!(r.is_ok(), "干净的 0069 库必须装得上 0070: {r:?}");
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pg_trigger WHERE tgname = 'fact_evidence_same_kb'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(n, 1, "触发器要真的装上");
    pool.close().await;
    drop_scratch(&name).await;
    Ok(())
}

#[tokio::test]
async fn a_dirty_ledger_stops_the_migration_atomically() -> anyhow::Result<()> {
    let Some((name, pool)) = scratch("dirty").await else {
        return Ok(());
    };
    seed_dirty(&pool).await?;
    let r = migration_70(&pool).await;
    let msg = format!("{r:?}");
    assert!(r.is_err(), "脏库上跑 0070 必须中止");
    assert!(
        msg.contains("cross-KB references already present") || msg.contains("evidence.chunk"),
        "中止要报出坏在哪条边: {msg}"
    );
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pg_trigger WHERE tgname LIKE '%same_kb%'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(n, 0, "中止的迁移不许留下半个不变量");
    pool.close().await;
    drop_scratch(&name).await;
    Ok(())
}
