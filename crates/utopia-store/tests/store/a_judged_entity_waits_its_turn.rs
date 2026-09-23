//! 0016 C2：类型消解自动跑，看过的不再回头，同库的任务不重复排。
//!
//! 候选查询按事实数排序、一轮六十个；自动跑若不跳过看过的，每一轮都是同一批，
//! 后面的永远轮不到。这里守三件事：
//!
//! 1. **自动跑跳过看过的，人点的不跳。** 打了 `type_resolved_at` 的实体在
//!    `unattended = true` 时不出现，`false` 时照常出现。
//! 2. **人拍过板的两条路都不碰。** `type_source = 'human'` 的实体谁也不重判。
//! 3. **同种任务同载荷排着就不再排。** `enqueue_unless_queued` 第二次返回 None；
//!    那条跑完（不再 queued）之后才能再排。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::{jobs, resolution};
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    thing: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb, thing, sub) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'judged-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'judged-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'judged-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    // 一个有子类的类：现类还有子类，是候选查询的第三种条件
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'thing', 'Thing')",
    )
    .bind(thing)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'gadget', 'Gadget')",
    )
    .bind(sub)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
        .bind(sub)
        .bind(thing)
        .execute(pool)
        .await?;
    Ok(Fx { org, kb, thing })
}

async fn entity(pool: &PgPool, f: &Fx, name: &str, source: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name, type_source)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.thing)
    .bind(name)
    .bind(source)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn candidates(pool: &PgPool, f: &Fx, unattended: bool) -> anyhow::Result<Vec<Uuid>> {
    Ok(
        resolution::entities_for_type_resolution(pool, f.kb, 50, unattended)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect(),
    )
}

#[tokio::test]
async fn a_judged_entity_waits_its_turn() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let mut job_ids: Vec<i64> = Vec::new();

    let run = async {
        let fresh = entity(&pool, &f, "Fresh", "inferred").await?;
        let seen = entity(&pool, &f, "Seen", "inferred").await?;
        let human = entity(&pool, &f, "Decided", "human").await?;
        resolution::mark_type_judged(&pool, f.kb, &[seen]).await?;

        // 1. 自动跑跳过看过的；人点的不跳
        let auto = candidates(&pool, &f, true).await?;
        assert!(auto.contains(&fresh));
        assert!(
            !auto.contains(&seen),
            "the engine does not look twice on its own"
        );
        let manual = candidates(&pool, &f, false).await?;
        assert!(manual.contains(&fresh) && manual.contains(&seen));

        // 2. 人拍过板的两条路都不碰
        assert!(!auto.contains(&human) && !manual.contains(&human));

        // 3. 同库的任务不重复排
        let payload = serde_json::json!({ "kb_id": f.kb });
        let first = jobs::enqueue_unless_queued(&pool, "resolve_types", payload.clone()).await?;
        assert!(first.is_some());
        job_ids.extend(first);
        let second = jobs::enqueue_unless_queued(&pool, "resolve_types", payload.clone()).await?;
        assert!(second.is_none(), "one queued job per base is enough");
        sqlx::query("UPDATE jobs SET status = 'done' WHERE id = $1")
            .bind(first.unwrap())
            .execute(&pool)
            .await?;
        let third = jobs::enqueue_unless_queued(&pool, "resolve_types", payload).await?;
        assert!(third.is_some(), "once it ran, the next one may queue");
        job_ids.extend(third);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM jobs WHERE id = ANY($1)")
        .bind(&job_ids)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
