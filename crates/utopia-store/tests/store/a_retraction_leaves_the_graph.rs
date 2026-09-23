//! #202：「数据错了」要真的把事实撤掉，队列也得诚实。
//!
//! 此前 `decide` 只改 `axiom_violations`，事实还活在图里；重跑检查撞上 resolved 行
//! 又 `DO NOTHING`，违规既没消失也不再出现。这里守四件事：
//!
//! 1. **撤指定的那一条。** 双事实的违规（asymmetry）要说撤哪条，撤完事实 `invalidated_at`
//!    非空、违规 resolved、重跑不再报。
//! 2. **不在违规里的事实撤不了。**
//! 3. **单事实的违规不用说。** 自环只有一条，直接撤 left。
//! 4. **承诺没兑现就重开。** `axiom_relaxed` 说要去改本体，本体没改、违规又算出来，
//!    那行回到 open；`accepted` 是有意并存，重跑照旧沉默。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    user: Uuid,
    kb: Uuid,
    reports_to: Uuid,
    etype: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb, user) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let (etype, reports_to) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'retract-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'retract-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $1 || '@retract.test', 'r', 'x')",
    )
    .bind(user)
    .bind(org)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'retract-test')",
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
    // 反对称且非自反：双向断言是矛盾，自环也是
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, is_asymmetric, is_irreflexive)
         VALUES ($1, $2, 'reports_to', 'reports to', TRUE, TRUE)",
    )
    .bind(reports_to)
    .bind(kb)
    .execute(pool)
    .await?;
    Ok(Fx {
        org,
        user,
        kb,
        reports_to,
        etype,
    })
}

async fn entity(pool: &PgPool, f: &Fx, name: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.etype)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn fact(pool: &PgPool, f: &Fx, s: Uuid, o: Uuid) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(s)
    .bind(f.reports_to)
    .bind(o)
    .execute(pool)
    .await?;
    Ok(id)
}

/// (id, kind, status, resolution, left, right) 按检出时间
async fn violations(
    pool: &PgPool,
    f: &Fx,
) -> anyhow::Result<Vec<(Uuid, String, String, Option<String>, Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT id, kind, status, resolution, left_fact, right_fact FROM axiom_violations
          WHERE kb_id = $1 ORDER BY detected_at, id",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

async fn retracted(pool: &PgPool, id: Uuid) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT invalidated_at IS NOT NULL FROM facts WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
async fn a_retraction_leaves_the_graph() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let (a, b) = (entity(&pool, &f, "A").await?, entity(&pool, &f, "B").await?);
        let ab = fact(&pool, &f, a, b).await?;
        let ba = fact(&pool, &f, b, a).await?;
        reasoning::run(&pool, f.kb).await?;
        let rows = violations(&pool, &f).await?;
        assert_eq!(rows.len(), 1);
        let (vid, kind, ..) = &rows[0];
        assert_eq!(kind, "asymmetry");

        // 2. 不在违规里的撤不了；双事实的不说撤哪条也不行
        let stranger = fact(&pool, &f, a, entity(&pool, &f, "Z").await?).await?;
        assert!(
            reasoning::retract_from_violation(&pool, f.kb, *vid, Some(stranger), f.user)
                .await
                .is_err()
        );
        assert!(
            reasoning::retract_from_violation(&pool, f.kb, *vid, None, f.user)
                .await
                .is_err()
        );

        // 1. 撤 B→A：事实作废、违规 resolved、重跑不再报
        let gone = reasoning::retract_from_violation(&pool, f.kb, *vid, Some(ba), f.user).await?;
        assert_eq!(gone, ba);
        assert!(retracted(&pool, ba).await?, "the button does what it says");
        assert!(!retracted(&pool, ab).await?, "the other fact stays");
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.reopened, 0);
        let rows = violations(&pool, &f).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2, "resolved");
        assert_eq!(rows[0].3.as_deref(), Some("fact_retracted"));

        // 3. 自环只有一条事实，不用说撤哪条
        let e = entity(&pool, &f, "E").await?;
        let ee = fact(&pool, &f, e, e).await?;
        reasoning::run(&pool, f.kb).await?;
        let (loop_id, ..) = violations(&pool, &f)
            .await?
            .into_iter()
            .find(|v| v.1 == "self_loop")
            .expect("the self loop is reported");
        assert_eq!(
            reasoning::retract_from_violation(&pool, f.kb, loop_id, None, f.user).await?,
            ee
        );
        assert!(retracted(&pool, ee).await?);

        // 4. 承诺没兑现就重开：axiom_relaxed 后本体没改，重跑回到 open；accepted 沉默
        let (c, d) = (entity(&pool, &f, "C").await?, entity(&pool, &f, "D").await?);
        fact(&pool, &f, c, d).await?;
        fact(&pool, &f, d, c).await?;
        reasoning::run(&pool, f.kb).await?;
        let (cd_id, ..) = violations(&pool, &f)
            .await?
            .into_iter()
            .find(|v| v.2 == "open")
            .expect("the new pair is open");
        reasoning::decide(&pool, f.kb, cd_id, "axiom_relaxed", f.user).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(
            r.reopened, 1,
            "the axiom is still declared, so the promise did not hold"
        );
        let row = violations(&pool, &f)
            .await?
            .into_iter()
            .find(|v| v.0 == cd_id)
            .unwrap();
        assert_eq!(row.2, "open");
        assert_eq!(row.3, None);
        reasoning::decide(&pool, f.kb, cd_id, "accepted", f.user).await?;
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(
            r.reopened, 0,
            "accepted means both stand; the queue stays quiet"
        );
        let row = violations(&pool, &f)
            .await?
            .into_iter()
            .find(|v| v.0 == cd_id)
            .unwrap();
        assert_eq!(row.2, "resolved");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
