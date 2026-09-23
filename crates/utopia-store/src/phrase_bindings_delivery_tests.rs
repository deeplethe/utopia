//! 人的判定与它的重算任务是一次提交（0051）。
//!
//! 四件事：判定和 job 一起落库、载荷指着这个库；排队失败时判定也没了；别人握着
//! 物化锁时 `try_materialize` 立刻让开而不是等；两次判定各自的 job 不管先后处理，
//! 类型化图都收敛到最后一次判定。没有 `UTOPIA_DATABASE_URL` 时跳过。

use super::{decide_with_delivery, decide_with_delivery_budget, Decision, MATERIALIZE_KIND};
use crate::{materialize, phrase_bindings};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    property: Uuid,
    sig: phrase_bindings::PhraseSignature,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb, subject, object, property, statement) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'phrase-delivery')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'phrase-delivery')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'phrase-delivery')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, name) in [(subject, "Acme"), (object, "London")] {
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(pool)
            .await?;
    }
    sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES($1,$2,'based_in','based in','state')")
        .bind(property).bind(kb).execute(pool).await?;
    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES($1,$2,$3,$4,'open','based in')")
        .bind(statement).bind(kb).bind(subject).bind(object).execute(pool).await?;
    let sig = phrase_bindings::signatures(pool, kb).await?.remove(0);
    Ok(Fx {
        org,
        kb,
        property,
        sig,
    })
}

async fn cleanup(pool: &PgPool, f: &Fx) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(f.kb.to_string())
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

fn bound(property: Uuid) -> Decision<'static> {
    Decision {
        relation_type_id: Some(property),
        direction: Some("forward"),
        status: "bound",
        votes: &serde_json::Value::Null,
        decided_by: "person",
    }
}

fn none() -> Decision<'static> {
    Decision {
        relation_type_id: None,
        direction: None,
        status: "none",
        votes: &serde_json::Value::Null,
        decided_by: "person",
    }
}

async fn jobs_for(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<(i64, String, String)>> {
    Ok(
        sqlx::query_as("SELECT id, kind, status FROM jobs WHERE payload->>'kb_id'=$1 ORDER BY id")
            .bind(kb.to_string())
            .fetch_all(pool)
            .await?,
    )
}

#[tokio::test]
async fn a_decision_and_its_job_commit_together() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let id = decide_with_delivery(&pool, f.kb, &f.sig, bound(f.property))
            .await?
            .expect("a person's decision is always written");
        let jobs = jobs_for(&pool, f.kb).await?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].0, id);
        assert_eq!(jobs[0].1, MATERIALIZE_KIND);
        assert_eq!(jobs[0].2, "queued");
        let bindings = phrase_bindings::bindings(&pool, f.kb).await?;
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].status, "bound");
        // 载荷只带库：job 读当前绑定，不回放这次判定的属性
        let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM jobs WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(payload, json!({ "kb_id": f.kb }));
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn an_enqueue_failure_takes_the_decision_with_it() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        // 预算 0 被 enqueue 拒掉：一次发生在判定写入之后的真实失败
        assert!(
            decide_with_delivery_budget(&pool, f.kb, &f.sig, bound(f.property), 0)
                .await
                .is_err()
        );
        assert!(phrase_bindings::bindings(&pool, f.kb).await?.is_empty());
        assert!(jobs_for(&pool, f.kb).await?.is_empty());
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn a_busy_projection_is_declined_not_waited_for() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        decide_with_delivery(&pool, f.kb, &f.sig, bound(f.property)).await?;
        // 另一条连接抱着锁：try 版本立刻回 None，不占第二条连接排队
        let mut gate = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext('typed_materialize'), hashtext($1))")
            .bind(f.kb.to_string())
            .execute(&mut *gate)
            .await?;
        let started = std::time::Instant::now();
        assert!(materialize::try_materialize(&pool, f.kb).await?.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        gate.commit().await?;
        let outcome = materialize::try_materialize(&pool, f.kb)
            .await?
            .expect("lock released");
        assert_eq!(outcome.added, 1);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}

#[tokio::test]
async fn jobs_processed_in_any_order_converge_on_the_last_decision() -> anyhow::Result<()> {
    let Some(url) = crate::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    crate::db::migrate(&pool).await?;
    let f = seed(&pool).await?;
    let run = async {
        let first = decide_with_delivery(&pool, f.kb, &f.sig, bound(f.property)).await?;
        let second = decide_with_delivery(&pool, f.kb, &f.sig, none()).await?;
        assert!(first < second);
        // 两个 job 都排着；先处理后来的，再处理先来的——每次都读当前绑定
        assert_eq!(jobs_for(&pool, f.kb).await?.len(), 2);
        materialize::try_materialize(&pool, f.kb).await?;
        materialize::try_materialize(&pool, f.kb).await?;
        assert_eq!(
            materialize::count(&pool, f.kb).await?,
            0,
            "the last decision was none"
        );
        // 反过来：先绑、再解绑、先处理老的
        decide_with_delivery(&pool, f.kb, &f.sig, bound(f.property)).await?;
        materialize::try_materialize(&pool, f.kb).await?;
        assert_eq!(materialize::count(&pool, f.kb).await?, 1);
        anyhow::Ok(())
    }
    .await;
    cleanup(&pool, &f).await?;
    run
}
