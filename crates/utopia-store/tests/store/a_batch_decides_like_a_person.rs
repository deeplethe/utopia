//! 批量裁决与按类型筛重复项（#428），打在真库上。
//!
//! 要钉住的行为：
//!
//! - **筛选按类型分三档**：全部 / 两边同一个类型（都有类型且相等）/ 两边类型冲突
//!   （都有类型且不等）。没类型的一侧哪档都不进——不知道的不能当成一样，也不能
//!   当成不一样
//! - **计数与筛选同一套口径**：左栏的两个数就是这两档各多少条
//! - **批量 = 逐条的人工裁决**：每一条都走 `decide_review`，合并方向、状态、
//!   decided_by 与单条一模一样；一条失败（不存在、已经裁过）不拖累其余的，
//!   结果里逐条说明；动作不认识整批拒绝
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::resolution::TypeFilter;
use uuid::Uuid;

struct Seed {
    kb: Uuid,
    org: Uuid,
    /// 同名同类（两个 Person 都叫 Zhang Wei）
    same_a: Uuid,
    /// 同名类型冲突（Apple 的 Organization 与 Apple 的 Person）
    conflict: Uuid,
    /// 同名同类（两个 Person 都叫 Li Si）
    same_b: Uuid,
    /// 一侧没有类型：哪档都不进
    untyped: Uuid,
    pair_a: (Uuid, Uuid),
    /// 裁决的人：entity_merges.merged_by 与 resolution_reviews.decided_by 都指着 users
    user: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Seed> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, company) = (Uuid::now_v7(), Uuid::now_v7());
    let user = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'batch-decide-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $3, 'Batch Tester', 'x')",
    )
    .bind(user)
    .bind(org)
    .bind(format!("batch-{}@test.local", user.simple()))
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'batch-decide-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'batch-decide-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (company, "org", "Organization"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    let entity = |type_id: Option<Uuid>, name: &str| {
        let id = Uuid::now_v7();
        let name = name.to_string();
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
            )
            .bind(id)
            .bind(kb)
            .bind(type_id)
            .bind(name)
            .execute(&pool)
            .await
            .map(|_| id)
        }
    };
    let a1 = entity(Some(person), "Zhang Wei").await?;
    let a2 = entity(Some(person), "Zhang Wei").await?;
    let b1 = entity(Some(company), "Apple").await?;
    let b2 = entity(Some(person), "Apple").await?;
    let c1 = entity(Some(person), "Li Si").await?;
    let c2 = entity(Some(person), "Li Si").await?;
    let d1 = entity(Some(person), "Wang Wu").await?;
    let d2 = entity(None, "Wang Wu").await?;

    let review = |l: Uuid, r: Uuid, score: f32| {
        let id = Uuid::now_v7();
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO resolution_reviews (id, kb_id, left_id, right_id, score, reason, stage)
                 VALUES ($1, $2, $3, $4, $5, 'namesake', 'human')",
            )
            .bind(id)
            .bind(kb)
            .bind(l)
            .bind(r)
            .bind(score)
            .execute(&pool)
            .await
            .map(|_| id)
        }
    };
    let same_a = review(a1, a2, 1.0).await?;
    let conflict = review(b1, b2, 1.0).await?;
    let same_b = review(c1, c2, 1.0).await?;
    let untyped = review(d1, d2, 1.0).await?;
    Ok(Seed {
        kb,
        org,
        same_a,
        conflict,
        same_b,
        untyped,
        pair_a: (a1, a2),
        user,
    })
}

async fn teardown(pool: &PgPool, s: &Seed) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(s.kb)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(s.org)
        .execute(pool)
        .await?;
    Ok(())
}

async fn run(pool: &PgPool, s: &Seed) -> anyhow::Result<()> {
    // 三档筛选
    let ids = |items: Vec<utopia_core::models::ReviewItem>| {
        let mut v: Vec<Uuid> = items.into_iter().map(|i| i.id).collect();
        v.sort();
        v
    };
    let all =
        ids(utopia_store::resolution::list_reviews(pool, s.kb, TypeFilter::Any, 100, 0).await?);
    assert_eq!(all.len(), 4, "全部四对都在等");
    let mut same =
        ids(utopia_store::resolution::list_reviews(pool, s.kb, TypeFilter::Same, 100, 0).await?);
    let mut want_same = vec![s.same_a, s.same_b];
    same.sort();
    want_same.sort();
    assert_eq!(same, want_same, "同类：两对同名 Person");
    let conflict =
        ids(
            utopia_store::resolution::list_reviews(pool, s.kb, TypeFilter::Conflict, 100, 0)
                .await?,
        );
    assert_eq!(conflict, vec![s.conflict], "冲突：Organization 对 Person");
    assert!(
        !same.contains(&s.untyped) && !conflict.contains(&s.untyped),
        "一侧没类型的哪档都不进"
    );

    // 计数与筛选同一套
    let counts = utopia_store::review::counts(pool, s.kb).await?;
    assert_eq!(counts.duplicates, 4);
    assert_eq!(counts.duplicates_same_type, 2);
    assert_eq!(counts.duplicates_type_conflict, 1);

    // 动作不认识：整批拒绝，什么都没动
    assert!(utopia_store::resolution::decide_reviews(
        pool,
        s.kb,
        &[s.same_a],
        "smash",
        Uuid::now_v7(),
        None,
    )
    .await
    .is_err());

    // 批量合并：两条真的、一条不存在的
    let user = s.user;
    let ghost = Uuid::now_v7();
    let outcomes = utopia_store::resolution::decide_reviews(
        pool,
        s.kb,
        &[s.same_a, ghost, s.same_b],
        "merge",
        user,
        None,
    )
    .await?;
    assert_eq!(outcomes.len(), 3, "每个 id 一条结果，顺序照旧");
    assert!(
        outcomes[0].error.is_none(),
        "第一对合了: {:?}",
        outcomes[0].error
    );
    assert!(outcomes[1].error.is_some(), "不存在的那条报出来");
    assert!(outcomes[2].error.is_none(), "第三对不受第二条拖累");

    let (status, decided_by): (String, Option<Uuid>) =
        sqlx::query_as("SELECT status, decided_by FROM resolution_reviews WHERE id = $1")
            .bind(s.same_a)
            .fetch_one(pool)
            .await?;
    assert_eq!(status, "merged");
    assert_eq!(decided_by, Some(user), "记的是这个人，与单条裁决一样");
    let merged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM entities WHERE id = ANY($1) AND merged_into IS NOT NULL",
    )
    .bind(vec![s.pair_a.0, s.pair_a.1])
    .fetch_one(pool)
    .await?;
    assert_eq!(merged, 1, "一对里恰好一个被并进另一个");

    // 已经裁过的再裁一次：报错、不重复合并
    let again =
        utopia_store::resolution::decide_reviews(pool, s.kb, &[s.same_a], "keep", user, None)
            .await?;
    assert!(again[0].error.is_some());

    // 批量分开
    let kept =
        utopia_store::resolution::decide_reviews(pool, s.kb, &[s.conflict], "keep", user, None)
            .await?;
    assert!(kept[0].error.is_none());
    let counts = utopia_store::review::counts(pool, s.kb).await?;
    assert_eq!(counts.duplicates, 1, "只剩没类型的那对");
    assert_eq!(counts.duplicates_same_type, 0);
    assert_eq!(counts.duplicates_type_conflict, 0);
    Ok(())
}

#[tokio::test]
async fn a_batch_decides_like_a_person() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let s = seed(&pool).await?;
    let outcome = run(&pool, &s).await;
    teardown(&pool, &s).await?;
    outcome
}
