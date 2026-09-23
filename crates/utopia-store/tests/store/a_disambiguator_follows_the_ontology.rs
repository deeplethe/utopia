//! 同名后缀从本体来，不从一张手写词表来（#299）。
//!
//! 为什么非要连库：选谁当后缀整个是一条 `ORDER BY`。四个排序键写反一个，
//! `cargo check` 不会说话，症状是图上两个同名者并排显示成一模一样的
//! "John Smith · Person"——看起来像没做消歧，其实是词表没对上。
//!
//! 词汇表用 schema.org 那一套（`works_for`），**正是原先那张词表里没有的**：
//! 这个测试在旧实现上必须是红的，否则它什么也没证明。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    /// 两个 John Smith：一个在 Acme Robotics，一个在 St Mary's Hospital
    smith_a: Uuid,
    smith_b: Uuid,
    /// 两个 Wang Ning：都在 Acme，谁也分不开谁
    wang_a: Uuid,
    wang_b: Uuid,
    /// 只有一个 Lin Zhao：组内唯一，不该有后缀
    lin: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, organization, document) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    // works_for 声明了值域；mentioned_in 是抽取顺手长出来的，没人说过它指向什么
    let (works_for, mentioned_in) = (Uuid::now_v7(), Uuid::now_v7());
    let (acme, st_marys) = (Uuid::now_v7(), Uuid::now_v7());
    let (report_a, report_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (smith_a, smith_b) = (Uuid::now_v7(), Uuid::now_v7());
    let (wang_a, wang_b, lin) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'disambig-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'disambig-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'disambig-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (organization, "organization", "Organization"),
        (document, "document", "Document"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    // works_for：状态关系，声明了值域 → 身份性的
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal)
         VALUES ($1, $2, 'works_for', 'works for', 'state')",
    )
    .bind(works_for)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_type_ranges (relation_type_id, entity_type_id) VALUES ($1, $2)",
    )
    .bind(works_for)
    .bind(organization)
    .execute(pool)
    .await?;
    // mentioned_in：没声明值域，且是事件——**置信度和时间都更占优**，
    // 旧排序（confidence DESC, recorded_at DESC）会选它
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal)
         VALUES ($1, $2, 'mentioned_in', 'mentioned in', 'event')",
    )
    .bind(mentioned_in)
    .bind(kb)
    .execute(pool)
    .await?;

    for (id, type_id, name) in [
        (acme, organization, "Acme Robotics"),
        (st_marys, organization, "St Mary's Hospital"),
        (report_a, document, "2026 Field Report"),
        (report_b, document, "2026 Ward Roster"),
        (smith_a, person, "John Smith"),
        (smith_b, person, "John Smith"),
        (wang_a, person, "Wang Ning"),
        (wang_b, person, "Wang Ning"),
        (lin, person, "Lin Zhao"),
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

    let fact =
        |subject: Uuid, predicate: Uuid, object: Uuid, confidence: f32, rec: &'static str| {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                confidence, recorded_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(Uuid::now_v7())
            .bind(kb)
            .bind(subject)
            .bind(predicate)
            .bind(object)
            .bind(confidence)
            .bind(rec.parse::<chrono::DateTime<chrono::Utc>>().unwrap())
        };
    // 两个 John Smith 分得清：雇主不同
    fact(smith_a, works_for, acme, 0.8, "2026-01-01T00:00:00Z")
        .execute(pool)
        .await?;
    fact(smith_b, works_for, st_marys, 0.8, "2026-01-01T00:00:00Z")
        .execute(pool)
        .await?;
    // 同时各有一条更晚、更高置信的「被提到过」——旧排序会挑中它
    fact(
        smith_a,
        mentioned_in,
        report_a,
        0.99,
        "2026-05-01T00:00:00Z",
    )
    .execute(pool)
    .await?;
    fact(
        smith_b,
        mentioned_in,
        report_b,
        0.99,
        "2026-05-01T00:00:00Z",
    )
    .execute(pool)
    .await?;
    // 两个 Wang Ning 都在 Acme：这条事实分不开他们
    fact(wang_a, works_for, acme, 0.8, "2026-02-01T00:00:00Z")
        .execute(pool)
        .await?;
    fact(wang_b, works_for, acme, 0.8, "2026-02-01T00:00:00Z")
        .execute(pool)
        .await?;
    fact(lin, works_for, acme, 0.8, "2026-02-01T00:00:00Z")
        .execute(pool)
        .await?;

    Ok(Fixture {
        org,
        kb,
        smith_a,
        smith_b,
        wang_a,
        wang_b,
        lin,
    })
}

async fn suffix(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<String>> {
    let row: (Option<String>,) = sqlx::query_as("SELECT disambiguator FROM entities WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

#[tokio::test]
async fn a_suffix_comes_from_the_ontology_not_a_word_list() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    for name in ["John Smith", "Wang Ning", "Lin Zhao"] {
        utopia_store::resolution::refresh_disambiguators(&pool, f.kb, name).await?;
    }

    // 1. schema.org 的词汇也认。旧实现的词表里没有 works_for，两个人双双退回
    //    "Person"——分得清清楚楚的两个人，界面上却一模一样
    assert_eq!(
        suffix(&pool, f.smith_a).await?.as_deref(),
        Some("Acme Robotics")
    );
    assert_eq!(
        suffix(&pool, f.smith_b).await?.as_deref(),
        Some("St Mary's Hospital")
    );

    // 2. 更晚、更高置信的 mentioned_in 没有胜出：它没声明过值域，又是事件。
    //    「他被写进过哪份报告」不是他是谁
    assert_ne!(
        suffix(&pool, f.smith_a).await?.as_deref(),
        Some("2026 Field Report")
    );

    // 3. 同一个雇主分不开两个 Wang Ning：那就都写雇主，而不是编一个假的区分。
    //    共同的雇主仍是信息，只是它不承担区分的职责
    assert_eq!(
        suffix(&pool, f.wang_a).await?.as_deref(),
        Some("Acme Robotics")
    );
    assert_eq!(
        suffix(&pool, f.wang_b).await?.as_deref(),
        Some("Acme Robotics")
    );

    // 4. 组内唯一：没有要分开的对象，就不该挂后缀
    assert_eq!(suffix(&pool, f.lin).await?, None);

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    let gone = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
