//! `judge_direction` 看 range，不只看 domain（#222）。
//!
//! 从前只要主语过了 domain 就 Keep，宾语违反 range 没人看：`headOf` 的 domain 是
//! Agent，schema.org 里 Project 也是 Agent，于是 `Project Aurora head_of Li Ting`
//! 原样进图。这里用最小的本体复现那一形：一个两端都允许的 domain，一个只认
//! 公司的 range，反过来读才成立的事实必须被对调。

use sqlx::PgPool;
use utopia_store::ontology::{judge_direction, Fit};
use uuid::Uuid;

struct Fixture {
    /// domain = person | company，range = company
    leads: Uuid,
    alice: Uuid,
    acme: Uuid,
    globex: Uuid,
    /// 还没判出类型的实体
    mystery: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, company, leads) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (alice, acme, globex, mystery) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'direction-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'direction-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'direction-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (company, "company", "Company"),
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
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'leads', 'leads')",
    )
    .bind(leads)
    .bind(kb)
    .execute(pool)
    .await?;
    for ty in [person, company] {
        sqlx::query(
            "INSERT INTO relation_type_domains (relation_type_id, entity_type_id) VALUES ($1, $2)",
        )
        .bind(leads)
        .bind(ty)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO relation_type_ranges (relation_type_id, entity_type_id) VALUES ($1, $2)",
    )
    .bind(leads)
    .bind(company)
    .execute(pool)
    .await?;
    for (id, ty, name) in [
        (alice, Some(person), "Alice"),
        (acme, Some(company), "Acme"),
        (globex, Some(company), "Globex"),
        (mystery, None, "Mystery"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(ty)
        .bind(name)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        leads,
        alice,
        acme,
        globex,
        mystery,
    })
}

#[tokio::test]
async fn direction_is_judged_by_range_too() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    // 正向两端都符合：照旧
    assert_eq!(
        judge_direction(&pool, f.leads, f.alice, f.acme).await?,
        Fit::Keep
    );
    // 主语过了 domain（公司也允许当主语），宾语却是个人、range 只认公司。
    // 从前这里是 Keep——正是 Project Aurora head_of Li Ting 那一形；
    // 反过来读两端都成立，所以对调
    assert_eq!(
        judge_direction(&pool, f.leads, f.acme, f.alice).await?,
        Fit::Swap
    );
    // 宾语还没判出类型：range 这一端"不知道"不算违反，不能因此丢谓词
    assert_eq!(
        judge_direction(&pool, f.leads, f.alice, f.mystery).await?,
        Fit::Keep
    );
    // 两个公司：正向宾语 range 符合、主语 domain 也符合 → Keep
    assert_eq!(
        judge_direction(&pool, f.leads, f.acme, f.globex).await?,
        Fit::Keep
    );
    Ok(())
}
