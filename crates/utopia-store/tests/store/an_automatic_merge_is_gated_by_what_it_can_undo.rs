//! 执行闸门（0027）：一次自动合并能不能落地，看它撤得回什么。打在真库上。
//!
//! 三种会离开图的东西，各造一个最小的例子，再各撤一次，证明闸门看的是「现在」：
//! - **矛盾**：两边各持一条同一个 functional 谓词、指向不同实体的事实——合了就是一条
//!   `functional` 违规。指向同一个实体的不算；作废了的不算
//! - **派生**：任一边参与的、还成立的派生事实——合并改前提，派生会被重写
//! - **回答**：任一边在对话里被认过——它是人在问的东西
//!
//! 什么都不牵动的一对，闸门不拦：把握够就照旧自动

use sqlx::PgPool;
use utopia_store::execution_gate::{hold, impact_of, Hold};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    user: Uuid,
    ceo_of: Uuid,
    part_of: Uuid,
    a: Uuid,
    b: Uuid,
    c1: Uuid,
    c2: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (user, company) = (Uuid::now_v7(), Uuid::now_v7());
    let (ceo_of, part_of) = (Uuid::now_v7(), Uuid::now_v7());
    let (a, b, c1, c2) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'gate-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'gate-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'gate-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $3, 'Asker', 'x')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("asker-{}@test.local", user.simple()))
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'company', 'Company')",
    )
    .bind(company)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, functional)
         VALUES ($1, $2, 'ceo_of', 'CEO of', TRUE)",
    )
    .bind(ceo_of)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'part_of', 'part of')",
    )
    .bind(part_of)
    .bind(kb)
    .execute(pool)
    .await?;
    for (id, name) in [(a, "Acme"), (b, "Acme Corp"), (c1, "Alice"), (c2, "Bob")] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(company)
        .bind(name)
        .execute(pool)
        .await?;
    }
    Ok(Fixture {
        org,
        kb,
        user,
        ceo_of,
        part_of,
        a,
        b,
        c1,
        c2,
    })
}

async fn fact(pool: &PgPool, kb: Uuid, s: Uuid, p: Uuid, o: Uuid) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(kb)
    .bind(s)
    .bind(p)
    .bind(o)
    .execute(pool)
    .await?;
    Ok(id)
}

#[tokio::test]
async fn a_merge_that_would_leave_the_graph_is_held() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    // 什么都没有：不拦
    let none = impact_of(&pool, f.kb, f.a, f.b).await?;
    assert_eq!(hold(&none), None, "两边空空的一对，闸门不该拦：{none:?}");

    // 矛盾：a 的 CEO 是 Alice，b 的 CEO 是 Bob。合了就是一个实体两个 CEO
    fact(&pool, f.kb, f.a, f.ceo_of, f.c1).await?;
    let clash = fact(&pool, f.kb, f.b, f.ceo_of, f.c2).await?;
    let i = impact_of(&pool, f.kb, f.a, f.b).await?;
    assert_eq!(i.contradictions, vec!["CEO of".to_string()]);
    assert_eq!(hold(&i), Some(Hold::Contradiction("CEO of".into())));
    // 反过来问也一样
    let i = impact_of(&pool, f.kb, f.b, f.a).await?;
    assert_eq!(hold(&i), Some(Hold::Contradiction("CEO of".into())));

    // 撤掉 b 的那条：矛盾没了。再给 b 一条指向同一个 Alice 的：同一个值不是矛盾
    sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
        .bind(clash)
        .execute(&pool)
        .await?;
    fact(&pool, f.kb, f.b, f.ceo_of, f.c1).await?;
    let i = impact_of(&pool, f.kb, f.a, f.b).await?;
    assert!(i.contradictions.is_empty(), "作废的与同值的都不算：{i:?}");
    assert_eq!(hold(&i), None);

    // 派生：一条靠着 a 推出来的还成立的派生
    let rule = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO rules (id, kb_id, predicate_id, kind) VALUES ($1, $2, $3, 'transitive')",
    )
    .bind(rule)
    .bind(f.kb)
    .bind(f.part_of)
    .execute(&pool)
    .await?;
    let derived = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO derived_facts (id, kb_id, subject_id, predicate_id, object_id, rule_id)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(derived)
    .bind(f.kb)
    .bind(f.a)
    .bind(f.part_of)
    .bind(f.c2)
    .bind(rule)
    .execute(&pool)
    .await?;
    let i = impact_of(&pool, f.kb, f.a, f.b).await?;
    assert_eq!(i.derived, 1);
    assert_eq!(hold(&i), Some(Hold::Derived(1)));
    sqlx::query("UPDATE derived_facts SET invalidated_at = now() WHERE id = $1")
        .bind(derived)
        .execute(&pool)
        .await?;
    assert_eq!(
        hold(&impact_of(&pool, f.kb, f.a, f.b).await?),
        None,
        "推翻了的派生不算"
    );

    // 回答：b 在一轮回答里被认过。人的那一轮与认了别人的那一轮都不算
    let conv = Uuid::now_v7();
    sqlx::query("INSERT INTO conversations (id, kb_id, user_id) VALUES ($1, $2, $3)")
        .bind(conv)
        .bind(f.kb)
        .bind(f.user)
        .execute(&pool)
        .await?;
    for (role, who) in [("user", f.b), ("assistant", f.c2), ("assistant", f.b)] {
        sqlx::query(
            "INSERT INTO conversation_messages (id, conversation_id, role, content, resolved)
             VALUES ($1, $2, $3, 'hi', $4)",
        )
        .bind(Uuid::now_v7())
        .bind(conv)
        .bind(role)
        .bind(serde_json::json!([{ "id": who.to_string(), "name": "x", "type": null }]))
        .execute(&pool)
        .await?;
    }
    let i = impact_of(&pool, f.kb, f.a, f.b).await?;
    assert_eq!(i.answered, 1, "只数认了 a 或 b 的那些回答：{i:?}");
    assert_eq!(hold(&i), Some(Hold::Answered(1)));
    assert_eq!(hold(&i).unwrap().to_string(), "answered 1");

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

