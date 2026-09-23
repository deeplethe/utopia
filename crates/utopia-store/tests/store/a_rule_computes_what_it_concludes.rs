//! 算出来的结论打在真库上（0032 / #488）。
//!
//! 算式本身的用例在 `utopia-reason::rules`，纯逻辑、不起库。这里钉的是那一层
//! 看不见的三样：
//!
//! - 一棵存进 `conclude_expr` 的树，**编译回来还是同一棵**，算得出同一个数
//! - **算式读的那几条读数一并进前提**——「这个数凭什么」在库里也得是真的
//! - 写坏的算式在**写的时候**就被拦下，而不是等到物化时表现成「不推东西」
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::business_rules::ConditionInput;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    well: Uuid,
    revenue: Uuid,
    cost: Uuid,
    margin: Uuid,
    w1: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let well = Uuid::now_v7();
    let (revenue, cost, margin) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let w1 = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'calc-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'calc-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'calc-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'well', 'Well')")
        .bind(well)
        .bind(kb)
        .execute(pool)
        .await?;
    for (id, key) in [(revenue, "revenue"), (cost, "cost"), (margin, "margin")] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype)
             VALUES ($1, $2, $3, $3, 'attribute', 'number')",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, 'W-1')",
    )
    .bind(w1)
    .bind(kb)
    .bind(well)
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        well,
        revenue,
        cost,
        margin,
        w1,
    })
}

async fn attr(pool: &PgPool, f: &Fixture, predicate: Uuid, value: f64) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                            valid_from, valid_from_precision, confidence)
         VALUES ($1, $2, $3, $4, $5, '2023-06-01T00:00:00Z', 'day', 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.w1)
    .bind(predicate)
    .bind(serde_json::json!({ "value": value }))
    .execute(pool)
    .await?;
    Ok(id)
}

/// 只有一个条件：revenue 有记录。算式读的是 revenue 与 cost
fn present_revenue(f: &Fixture) -> Vec<ConditionInput> {
    vec![ConditionInput {
        group: 0,
        predicate_id: f.revenue,
        op: "present".into(),
        operand: None,
    }]
}

fn margin_expr(f: &Fixture) -> serde_json::Value {
    serde_json::json!({
        "op": "sub",
        "l": { "attr": f.revenue.to_string() },
        "r": { "attr": f.cost.to_string() },
    })
}

/// margin = revenue − cost，**走 `create` 写进去**（不是直接 INSERT：要过的
/// 正是校验与序列化那一段），跑一遍，看落下来的值和前提。
#[tokio::test]
async fn a_computed_conclusion_lands_with_the_readings_it_read() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let rev = attr(&pool, &f, f.revenue, 300.0).await?;
        let cost = attr(&pool, &f, f.cost, 120.0).await?;

        let rule = utopia_store::business_rules::create(
            &pool,
            f.kb,
            "margin",
            "",
            f.well,
            "computed",
            None,
            Some(f.margin),
            None,
            Some(margin_expr(&f)),
            &present_revenue(&f),
        )
        .await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.attribute_rules, 1, "规则要编译得出来");
        assert_eq!(report.rule_hits, 1);

        let rows: Vec<(Uuid, Option<serde_json::Value>, Option<Uuid>)> = sqlx::query_as(
            "SELECT id, object_value, attribute_rule_id FROM derived_facts WHERE kb_id = $1",
        )
        .bind(f.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].2, Some(rule), "结论指回它的规则");
        assert_eq!(
            rows[0]
                .1
                .as_ref()
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_f64()),
            Some(180.0),
            "300 − 120"
        );

        // **算式读的两条都在前提里。** 条件只提到 revenue，可 cost 也让这个数
        // 成立——漏掉它，这一行就说不出自己凭什么，改了 cost 也不会退场
        let premises: Vec<(Option<Uuid>,)> = sqlx::query_as(
            "SELECT premise_fact_id FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(rows[0].0)
        .fetch_all(&pool)
        .await?;
        let got: Vec<Uuid> = premises.into_iter().filter_map(|(x,)| x).collect();
        assert_eq!(got.len(), 2);
        assert!(got.contains(&rev) && got.contains(&cost));

        // 改掉 cost，值跟着变——这正是「算完抄回来当断言」做不到的那一半
        sqlx::query("UPDATE facts SET object_value = $2 WHERE id = $1")
            .bind(cost)
            .bind(serde_json::json!({ "value": 50.0 }))
            .execute(&pool)
            .await?;
        let again = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(again.invalidated, 1, "180 那一行退场");
        assert_eq!(again.inserted, 1, "250 那一行落下");
        let live: Vec<(Option<serde_json::Value>,)> = sqlx::query_as(
            "SELECT object_value FROM derived_facts
              WHERE kb_id = $1 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(live.len(), 1);
        assert_eq!(
            live[0]
                .0
                .as_ref()
                .and_then(|v| v.get("value"))
                .and_then(|v| v.as_f64()),
            Some(250.0)
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 缺一条读数**不落行**，而不是落一行零。没记不等于零（0032）
#[tokio::test]
async fn a_missing_reading_lands_nothing() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        attr(&pool, &f, f.revenue, 300.0).await?;
        utopia_store::business_rules::create(
            &pool,
            f.kb,
            "margin",
            "",
            f.well,
            "computed",
            None,
            Some(f.margin),
            None,
            Some(margin_expr(&f)),
            &present_revenue(&f),
        )
        .await?;
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.rule_hits, 0, "cost 没记，margin 就不该有");
        let n: (i64,) = sqlx::query_as("SELECT count(*) FROM derived_facts WHERE kb_id = $1")
            .bind(f.kb)
            .fetch_one(&pool)
            .await?;
        assert_eq!(n.0, 0);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 写坏的算式在**写的时候**就被拒，而不是等到物化时表现成「这条规则不推东西」。
///
/// `not_in` 那次（#494）正是后一种：一条读不懂的条件让整条规则被静默跳过，
/// 而规则页上它开着、看着好好的。这一组用例是那次的教训。
#[tokio::test]
async fn a_broken_expression_is_refused_where_it_is_written() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        for (expr, why) in [
            (
                serde_json::json!({ "op": "pow", "l": { "const": 2 }, "r": { "const": 3 } }),
                "四则以外的运算不收",
            ),
            (
                serde_json::json!({ "const": 12 }),
                "一条读数都不读的算式就是个常量，该走 attribute 那一支",
            ),
            (
                serde_json::json!({ "attr": "not-a-uuid" }),
                "认不出的谓词不收",
            ),
            (
                serde_json::json!({
                    "op": "sub",
                    "l": { "attr": f.revenue.to_string() },
                    "r": { "attr": Uuid::now_v7().to_string() },
                }),
                "不在这个库里的属性不收",
            ),
        ] {
            let r = utopia_store::business_rules::create(
                &pool,
                f.kb,
                &format!("r-{}", Uuid::now_v7()),
                "",
                f.well,
                "computed",
                None,
                Some(f.margin),
                None,
                Some(expr),
                &present_revenue(&f),
            )
            .await;
            assert!(r.is_err(), "{why}");
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
