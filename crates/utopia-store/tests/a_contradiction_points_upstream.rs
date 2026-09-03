//! 0017：派生撞上断言时，让路这件事从静默变成可见。
//!
//! `ceo_of ⊑ works_at`，`works_at` functional。Mira `ceo_of` Acme 推出 Mira `works_at`
//! Acme，而账本里说她 `works_at` Globex。这里守四件事：
//!
//! 1. **派生不落地，而队列里有一行。** `run` 记一条 `derived_contradiction`，left 是被撞
//!    的断言，right 是最后一条前提，detail 写着推出来的三元组；`materialize` 拦下它。
//! 2. **修了就落。** 给旧断言一个结束日期，派生的区间与它不再重叠，下一轮落地，
//!    队列里那一行随之清掉。
//! 3. **认可就落。** 人说两边都对，`accepted` 之后派生照常落地，那一行留着不再报。
//! 4. **派生之间互撞按规则对聚合。** 两个 ceo 推出两条互斥的 works_at，进
//!    `ontology_defects` 一行 `rules_disagree`，两条派生都不落。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::reasoning;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    user: Uuid,
    kb: Uuid,
    ceo_of: Uuid,
    works_at: Uuid,
    mira: Uuid,
    acme: Uuid,
    globex: Uuid,
    initech: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb, user) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let etype = Uuid::now_v7();
    let (ceo_of, works_at) = (Uuid::now_v7(), Uuid::now_v7());
    let (mira, acme, globex, initech) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'contradiction-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'contradiction-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $1 || '@contradiction.test', 'c', 'x')",
    )
    .bind(user)
    .bind(org)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name, materialize_inferences)
         VALUES ($1, $2, 'contradiction-test', TRUE)",
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
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, functional)
         VALUES ($1, $2, 'works_at', 'works at', TRUE)",
    )
    .bind(works_at)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, sub_property_of)
         VALUES ($1, $2, 'ceo_of', 'CEO of', $3)",
    )
    .bind(ceo_of)
    .bind(kb)
    .bind(works_at)
    .execute(pool)
    .await?;
    for (id, name) in [
        (mira, "Mira"),
        (acme, "Acme"),
        (globex, "Globex"),
        (initech, "Initech"),
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
        org,
        user,
        kb,
        ceo_of,
        works_at,
        mira,
        acme,
        globex,
        initech,
    })
}

