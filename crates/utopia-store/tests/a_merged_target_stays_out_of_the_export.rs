//! 同库不等于在导出集里：`entities_page` 滤掉 `merged_into`
//! 非空的行，但 merge 只改写 fact 的主语/宾语——属性里的实体值、派生的
//! 主语/宾语仍可能指着已合并的行。序列化照铸它的 IRI 就是一条悬空边。
//!
//! 判法：**同库但不在导出集**也是越界——与越库同一处置，整份拒导。不
//! 重写指向留下的实体（那是另一个语义动作），也不静默省略。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    survivor: Uuid,
    merged: Uuid,
    rel: Uuid,
    attr: Uuid,
    rule: Uuid,
    fact: Uuid,
}

/// 一个库、留着的实体与已合并的实体、一条事实、一条指向已合并实体的
/// 派生——merged 是合法状态（没有触发器拦它），要拦的是导出侧
async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'merged-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'merged-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'merged-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;

    let (survivor, merged) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'survivor')")
        .bind(survivor)
        .bind(kb)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO entities (id, kb_id, canonical_name, merged_into) VALUES ($1, $2, 'gone', $3)",
    )
    .bind(merged)
    .bind(kb)
    .bind(survivor)
    .execute(pool)
    .await?;

    let (rel, attr) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'knows', 'knows')",
    )
    .bind(rel)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind) VALUES ($1, $2, 'since', 'since', 'attribute')",
    )
    .bind(attr)
    .bind(kb)
    .execute(pool)
    .await?;

    let fact = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $3, 0.9)",
    )
    .bind(fact)
    .bind(kb)
    .bind(survivor)
    .bind(rel)
    .execute(pool)
    .await?;

    let rule = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule)
    .bind(kb)
    .bind(rel)
    .execute(pool)
    .await?;

    Ok(Fixture {
        org,
        kb,
        survivor,
        merged,
        rel,
        attr,
        rule,
        fact,
    })
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(pool)
        .await?;
    Ok(())
}

async fn insert_derived(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    object: Uuid,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(subject)
    .bind(f.rel)
    .bind(object)
    .bind(f.rule)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 已合并的实体不进导出集——它不在 entities 页里出现
#[tokio::test]
async fn a_merged_entity_is_not_emitted() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let page = utopia_store::export::entities_page(&mut pool.begin().await?, f.kb, None).await?;
    assert!(page.iter().any(|e| e.id == f.survivor));
    assert!(
        !page.iter().any(|e| e.id == f.merged),
        "merged 实体必须不在导出集里"
    );

    cleanup(&pool, &f).await
}

/// 指着已合并实体的属性值：同库但缺席——体检与事实页都要拦
#[tokio::test]
async fn a_qualifier_on_a_merged_entity_fails_closed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    sqlx::query(
        "INSERT INTO fact_qualifiers (fact_id, qualifier_type_id, entity_id)
         VALUES ($1, $2, $3)",
    )
    .bind(f.fact)
    .bind(f.attr)
    .bind(f.merged)
    .execute(&pool)
    .await?;

    let err = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.kb).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "qualifier→merged 的体检必须拒导");
    assert!(
        msg.contains("qualifier.entity(merged)"),
        "要报 qualifier.entity(merged): {msg}"
    );
    assert!(
        utopia_store::export::facts_page(&mut pool.begin().await?, f.kb, None)
            .await
            .is_err(),
        "事实页也要拦下同一条边"
    );

    sqlx::query("DELETE FROM fact_qualifiers WHERE fact_id = $1")
        .bind(f.fact)
        .execute(&pool)
        .await?;
    cleanup(&pool, &f).await
}

/// 派生的主语/宾语指着已合并的实体：同库但缺席
#[tokio::test]
async fn a_derived_on_a_merged_entity_fails_closed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let d_subj = insert_derived(&pool, &f, f.merged, f.survivor).await?;
    let err = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.kb).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "derived.subject→merged 的体检必须拒导");
    assert!(
        msg.contains("derived.subject(merged)"),
        "要报 derived.subject(merged): {msg}"
    );
    sqlx::query("DELETE FROM derived_facts WHERE id = $1")
        .bind(d_subj)
        .execute(&pool)
        .await?;

    let d_obj = insert_derived(&pool, &f, f.survivor, f.merged).await?;
    let err = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.kb).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "derived.object→merged 的体检必须拒导");
    assert!(
        msg.contains("derived.object(merged)"),
        "要报 derived.object(merged): {msg}"
    );
    sqlx::query("DELETE FROM derived_facts WHERE id = $1")
        .bind(d_obj)
        .execute(&pool)
        .await?;

    // 干净之后就放行——拒的是缺席的引用，不是库本身
    utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.kb).await?;
    cleanup(&pool, &f).await
}

/// 事实的主语/宾语指着已合并的实体（merge 没走到的旧写或绕过 store 的写）：
/// 事实页与体检都要拦
#[tokio::test]
async fn a_fact_on_a_merged_entity_fails_closed() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let bad_fact = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
         VALUES ($1, $2, $3, $4, $5, 0.9)",
    )
    .bind(bad_fact)
    .bind(f.kb)
    .bind(f.survivor)
    .bind(f.rel)
    .bind(f.merged)
    .execute(&pool)
    .await?;

    let err = utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.kb).await;
    let msg = format!("{err:?}");
    assert!(err.is_err(), "fact.object→merged 的体检必须拒导");
    assert!(
        msg.contains("fact.object(merged)"),
        "要报 fact.object(merged): {msg}"
    );
    assert!(
        utopia_store::export::facts_page(&mut pool.begin().await?, f.kb, None)
            .await
            .is_err(),
        "事实页也要拦下同一条边"
    );

    sqlx::query("DELETE FROM facts WHERE id = $1")
        .bind(bad_fact)
        .execute(&pool)
        .await?;
    utopia_store::export::provenance_integrity(&mut pool.begin().await?, f.kb).await?;
    cleanup(&pool, &f).await
}
