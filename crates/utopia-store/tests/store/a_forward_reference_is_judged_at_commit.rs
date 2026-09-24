//! 同表自指的前向引用在**提交边界**上判（0070 §1b 递延复合外键）。
//!
//! `facts.supersedes`、`facts.from_statement_id`、`relation_types.inverse_of`、
//! `relation_types.sub_property_of` 都指着同表的行——恢复/批量装载时目标可能
//! 在本语句之后才落盘。`(kb_id, ref)` 复合外键把「存在且同库」并成一条约束，
//! `DEFERRABLE INITIALLY DEFERRED` 让它在提交时重估，那时整批都在。
//!
//! 写入形状分三种，各自的墙不一样：
//!   - **多行 INSERT / COPY**：一条语句一批行——递延外键在提交边界看整批，
//!     跨库的过不了，同库的前向链进得来；
//!   - **顺序语句**：递延外键同样把判断留到提交——同事务里「先插引用、
//!     后插目标」现在合法，目标始终不到的提交时被拦；
//!   - **replica 会话**（pg_restore --disable-triggers 的形状）：这一层其实
//!     没有墙——触发器全关，外键的约束触发器同样静默，同库判定不挡装载。
//!     那样的存量坏行由导出预检与 §0 审计兜底；本文件只验正常事务的
//!     提交边界。

use sqlx::{Acquire, PgPool};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    a: Uuid,
    b: Uuid,
    ent_a: Uuid,
    ent_b: Uuid,
}

/// 两个库各一件最小零件：实体——supersedes 的合法写法也要它们
async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'fwdref-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'fwdref-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for kb in [a, b] {
        sqlx::query(
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'fwdref-test')",
        )
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    }
    let (ent_a, ent_b) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, kb) in [(ent_a, a), (ent_b, b)] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'e')")
            .bind(id)
            .bind(kb)
            .execute(pool)
            .await?;
    }
    Ok(Fixture {
        org,
        a,
        b,
        ent_a,
        ent_b,
    })
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
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

/// 多行 INSERT：引用行在前、目标行在后——FK 在语句末放行，BEFORE 逐行看时
/// 目标还不在。**提交边界上的递延约束**是抓住它的地方：跨库的过不了，
/// 同库的照样落
#[tokio::test]
async fn a_multi_row_insert_is_judged_at_commit() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 引用行在前、别库目标行在后：BEFORE 看不见目标，递延约束看得见整批
    let (newer, older) = (Uuid::now_v7(), Uuid::now_v7());
    let r = sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence, supersedes)
         VALUES ($1, $2, $4, $4, 0.9, $3), ($3, $5, $6, $6, 0.9, NULL)",
    )
    .bind(newer)
    .bind(f.a)
    .bind(older)
    .bind(f.ent_a)
    .bind(f.b) // 目标行落在别库
    .bind(f.ent_b)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "多行 INSERT 里的跨库 supersedes 必须被拒");

    // 同库的同样写法：两行同库，一条语句——合法
    let (newer2, older2) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence, supersedes)
         VALUES ($1, $2, $4, $4, 0.9, $3), ($3, $2, $4, $4, 0.9, NULL)",
    )
    .bind(newer2)
    .bind(f.a)
    .bind(older2)
    .bind(f.ent_a)
    .execute(&pool)
    .await?;

    cleanup(&pool, &f).await
}

/// COPY 是恢复灌库的形状：先放行后校验只在语句提交边界做一次。别库目标
/// 排在本批后面也过不了那道闸；同库的前向链照样进得来
#[tokio::test]
async fn a_copy_batch_is_judged_at_commit() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 跨库：newer 在前、older 在后，older 属 B 库——提交时被拦
    let (newer, older) = (Uuid::now_v7(), Uuid::now_v7());
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    let mut copy = tx
        .copy_in_raw(
            "COPY public.facts (id, kb_id, subject_id, object_id, confidence, supersedes) FROM stdin",
        )
        .await?;
    copy.send(
        format!(
            "{newer}\t{a}\t{ent_a}\t{ent_a}\t0.9\t{older}\n{older}\t{b}\t{ent_b}\t{ent_b}\t0.9\t\\N\n",
            a = f.a,
            b = f.b,
            ent_a = f.ent_a,
            ent_b = f.ent_b,
        )
        .into_bytes(),
    )
    .await?;
    copy.finish().await?;
    let r = tx.commit().await;
    assert!(r.is_err(), "COPY 批里的跨库 supersedes 必须在提交时被拦下");

    // 同库：同样的前向顺序，整条链合法
    let (newer2, older2) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = conn.begin().await?;
    let mut copy = tx
        .copy_in_raw(
            "COPY public.facts (id, kb_id, subject_id, object_id, confidence, supersedes) FROM stdin",
        )
        .await?;
    copy.send(
        format!(
            "{newer2}\t{a}\t{ent_a}\t{ent_a}\t0.9\t{older2}\n{older2}\t{a}\t{ent_a}\t{ent_a}\t0.9\t\\N\n",
            a = f.a,
            ent_a = f.ent_a,
        )
        .into_bytes(),
    )
    .await?;
    copy.finish().await?;
    tx.commit().await?;
    drop(conn);

    cleanup(&pool, &f).await
}

