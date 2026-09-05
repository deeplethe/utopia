//! 业务规则打在真库上（0021 / #277）。
//!
//! 求值本身的用例在 `utopia-reason::rules`，纯逻辑、不起库。这里钉的是那一层
//! 看不见的四样：
//!
//! - 命中真的落进 `derived_facts` 的**字面值通道**，并挂着 `attribute_rule_id`
//! - 前提链进 `fact_derivations`——「这口井凭哪两条读数」
//! - **重跑不产生第二行**：同一性索引把 NULL 宾语认成同一条（这是拓宽表时
//!   最容易漏的一处，漏了就是每轮多一行）
//! - 阈值抬高后重跑，旧结论**作废而不是删除**，记录轴上留着「曾据此推出」
//!
//! 还钉一条与公理那一趟的相处：两趟合流进同一次对账，谁也不该把对方的行
//! 判成陈旧。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    well: Uuid,
    gas_bearing: Uuid,
    thc: Uuid,
    category: Uuid,
    is_a: Uuid,
    w1: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (well, gas_bearing) = (Uuid::now_v7(), Uuid::now_v7());
    let (thc, category, is_a) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let w1 = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'rule-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'rule-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'rule-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (well, "well", "Well"),
        (gas_bearing, "gas_well", "GasBearingWell"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    // 两个属性谓词 + 内建 is_a
    for (id, key, label, datatype) in [
        (thc, "total_hydrocarbon", "全烃", "number"),
        (category, "interpretation_category", "解释结论", "text"),
        (is_a, "is_a", "is a", "text"),
    ] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, builtin)
             VALUES ($1, $2, $3, $4, 'attribute', $5, $6)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .bind(label)
        .bind(datatype)
        .bind(key == "is_a")
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
        gas_bearing,
        thc,
        category,
        is_a,
        w1,
    })
}

/// 一条属性事实。
async fn attr(
    pool: &PgPool,
    f: &Fixture,
    predicate: Uuid,
    value: serde_json::Value,
    from: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                            valid_from, valid_from_precision, confidence)
         VALUES ($1, $2, $3, $4, $5, $6, 'day', 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.w1)
    .bind(predicate)
    .bind(serde_json::json!({ "value": value }))
    .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 一条规则：全烃 > 阈值 且 解释结论 ∈ {气测异常, 气测异常后效} → GasBearingWell
async fn rule(pool: &PgPool, f: &Fixture, threshold: f64) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion, conclude_type_id)
         VALUES ($1, $2, 'gas-bearing', $3, 'typing', $4)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.well)
    .bind(f.gas_bearing)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO attribute_rule_conditions (id, rule_id, seq, predicate_id, op, operand)
         VALUES ($1, $2, 0, $3, 'gt', $4), ($5, $2, 1, $6, 'in', $7)",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(f.thc)
    .bind(serde_json::json!(threshold))
    .bind(Uuid::now_v7())
    .bind(f.category)
    .bind(serde_json::json!(["气测异常", "气测异常后效"]))
    .execute(pool)
    .await?;
    Ok(id)
}

