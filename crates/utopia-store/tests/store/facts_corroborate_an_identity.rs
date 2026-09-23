//! 画像分不开同名的人时，事实能分开（#331，续 #270/#329）。
//!
//! 一个库里有两个「张伟」：一个在平台工程部，一个在财务部。画像向量分不出谁是谁
//! ——#329 之后这种并列会新建实体、入两条人工审核对，而人在审核卡上看的正是那两条
//! `works_for` 事实。**代码本可以自己看。** 这个文件钉的就是那一步。
//!
//! 证据只往一个方向走：一致是「同一人」的弱证据，不一致**什么都不是**。在 Acme
//! 上班又在 Zenith 兼职的人有两个 `works_for`，他仍是一个人；两个不同的人也可能
//! 同属一家公司。所以这里既钉「命中就定案」，也钉「命中不了就一切照旧」——
//! 后者才是不把弱证据当强证据的那道闸。
//!
//! 连库才测得到：命中与否取决于一条带窗口函数、带 NOT EXISTS 的 SQL 挑出了哪些
//! 宾语，`cargo check` 一个字看不见。没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。

use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    kb: Uuid,
    person: Uuid,
    zhang_a: Uuid,
    zhang_b: Uuid,
}

/// 两个同名的张伟。`shared` 为真时再给他们一个**共同**的雇主——
/// 那条事实对分辨他们毫无帮助，却最容易在文档里撞上。
async fn seed(pool: &PgPool, shared: bool) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, organization) = (Uuid::now_v7(), Uuid::now_v7());
    let works_for = Uuid::now_v7();
    let (platform, finance, both) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (zhang_a, zhang_b) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'corroboration-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'corroboration-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name)
         VALUES ($1, $2, 'corroboration-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (organization, "organization", "Organization"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal)
         VALUES ($1, $2, 'works_for', 'works for', 'state')",
    )
    .bind(works_for)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, type_id, name) in [
        (platform, organization, "Platform Engineering"),
        (finance, organization, "Finance"),
        (both, organization, "Nebula Holdings"),
        (zhang_a, person, "Zhang Wei"),
        (zhang_b, person, "Zhang Wei"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .execute(pool)
        .await?;
    }
    // 两人的画像**完全相同**：同一个 chunk 播的种（#221 之后是常态）。
    // 向量在这个场景里注定分不开他们
    for (id, v) in [(zhang_a, "[1,0,0]"), (zhang_b, "[1,0,0]")] {
        sqlx::query(
            "UPDATE entities SET profile_embedding = $2::vector, profile_n = 1 WHERE id = $1",
        )
        .bind(id)
        .bind(v)
        .execute(pool)
        .await?;
    }
    let mut pairs = vec![(zhang_a, platform), (zhang_b, finance)];
    if shared {
        pairs.push((zhang_a, both));
        pairs.push((zhang_b, both));
    }
    for (subject, object) in pairs {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
             VALUES ($1, $2, $3, $4, $5, 0.9)",
        )
        .bind(Uuid::now_v7())
        .bind(kb)
        .bind(subject)
        .bind(works_for)
        .bind(object)
        .execute(pool)
        .await?;
    }
    Ok(Fx {
        kb,
        person,
        zhang_a,
        zhang_b,
    })
}

async fn drop_kb(pool: &PgPool, kb: Uuid) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(pool)
        .await?;
    Ok(())
}

/// 并列 + 原文点了其中一个人的部门 → 定案，不再掷硬币也不再劳烦人。
#[tokio::test]
async fn a_fact_in_the_text_settles_a_namesake_tie() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, false).await?;

    let run = async {
        let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.person),
            "Zhang Wei",
            Some(&ctx),
            Some("Zhang Wei of Finance signed off on the quarterly report."),
            &[],
        )
        .await?;
        assert!(!r.created, "原文点名了财务，不该再新建第三个张伟");
        assert_eq!(r.entity_id, f.zhang_b, "该归到财务部那个张伟");
        assert!(
            r.reviews.is_empty(),
            "事实已经把人分开了，不该再入人工审核对"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    drop_kb(&pool, f.kb).await?;
    run
}

/// 原文把两个部门都提了 → 这条线索在他们之间也分不开，维持 #329 的行为。
#[tokio::test]
async fn a_clue_that_points_at_both_settles_nothing() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, false).await?;

    let run = async {
        let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.person),
            "Zhang Wei",
            Some(&ctx),
            Some("Platform Engineering and Finance both sent a Zhang Wei to the review."),
            &[],
        )
        .await?;
        assert!(r.created, "两边都命中就等于没命中，仍该新建实体");
        assert_eq!(r.reviews.len(), 2, "仍该对两个候选各入一条人工审核对");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    drop_kb(&pool, f.kb).await?;
    run
}

/// **共同的雇主不是线索。** 两个张伟都在星云控股，文档提到星云控股什么也没说明——
/// 这条事实在 SQL 里就该被排除，否则它是最容易撞上的那种误判。
#[tokio::test]
async fn an_employer_they_share_is_not_a_clue() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, true).await?;

    let run = async {
        let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.person),
            "Zhang Wei",
            Some(&ctx),
            Some("Zhang Wei has worked at Nebula Holdings for six years."),
            &[],
        )
        .await?;
        assert!(
            r.created,
            "两人共有的雇主没有分辨力，不该拿它把 mention 判给其中一个"
        );
        assert_eq!(r.reviews.len(), 2, "仍是分不开，照旧入两条人工审核对");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    drop_kb(&pool, f.kb).await?;
    run
}

/// 谁都没提到 → 一切照旧。**这一条是那道闸**：不一致（或没提）绝不能被当成
/// 「不是同一人」的证据，否则兼职的人会被拆成两个。
#[tokio::test]
async fn text_that_names_neither_changes_nothing() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, false).await?;

    let run = async {
        let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
        let with_text = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.person),
            "Zhang Wei",
            Some(&ctx),
            // 提到的是第三家公司：一致的证据没有，"不一致"也不算证据
            Some("Zhang Wei moonlights at Zenith Robotics on weekends."),
            &[],
        )
        .await?;
        assert!(with_text.created, "没有命中就该保持 #329 的行为");
        assert_eq!(with_text.reviews.len(), 2, "两条人工审核对照旧");
        assert_ne!(with_text.entity_id, f.zhang_a);
        assert_ne!(with_text.entity_id, f.zhang_b);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    drop_kb(&pool, f.kb).await?;
    run
}

/// 灰区（分数在 `SIM_NEW..SIM_ATTACH`）同样受旁证左右：向量说「不够像」，
/// 而原文点了名。阈值一个都没动，动的是在阈值之外多问了一句。
#[tokio::test]
async fn the_grey_zone_listens_to_the_facts_too() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool, false).await?;

    let run = async {
        // 让两个候选分开且都落在灰区：与 A 约 0.45，与 B 约 0.40，
        // 差 0.05 > SIM_TIE_MARGIN，所以走的是灰区那条路而不是并列那条
        sqlx::query("UPDATE entities SET profile_embedding = '[0,1,0]'::vector WHERE id = $1")
            .bind(f.zhang_b)
            .execute(&pool)
            .await?;
        let ctx: Vec<f32> = vec![0.45, 0.40, 0.80];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.person),
            "Zhang Wei",
            Some(&ctx),
            Some("The Finance lead, Zhang Wei, approved it."),
            &[],
        )
        .await?;
        assert!(!r.created, "灰区里原文点了名，就不该再新建一个同名实体");
        assert_eq!(r.entity_id, f.zhang_b, "该归到财务部那个");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    drop_kb(&pool, f.kb).await?;
    run
}
