//! 一次裁决记下的是「为什么」（0026），打在真库上。
//!
//! 从前 `resolution_reviews` 记 status / decided_by / decided_at，`reason` 记的是
//! 这一对**为什么被排进队列**。人拍板时想的那句话没有地方落，于是先例只有结果：
//! 一次错误的合并被读成"这类该合"，错误洗成政策。
//!
//! 三件事各自会以不同的方式坏掉：
//! - **落库**：理由写进这一行，空白与只有空格算没写（不存一串空格）
//! - **先例带理由**：治理与裁决器读先例走的是审计台账，不是这张表——所以理由
//!   也得进 `detail.why`，`precedents_for` 才读得到，`render_lines` 才写得出
//! - **老行不炸**：0026 之前的决定没有 why，`Precedent.why` 是 None，渲染照旧

use sqlx::PgPool;
use utopia_core::models::{ReviewItem, ReviewSide};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    user: Uuid,
    a: Uuid,
    b: Uuid,
    review: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (user, person) = (Uuid::now_v7(), Uuid::now_v7());
    let (a, b, review) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'why-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'why-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'why-test')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $3, 'Decider', 'x')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("decider-{}@test.local", user.simple()))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'company', 'Company')",
    )
    .bind(person)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(a, "Acme"), (b, "Acme Corp")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(person)
        .bind(name)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO resolution_reviews (id, kb_id, left_id, right_id, score, reason, stage)
         VALUES ($1, $2, $3, $4, 0.6, 'contains', 'human')",
    )
    .bind(review)
    .bind(kb)
    .bind(a)
    .bind(b)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        user,
        a,
        b,
        review,
    })
}

fn item(f: &Fixture) -> ReviewItem {
    let side = |id: Uuid, name: &str| ReviewSide {
        id,
        name: name.into(),
        type_label: Some("Company".into()),
        color: "#888888".into(),
        degree: 0,
        disambiguator: None,
        top_facts: vec![],
    };
    ReviewItem {
        id: f.review,
        left: side(f.a, "Acme"),
        right: side(f.b, "Acme Corp"),
        score: 0.6,
        reason: Some("contains".into()),
        stage: "human".into(),
        created_at: chrono::Utc::now(),
        proposal: None,
    }
}

#[tokio::test]
async fn a_decision_keeps_its_reason_and_the_precedent_carries_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    // 人把它们分开，并说了为什么
    utopia_store::resolution::decide_review(
        &pool,
        f.kb,
        f.review,
        "keep",
        f.user,
        Some("  Acme Corp is the parent company; Acme is the product line  "),
    )
    .await?;
    let stored: (String, Option<String>) =
        sqlx::query_as("SELECT status, rationale FROM resolution_reviews WHERE id = $1")
            .bind(f.review)
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored.0, "kept");
    assert_eq!(
        stored.1.as_deref(),
        Some("Acme Corp is the parent company; Acme is the product line"),
        "理由落在这一行上，两头的空格去掉"
    );

    // 先例走审计：路由把 why 写进 detail，这里照路由的写法记一笔
    utopia_store::audit::record(
        &pool,
        Some(f.kb),
        f.user,
        "review.keep",
        "review",
        Some(f.review),
        serde_json::json!({
            "left": "Acme", "right": "Acme Corp", "score": 0.6,
            "why": "Acme Corp is the parent company; Acme is the product line",
        }),
    )
    .await?;
    let p = utopia_store::governance::precedents_for(&pool, f.kb, &item(&f)).await?;
    assert_eq!(p.same_pair.len(), 1, "同对先例一条");
    assert_eq!(
        p.same_pair[0].why.as_deref(),
        Some("Acme Corp is the parent company; Acme is the product line"),
        "先例带着人写的理由"
    );
    let lines = utopia_store::governance::render_lines(&p);
    assert!(
        lines.iter().any(
            |l| l.contains("kept apart") && l.contains("they wrote: \"Acme Corp is the parent")
        ),
        "提示词里那一行要引用理由：{lines:?}"
    );

    // 老行：没有 why 的决定照样是先例，只是没有理由
    utopia_store::audit::record(
        &pool,
        Some(f.kb),
        f.user,
        "review.keep",
        "review",
        Some(f.review),
        serde_json::json!({ "left": "Acme", "right": "Acme Corp", "score": 0.6 }),
    )
    .await?;
    let p = utopia_store::governance::precedents_for(&pool, f.kb, &item(&f)).await?;
    assert_eq!(p.same_pair.len(), 2);
    assert!(
        p.same_pair.iter().any(|x| x.why.is_none()),
        "0026 之前的行没有理由，读出来是 None 而不是报错"
    );
    let lines = utopia_store::governance::render_lines(&p);
    assert!(
        lines
            .iter()
            .any(|l| l.contains("kept apart") && !l.contains("they wrote")),
        "没理由的那条不该凭空长出一句引文：{lines:?}"
    );

    // 空理由不落库
    let review2 = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO resolution_reviews (id, kb_id, left_id, right_id, score, reason, stage)
         VALUES ($1, $2, $3, $4, 0.6, 'contains', 'human')",
    )
    .bind(review2)
    .bind(f.kb)
    .bind(f.a)
    .bind(f.b)
    .execute(&pool)
    .await?;
    utopia_store::resolution::decide_review(&pool, f.kb, review2, "keep", f.user, Some("   "))
        .await?;
    let blank: Option<String> =
        sqlx::query_scalar("SELECT rationale FROM resolution_reviews WHERE id = $1")
            .bind(review2)
            .fetch_one(&pool)
            .await?;
    assert!(blank.is_none(), "只有空格等于没写");

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(f.user)
        .execute(&pool)
        .await?;
    let gone = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert_eq!(gone.rows_affected(), 1, "一次性 org 没删掉");
    Ok(())
}