#[derive(sqlx::FromRow)]
struct Derived {
    id: Uuid,
    object_value: Option<serde_json::Value>,
    object_id: Option<Uuid>,
    attribute_rule_id: Option<Uuid>,
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    invalidated_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn derived(pool: &PgPool, f: &Fixture) -> anyhow::Result<Vec<Derived>> {
    Ok(sqlx::query_as(
        "SELECT id, object_value, object_id, attribute_rule_id, valid_from, invalidated_at
           FROM derived_facts WHERE kb_id = $1 ORDER BY derived_at",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn a_rule_types_a_well_and_says_which_readings_did_it() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let a = attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        let b = attr(
            &pool,
            &f,
            f.category,
            serde_json::json!("气测异常"),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        let r = rule(&pool, &f, 8.0).await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.attribute_rules, 1, "规则要被读进来");
        assert_eq!(report.rule_hits, 1, "两条读数满足合取");

        let rows = derived(&pool, &f).await?;
        assert_eq!(rows.len(), 1, "一条派生归类");
        let d = &rows[0];
        assert!(d.object_id.is_none(), "归类走字面值通道，没有实体宾语");
        assert_eq!(
            d.object_value
                .as_ref()
                .and_then(|v| v.get("class"))
                .and_then(|v| v.as_str()),
            Some("gas_well"),
            "没有 IRI 时退回 key，记的是类而不是标签"
        );
        assert_eq!(d.attribute_rule_id, Some(r), "结论指回它的规则");
        assert_eq!(
            d.valid_from.map(|t| t.to_rfc3339()),
            Some("2023-06-01T00:00:00+00:00".into()),
            "区间取两条前提的交集"
        );

        // 前提链：凭哪两条读数
        let premises: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT premise_fact_id FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(d.id)
        .fetch_all(&pool)
        .await?;
        let got: Vec<Uuid> = premises.into_iter().map(|(x,)| x).collect();
        assert_eq!(got.len(), 2, "两条前提都要记下来");
        assert!(got.contains(&a) && got.contains(&b));

        // **重跑不该多一行。** 字面值结论的 object_id 是 NULL，而 Postgres 默认
        // 把 NULL 当互不相同——同一性索引写错的话这里每跑一轮多一条
        let again = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(again.inserted, 0, "同一条结论重跑不再插");
        assert_eq!(again.invalidated, 0, "也不该把自己判成陈旧");
        assert_eq!(derived(&pool, &f).await?.len(), 1, "库里还是一行");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 阈值抬到读数之上再跑：结论**作废而不是删除**——记录轴上留着「我们曾据此推出」
#[tokio::test]
async fn raising_the_threshold_invalidates_without_deleting() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        attr(
            &pool,
            &f,
            f.category,
            serde_json::json!("气测异常"),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        let r = rule(&pool, &f, 8.0).await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(derived(&pool, &f).await?.len(), 1);

        // 阈值抬到 20：同一份数据不再满足
        sqlx::query(
            "UPDATE attribute_rule_conditions SET operand = '20' WHERE rule_id = $1 AND seq = 0",
        )
        .bind(r)
        .execute(&pool)
        .await?;
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.rule_hits, 0, "抬高之后不再命中");
        assert_eq!(report.invalidated, 1);

        let rows = derived(&pool, &f).await?;
        assert_eq!(rows.len(), 1, "行还在——作废不是删除");
        assert!(rows[0].invalidated_at.is_some(), "标了作废时刻");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 关掉规则等同于前提消失：结论下一轮失效，而规则行还在
#[tokio::test]
async fn disabling_a_rule_retires_what_it_concluded() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        attr(
            &pool,
            &f,
            f.category,
            serde_json::json!("气测异常"),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        let r = rule(&pool, &f, 8.0).await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;

        sqlx::query("UPDATE attribute_rules SET enabled = FALSE WHERE id = $1")
            .bind(r)
            .execute(&pool)
            .await?;
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.attribute_rules, 0, "关掉的规则不参与求值");
        assert_eq!(report.invalidated, 1, "它推出来的东西跟着退场");
        let rows = derived(&pool, &f).await?;
        assert!(rows[0].invalidated_at.is_some());
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 撤掉一条前提事实，结论跟着失效——与公理那一趟同一条路
#[tokio::test]
async fn retracting_a_reading_retires_the_conclusion() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let a = attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        attr(
            &pool,
            &f,
            f.category,
            serde_json::json!("气测异常"),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        rule(&pool, &f, 8.0).await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(derived(&pool, &f).await?[0].invalidated_at, None);

        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(a)
            .execute(&pool)
            .await?;
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.rule_hits, 0, "少了全烃这条前提就不成立");
        assert_eq!(report.invalidated, 1);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 派生的结论**不写 `entities.type_id`**（0021 决策 2）：断言类型是人的，
/// 规则的结论是叠在上面的一层
#[tokio::test]
async fn the_asserted_type_is_left_alone() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        attr(
            &pool,
            &f,
            f.category,
            serde_json::json!("气测异常"),
            "2023-06-01T00:00:00Z",
        )
        .await?;
        rule(&pool, &f, 8.0).await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;

        let (t,): (Option<Uuid>,) = sqlx::query_as("SELECT type_id FROM entities WHERE id = $1")
            .bind(f.w1)
            .fetch_one(&pool)
            .await?;
        assert_eq!(t, Some(f.well), "实体的断言类型仍然是 Well");
        let rows = derived(&pool, &f).await?;
        assert_eq!(rows.len(), 1, "GasBearingWell 只作为派生存在");
        assert_eq!(rows[0].object_id, None);
        // 结论落在内建 is_a 上
        let (p,): (Uuid,) = sqlx::query_as("SELECT predicate_id FROM derived_facts WHERE id = $1")
            .bind(rows[0].id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(p, f.is_a);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
