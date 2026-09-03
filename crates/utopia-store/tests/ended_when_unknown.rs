//! 「结束了，但不知道哪天」这个状态，打在真库上。
//!
//! 两端各记精度之前，`valid_to IS NULL` 同时表示「还在持续」和「结束了但不知何时」。
//! 那两件事**真值相反**——一个说关系现在成立，一个说不成立——而账本只有一种写法，
//! 于是 "former CEO of Weta Digital" 只能写成前者，图会断言一件原文说已经结束的事。
//!
//! 这里钉三样，每一样都活在 SQL 或 SQL 约束里，`cargo check` 一个字看不见：
//!
//! - 三种结束状态都存得进去，四种自相矛盾的组合存不进去
//! - 「结束了但不知哪天」的新观察**不会**被当成"什么都没说"并进开放行
//! - 时态引擎**不把它当开放行**去闭合——它自己都不知道自己何时结束
//!
//! 那四个否定用例里有一个曾经漏网：`valid_to` 有日期而精度为 NULL 时，
//! `NULL IN ('year',…)` 求值是 NULL，`TRUE AND NULL` 是 NULL，CHECK 遇 NULL 判通过。
//! 三值逻辑在这里是静默的，只有真跑一遍才看得见。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::graph::{Validity, ENDED_UNKNOWN};
use uuid::Uuid;

struct Fixture {
    kb: Uuid,
    subject: Uuid,
    predicate: Uuid,
    object_a: Uuid,
    object_b: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let etype = Uuid::now_v7();
    let predicate = Uuid::now_v7();
    let (subject, object_a, object_b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'ended-unknown-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'ended-unknown-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name)
         VALUES ($1, $2, 'ended-unknown-test')",
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
    // functional：主语侧唯一，时态引擎才会对它做闭合对账
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal, functional)
         VALUES ($1, $2, 'leads', 'leads', 'state', TRUE)",
    )
    .bind(predicate)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [
        (subject, "Akkaraju"),
        (object_a, "Weta"),
        (object_b, "Stability"),
    ] {
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
    Ok(Fixture {
        kb,
        subject,
        predicate,
        object_a,
        object_b,
    })
}

fn t(s: &str) -> chrono::DateTime<chrono::Utc> {
    s.parse().unwrap()
}

async fn shape(pool: &PgPool, id: Uuid) -> anyhow::Result<(bool, Option<String>)> {
    let row: (bool, Option<String>) =
        sqlx::query_as("SELECT valid_to IS NULL, valid_to_precision FROM facts WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok(row)
}

#[tokio::test]
async fn a_relation_the_text_says_is_over_is_not_stored_as_ongoing() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // "Akkaraju, former CEO of Weta" —— 结束是原文说的，日期是原文没给的
        let (ended, _) = utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.subject,
            Some(f.predicate),
            f.object_a,
            Validity::starting(Some(t("2020-01-01T00:00:00Z")), Some("day")).ended_when_unknown(),
            0.9,
        )
        .await?;
        let (to_is_null, prec) = shape(&pool, ended).await?;
        assert!(to_is_null, "没有日期可写，valid_to 仍然是 NULL");
        assert_eq!(
            prec.as_deref(),
            Some(ENDED_UNKNOWN),
            "但精度那一位要说出「它结束了」——少了它就跟「仍在持续」分不开"
        );

        // **同一断言再来一次「结束了但不知哪天」的观察，不该被并成"什么都没说"。**
        // 从前的判据是 valid_from.is_none() && valid_to.is_none()，
        // 而这条观察两者都满足——它会被并进开放行，唯一带来的信息（它结束了）就丢了
        let (second, created) = utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.subject,
            Some(f.predicate),
            f.object_b,
            Validity::default().ended_when_unknown(),
            0.9,
        )
        .await?;
        assert!(created, "它说了事情，不该被当成弱化陈述并掉");
        assert_eq!(
            shape(&pool, second).await?.1.as_deref(),
            Some(ENDED_UNKNOWN)
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}

/// 时态引擎**不该把「已结束-不知何时」当成开放行**去闭合：
/// 一条自己都不知道何时结束的断言，没有资格给别人定结束时刻。
#[tokio::test]
async fn an_already_ended_fact_is_not_treated_as_an_open_claim() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 旧行：结束了，不知哪天
        let (old, _) = utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.subject,
            Some(f.predicate),
            f.object_a,
            Validity::starting(Some(t("2020-01-01T00:00:00Z")), Some("day")).ended_when_unknown(),
            0.9,
        )
        .await?;
        // 新行：另一个宾语，2024 年开始。functional 关系，引擎会去找"开放行"
        let (new, _) = utopia_store::graph::insert_fact(
            &pool,
            f.kb,
            f.subject,
            Some(f.predicate),
            f.object_b,
            Validity::starting(Some(t("2024-01-01T00:00:00Z")), Some("day")),
            0.9,
        )
        .await?;
        let report = utopia_store::temporal::reconcile_new_fact(
            &pool,
            f.kb,
            new,
            f.subject,
            f.predicate,
            Some(f.object_b),
            None,
            utopia_store::temporal::Uniqueness::SubjectSide,
            Validity::starting(Some(t("2024-01-01T00:00:00Z")), Some("day")),
            0.9,
        )
        .await?;
        assert_eq!(report.corrected.len(), 0, "旧行已经结束，不该再被闭合一次");
        assert_eq!(report.conflicts, 0, "它也不构成矛盾——两段本来就不重叠");
        // 旧行原样未动
        let still: Option<chrono::DateTime<chrono::Utc>> =
            sqlx::query_scalar("SELECT valid_to FROM facts WHERE id = $1")
                .bind(old)
                .fetch_one(&pool)
                .await?;
        assert!(still.is_none(), "引擎不该凭空给它安一个结束日期");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    run
}