/// 顺序写的前向引用同样归到提交边界：递延外键让「先插引用、后插目标」
/// 在事务里合法，目标始终不到的写法在 COMMIT 被拦下
#[tokio::test]
async fn a_sequential_forward_reference_is_judged_at_commit() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 目标始终不到：语句放行、提交时被拦
    let (newer, older) = (Uuid::now_v7(), Uuid::now_v7());
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence, supersedes)
         VALUES ($1, $2, $3, $3, 0.9, $4)",
    )
    .bind(newer)
    .bind(f.a)
    .bind(f.ent_a)
    .bind(older)
    .execute(&mut *tx)
    .await?;
    let r = tx.commit().await;
    assert!(r.is_err(), "目标始终不到的 supersedes 顺序写，提交时必须报");

    // 同事务里目标后到：整条链在提交时齐了——合法
    let (newer2, older2) = (Uuid::now_v7(), Uuid::now_v7());
    let mut tx = conn.begin().await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence, supersedes)
         VALUES ($1, $2, $3, $3, 0.9, $4)",
    )
    .bind(newer2)
    .bind(f.a)
    .bind(f.ent_a)
    .bind(older2)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
         VALUES ($1, $2, $3, $3, 0.9)",
    )
    .bind(older2)
    .bind(f.a)
    .bind(f.ent_a)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    drop(conn);

    cleanup(&pool, &f).await
}

/// UPDATE 装上的跨库 supersedes：目标已存在，BEFORE 逐行检查看得见它——
/// 当场就报；真漏过去的那一层，递延约束在提交时兜底
#[tokio::test]
async fn an_update_to_a_foreign_supersedes_fails() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let (fa, fb) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence)
         VALUES ($1, $2, $3, $3, 0.9), ($4, $5, $6, $6, 0.9)",
    )
    .bind(fa)
    .bind(f.a)
    .bind(f.ent_a)
    .bind(fb)
    .bind(f.b)
    .bind(f.ent_b)
    .execute(&pool)
    .await?;

    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    let upd = sqlx::query("UPDATE facts SET supersedes = $2 WHERE id = $1")
        .bind(fa)
        .bind(fb)
        .execute(&mut *tx)
        .await;
    if upd.is_ok() {
        let r = tx.commit().await;
        assert!(r.is_err(), "UPDATE 装上的跨库 supersedes 最迟在提交时要报");
    }

    cleanup(&pool, &f).await
}

/// from_statement_id 是同一张表上的第二条自指边：陈述行在批尾、类型化事实
/// 在批头的跨库写法，提交时一样被拦
#[tokio::test]
async fn a_from_statement_is_judged_at_commit() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // 类型化事实在前、别库陈述在后：BEFORE 看不见目标，递延约束看得见整批
    let (typed, stmt) = (Uuid::now_v7(), Uuid::now_v7());
    let r = sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence,
                            layer, phrase, from_statement_id)
         VALUES ($1, $2, $4, $4, 0.9, 'typed', NULL, $3),
                ($3, $5, $6, $6, 0.9, 'open', 'joined', NULL)",
    )
    .bind(typed)
    .bind(f.a)
    .bind(stmt)
    .bind(f.ent_a)
    .bind(f.b) // 陈述行落在别库
    .bind(f.ent_b)
    .execute(&pool)
    .await;
    assert!(
        r.is_err(),
        "多行 INSERT 里的跨库 from_statement_id 必须被拒"
    );

    // 同库的同样写法：类型化事实在前、同库陈述在后——合法
    let (typed2, stmt2) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, object_id, confidence,
                            layer, phrase, from_statement_id)
         VALUES ($1, $2, $4, $4, 0.9, 'typed', NULL, $3),
                ($3, $2, $4, $4, 0.9, 'open', 'joined', NULL)",
    )
    .bind(typed2)
    .bind(f.a)
    .bind(stmt2)
    .bind(f.ent_a)
    .execute(&pool)
    .await?;

    cleanup(&pool, &f).await
}

/// 关系的同表自指同一条边界：inverse_of / sub_property_of 引用行在前、
/// 目标行在后——别库的在提交时被拦，同库的放行
#[tokio::test]
async fn relation_self_links_are_judged_at_commit() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    // inverse_of：一条语句里 A 库的谓词在前、B 库的目标在后——提交时报
    let (inv, tgt) = (Uuid::now_v7(), Uuid::now_v7());
    let r = sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, inverse_of)
         VALUES ($1, $2, 'inv', 'inv', $3), ($3, $4, 'tgt', 'tgt', NULL)",
    )
    .bind(inv)
    .bind(f.a)
    .bind(tgt)
    .bind(f.b)
    .execute(&pool)
    .await;
    assert!(r.is_err(), "多行 INSERT 里的跨库 inverse_of 必须被拒");

    // sub_property_of 同库、目标行在后：合法
    let (child, parent) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, sub_property_of)
         VALUES ($1, $2, 'child', 'child', $3), ($3, $2, 'parent', 'parent', NULL)",
    )
    .bind(child)
    .bind(f.a)
    .bind(parent)
    .execute(&pool)
    .await?;

    cleanup(&pool, &f).await
}