/// 一个值接替了另一个：两个名字各挂一个、合起来是先后接替的时间线，不是矛盾。
/// 同一天开始的两个值才排不开（Blackbaud 总部租约：房东先 HPBB1、后 BBHQ1，
/// 「Lease Agreement」与「Lease Agreement dated May 16, 2016」各挂一个，从前闸门把这一对
/// 当矛盾留给人，同一份租约就一直是两个实体）
#[tokio::test]
async fn a_value_that_took_over_from_another_is_not_a_contradiction() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let dated = |s: Uuid, o: Uuid, from: &'static str| {
        let pool = pool.clone();
        let (kb, p) = (f.kb, f.ceo_of);
        async move {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                    valid_from, valid_from_precision)
                 VALUES ($1, $2, $3, $4, $5, $6::timestamptz, 'day')",
            )
            .bind(Uuid::now_v7())
            .bind(kb)
            .bind(s)
            .bind(p)
            .bind(o)
            .bind(format!("{from}T00:00:00Z"))
            .execute(&pool)
            .await?;
            anyhow::Ok(())
        }
    };
    dated(f.a, f.c1, "2019-01-01").await?;
    dated(f.b, f.c2, "2021-01-01").await?;
    let succession = impact_of(&pool, f.kb, f.a, f.b).await?;

    let g = seed(&pool).await?;
    let same_day = |s: Uuid, o: Uuid| {
        let pool = pool.clone();
        let (kb, p) = (g.kb, g.ceo_of);
        async move {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                    valid_from, valid_from_precision)
                 VALUES ($1, $2, $3, $4, $5, '2020-01-01T00:00:00Z', 'day')",
            )
            .bind(Uuid::now_v7())
            .bind(kb)
            .bind(s)
            .bind(p)
            .bind(o)
            .execute(&pool)
            .await?;
            anyhow::Ok(())
        }
    };
    same_day(g.a, g.c1).await?;
    same_day(g.b, g.c2).await?;
    let clash = impact_of(&pool, g.kb, g.a, g.b).await?;

    for fx in [&f, &g] {
        sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
            .bind(fx.kb)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(fx.user)
            .execute(&pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(fx.org)
            .execute(&pool)
            .await?;
    }
    assert_eq!(hold(&succession), None, "先后接替不是矛盾：{succession:?}");
    assert_eq!(hold(&clash), Some(Hold::Contradiction("CEO of".into())));
    Ok(())
}

#[tokio::test]
async fn a_clash_one_side_already_had_is_not_the_merges() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    // b 自己已经挂着两个没日期的值；a 只挂着其中一个
    fact(&pool, f.kb, f.b, f.ceo_of, f.c1).await?;
    fact(&pool, f.kb, f.b, f.ceo_of, f.c2).await?;
    fact(&pool, f.kb, f.a, f.ceo_of, f.c1).await?;
    let impact = impact_of(&pool, f.kb, f.a, f.b).await?;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(f.user)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    assert!(
        impact.contradictions.is_empty(),
        "the clash was on b before the merge: {impact:?}"
    );
    Ok(())
}