async fn asserted(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    predicate: Uuid,
    object: Uuid,
    from: Option<&str>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence,
                            valid_from, valid_from_precision)
         VALUES ($1, $2, $3, $4, $5, 0.9, $6::timestamptz, CASE WHEN $6 IS NULL THEN NULL ELSE 'day' END)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(subject)
    .bind(predicate)
    .bind(object)
    .bind(from)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn live_derived(pool: &PgPool, f: &Fixture) -> anyhow::Result<Vec<(Uuid, Uuid)>> {
    Ok(sqlx::query_as(
        "SELECT subject_id, object_id FROM derived_facts
          WHERE kb_id = $1 AND invalidated_at IS NULL ORDER BY subject_id, object_id",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

async fn open_contradictions(
    pool: &PgPool,
    f: &Fixture,
) -> anyhow::Result<Vec<(Uuid, Uuid, serde_json::Value)>> {
    Ok(sqlx::query_as(
        "SELECT id, left_fact, detail FROM axiom_violations
          WHERE kb_id = $1 AND kind = 'derived_contradiction' AND status = 'open'",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn a_contradiction_points_upstream() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // Mira works_at Globex（没写结束日期）；Mira ceo_of Acme 自 2024 起
        let old = asserted(&pool, &f, f.mira, f.works_at, f.globex, Some("2020-01-01")).await?;
        let ceo = asserted(&pool, &f, f.mira, f.ceo_of, f.acme, Some("2024-01-01")).await?;

        // 1. 派生不落地，队列里有一行
        let m = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(m.derived, 1);
        assert_eq!(
            m.blocked, 1,
            "the derivation that hits an assertion stays out"
        );
        assert_eq!(m.inserted, 0);
        assert!(live_derived(&pool, &f).await?.is_empty());

        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.contradictions, 1);
        assert_eq!(r.rules_disagree, 0);
        let rows = open_contradictions(&pool, &f).await?;
        assert_eq!(rows.len(), 1);
        let (vid, left, detail) = &rows[0];
        assert_eq!(*left, old, "left is the assertion that was hit");
        assert_eq!(detail["axiom"], "functional");
        assert_eq!(detail["rule"], "sub_property");
        assert_eq!(detail["subject"], "Mira");
        assert_eq!(detail["predicate"], "works at");
        assert_eq!(detail["object"], "Acme");
        assert_eq!(detail["via_label"], "CEO of");
        assert_eq!(detail["premises"][0], serde_json::json!(ceo));
        let (right,): (Uuid,) =
            sqlx::query_as("SELECT right_fact FROM axiom_violations WHERE id = $1")
                .bind(vid)
                .fetch_one(&pool)
                .await?;
        assert_eq!(right, ceo, "right is the last premise");

        // Review 给的线索：旧断言没写结束日期、派生起得更晚 → stale
        let page = reasoning::open_violations(&pool, f.kb, 50, 0).await?;
        let card = page
            .iter()
            .find(|v| v.id == *vid)
            .expect("card on the page");
        assert_eq!(card.hint.as_deref(), Some("stale"));
        assert_eq!(card.detail["subject"], "Mira");

        // 重跑幂等：还是那一行
        reasoning::run(&pool, f.kb).await?;
        assert_eq!(open_contradictions(&pool, &f).await?.len(), 1);

        // 2. 修了就落：给旧断言一个结束日期，区间不再重叠
        sqlx::query(
            "UPDATE facts SET valid_to = '2023-06-30'::timestamptz, valid_to_precision = 'day'
              WHERE id = $1",
        )
        .bind(old)
        .execute(&pool)
        .await?;
        let m = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(m.blocked, 0);
        assert_eq!(
            m.inserted, 1,
            "once the assertion ends, the derivation lands"
        );
        assert_eq!(live_derived(&pool, &f).await?, vec![(f.mira, f.acme)]);
        reasoning::run(&pool, f.kb).await?;
        assert!(
            open_contradictions(&pool, &f).await?.is_empty(),
            "the queue row clears with the contradiction"
        );

        // 3. 认可就落：把结束日期拿掉，矛盾回来；人说两边都对，派生照常落地
        sqlx::query("UPDATE facts SET valid_to = NULL, valid_to_precision = NULL WHERE id = $1")
            .bind(old)
            .execute(&pool)
            .await?;
        let m = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(m.blocked, 1);
        assert_eq!(m.invalidated, 1, "the landed derivation is withdrawn again");
        reasoning::run(&pool, f.kb).await?;
        let rows = open_contradictions(&pool, &f).await?;
        assert_eq!(rows.len(), 1);
        reasoning::decide(&pool, f.kb, rows[0].0, "accepted", f.user).await?;
        let m = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(m.blocked, 0, "an accepted pair lands");
        assert_eq!(live_derived(&pool, &f).await?, vec![(f.mira, f.acme)]);
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.contradictions, 1, "still counted");
        assert!(
            open_contradictions(&pool, &f).await?.is_empty(),
            "but the accepted row stays resolved and nothing new is opened"
        );

        // 4. 派生之间互撞：先把旧断言闭合掉，让断言不再参与；再来一个 ceo_of Initech，
        //    两条 works_at 由同一条规则推出、互斥——按规则对报一次，两条都不落
        sqlx::query(
            "UPDATE facts SET valid_to = '2023-06-30'::timestamptz, valid_to_precision = 'day'
              WHERE id = $1",
        )
        .bind(old)
        .execute(&pool)
        .await?;
        let ceo2 = asserted(&pool, &f, f.mira, f.ceo_of, f.initech, Some("2024-01-01")).await?;
        let m = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(m.derived, 2);
        assert_eq!(m.blocked, 2, "both sides of a rule clash stay out");
        assert_eq!(m.invalidated, 1, "the one that had landed is withdrawn");
        assert!(live_derived(&pool, &f).await?.is_empty());
        let r = reasoning::run(&pool, f.kb).await?;
        assert_eq!(r.contradictions, 0);
        assert_eq!(r.rules_disagree, 1);
        let defects: Vec<(Uuid, Option<Uuid>, serde_json::Value)> = sqlx::query_as(
            "SELECT subject, other, detail FROM ontology_defects
              WHERE kb_id = $1 AND kind = 'rules_disagree' AND status = 'open'",
        )
        .bind(f.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(defects.len(), 1);
        assert_eq!(defects[0].0, f.ceo_of);
        assert_eq!(defects[0].1, Some(f.ceo_of));
        assert_eq!(defects[0].2["count"], 1);
        assert_eq!(defects[0].2["rules"][0]["axiom"], "functional");
        assert_eq!(defects[0].2["rules"][0]["rule_a"], "sub_property");
        assert_eq!(defects[0].2["rules"][0]["via_a"], "CEO of");
        let page = reasoning::open_defects(&pool, f.kb, 50, 0).await?;
        let card = page
            .iter()
            .find(|d| d.kind == "rules_disagree")
            .expect("the rule clash is on the page");
        assert_eq!(card.subject_label.as_deref(), Some("CEO of"));
        assert_eq!(card.other_label.as_deref(), Some("CEO of"));

        // 撤掉第二个 ceo：规则对的那一行清掉，第一条派生重新落地
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(ceo2)
            .execute(&pool)
            .await?;
        reasoning::run(&pool, f.kb).await?;
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM ontology_defects
              WHERE kb_id = $1 AND kind = 'rules_disagree' AND status = 'open'",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(n, 0, "a rule clash clears when its derivations go");
        let m = reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(m.blocked, 0);
        assert_eq!(live_derived(&pool, &f).await?, vec![(f.mira, f.acme)]);
        anyhow::Ok(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
