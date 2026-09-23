//! 规则接规则（0030 / #477）。
//!
//! 0021 的规则只读断言，只跑一趟，所以 `A → B → C` 得压成一条规则重写一遍。
//! 现在一轮的结论进下一轮的输入，这里钉的是那件事在真库上的四个后果：
//!
//! - **链接得上**：第二条规则读第一条的结论，前提里一条是断言、一条是派生，
//!   两列各存各的
//! - **派生归类带区间**：它是当条件进的，不是当筛子——2020–2021 是 A 的实体，
//!   2022 的读数不该满足一条 A 上的规则
//! - **叶子变了整条链退场**：抬高第一条规则的阈值，第二条推出来的那行跟着
//!   作废。这不是谁去追的，是「全量重算 + 对账」的自然结果
//! - **自反馈停得下来**：规则推出自己主类的类，第二轮推出的是同一个键，
//!   不动点在那里收敛
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    well: Uuid,
    gas_bearing: Uuid,
    producer: Uuid,
    thc: Uuid,
    completed: Uuid,
    w1: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (well, gas_bearing, producer) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (thc, completed, is_a) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let w1 = Uuid::now_v7();

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'chain-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'chain-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'chain-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (well, "well", "Well"),
        (gas_bearing, "gas_well", "GasBearingWell"),
        (producer, "producer", "Producer"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, key, label, datatype) in [
        (thc, "total_hydrocarbon", "全烃", "number"),
        (completed, "completion", "完井", "text"),
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
    // 实体断言的类是 Well。GasBearingWell 只能是推出来的——这正是要钉的那条路
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
        producer,
        thc,
        completed,
        w1,
    })
}

/// 一条属性事实，区间两端都给死——链上的区间是交出来的，端点含糊就看不出交没交
async fn attr(
    pool: &PgPool,
    f: &Fixture,
    predicate: Uuid,
    value: serde_json::Value,
    from: &str,
    to: Option<&str>,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence)
         VALUES ($1, $2, $3, $4, $5, $6, 'day', $7, $8, 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.w1)
    .bind(predicate)
    .bind(serde_json::json!({ "value": value }))
    .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?)
    .bind(
        to.map(|t| t.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()?,
    )
    .bind(to.map(|_| "day"))
    .execute(pool)
    .await?;
    Ok(id)
}

/// 一条归类规则：主类上的一个属性条件 → 结论类。条件三格捆成一个参数——
/// 它们本来就是一句话
async fn rule(
    pool: &PgPool,
    f: &Fixture,
    name: &str,
    subject: Uuid,
    condition: (Uuid, &str, Option<serde_json::Value>),
    concludes: Uuid,
) -> anyhow::Result<Uuid> {
    let (predicate, op, operand) = condition;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO attribute_rules (id, kb_id, name, subject_type_id, conclusion, conclude_type_id)
         VALUES ($1, $2, $3, $4, 'typing', $5)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(name)
    .bind(subject)
    .bind(concludes)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO attribute_rule_conditions (id, rule_id, seq, predicate_id, op, operand)
         VALUES ($1, $2, 0, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(id)
    .bind(predicate)
    .bind(op)
    .bind(operand)
    .execute(pool)
    .await?;
    Ok(id)
}

#[derive(sqlx::FromRow)]
struct Derived {
    id: Uuid,
    class: Option<String>,
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    invalidated_at: Option<chrono::DateTime<chrono::Utc>>,
}

async fn derived(pool: &PgPool, f: &Fixture) -> anyhow::Result<Vec<Derived>> {
    Ok(sqlx::query_as(
        "SELECT id, object_value ->> 'class' AS class,
                valid_to, invalidated_at
           FROM derived_facts WHERE kb_id = $1 ORDER BY derived_at, id",
    )
    .bind(f.kb)
    .fetch_all(pool)
    .await?)
}

/// `Well → GasBearingWell → Producer`：第二条规则读的是第一条的结论。
///
/// 钉的是链上那一行的**前提长什么样**：一条断言（完井）、一条派生（归类），
/// 分别落在 `premise_fact_id` 与 `premise_derived_id` 上。合成一列的话，
/// 「这一步还要不要再往下问」在库里就答不出来。
#[tokio::test]
async fn a_conclusion_is_a_premise_for_the_next_rule() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let thc_fact = attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
            None,
        )
        .await?;
        let done = attr(
            &pool,
            &f,
            f.completed,
            serde_json::json!("yes"),
            "2023-06-01T00:00:00Z",
            None,
        )
        .await?;
        rule(
            &pool,
            &f,
            "gas-bearing",
            f.well,
            (f.thc, "gt", Some(serde_json::json!(8.0))),
            f.gas_bearing,
        )
        .await?;
        rule(
            &pool,
            &f,
            "producer",
            f.gas_bearing,
            (f.completed, "present", None),
            f.producer,
        )
        .await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.attribute_rules, 2);
        assert_eq!(report.rule_hits, 2, "两条结论：含气井，以及在产井");
        assert!(
            report.rule_rounds >= 2,
            "链上一环一轮，一轮跑不出第二条：{}",
            report.rule_rounds
        );
        assert!(!report.rule_rounds_capped, "两环没到上限");

        let rows = derived(&pool, &f).await?;
        assert_eq!(rows.len(), 2);
        let gas = rows
            .iter()
            .find(|d| d.class.as_deref() == Some("gas_well"))
            .expect("含气井那一行");
        let prod = rows
            .iter()
            .find(|d| d.class.as_deref() == Some("producer"))
            .expect("在产井那一行");

        // 含气井：两条前提都是断言
        let leaf: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT premise_fact_id, premise_derived_id
               FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(gas.id)
        .fetch_all(&pool)
        .await?;
        assert_eq!(leaf, vec![(Some(thc_fact), None)], "含气井只凭那条全烃读数");

        // 在产井：一条断言 + 一条派生。**这一条就是链**
        let chained: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT premise_fact_id, premise_derived_id
               FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(prod.id)
        .fetch_all(&pool)
        .await?;
        assert_eq!(chained.len(), 2, "完井那条读数，加上「它是含气井」那条结论");
        assert!(
            chained.contains(&(Some(done), None)),
            "断言前提落在 premise_fact_id 上"
        );
        assert!(
            chained.contains(&(None, Some(gas.id))),
            "派生前提落在 premise_derived_id 上，指的是含气井那一行"
        );

        // 证明顺着链往下走一层：在产井 → 含气井 → 全烃那句话
        let proof = utopia_store::reasoning::proof(&pool, f.kb, prod.id)
            .await?
            .expect("在产井有证明");
        let step = proof
            .steps
            .iter()
            .find(|s| s.derived)
            .expect("证明里有一步是推出来的");
        assert_eq!(step.premises.len(), 1, "那一步自己也有前提，再往下一层");
        assert!(!step.premises[0].derived, "再下一层是断言，链到此为止");

        // 重跑不该多出行，也不该把自己判成陈旧——链上那一行认的是上一轮留下的
        // 那个行 id，认错了这里会看见「作废一条、插入一条」
        let again = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(again.inserted, 0, "同一条链重跑不再插");
        assert_eq!(again.invalidated, 0, "也不该把自己判成陈旧");
        assert_eq!(derived(&pool, &f).await?.len(), 2);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 推出来的类**带着区间**：它是当条件进的，不是当筛子。
///
/// 一口井 2020–2021 是含气井，完井那条读数在 2022——两段不相交，就不该有
/// 在产井。范围若还是个无区间的筛子，这里会多推出一行，而且那一行的区间
/// 说不出它凭什么成立。
#[tokio::test]
async fn a_concluded_class_only_holds_where_it_was_concluded() -> anyhow::Result<()> {
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
            "2020-01-01T00:00:00Z",
            Some("2021-01-01T00:00:00Z"),
        )
        .await?;
        attr(
            &pool,
            &f,
            f.completed,
            serde_json::json!("yes"),
            "2022-01-01T00:00:00Z",
            None,
        )
        .await?;
        rule(
            &pool,
            &f,
            "gas-bearing",
            f.well,
            (f.thc, "gt", Some(serde_json::json!(8.0))),
            f.gas_bearing,
        )
        .await?;
        rule(
            &pool,
            &f,
            "producer",
            f.gas_bearing,
            (f.completed, "present", None),
            f.producer,
        )
        .await?;

        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        let rows = derived(&pool, &f).await?;
        assert_eq!(
            rows.len(),
            1,
            "只有含气井那一行；完井那条读数落在它成立的区间之外"
        );
        assert_eq!(rows[0].class.as_deref(), Some("gas_well"));
        assert_eq!(
            rows[0].valid_to.map(|t| t.to_rfc3339()),
            Some("2021-01-01T00:00:00+00:00".into()),
            "结论的区间就是那条读数的区间"
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

/// 叶子变了，整条链退场——**而且没有一行代码是为这件事写的**。
///
/// 抬高第一条规则的阈值，含气井不再成立；在产井站在它上面，这一轮压根没被
/// 重新算出来，于是从 `wanted` 里掉出去，跟含气井在同一条语句里作废。
#[tokio::test]
async fn a_chain_retires_with_the_reading_it_stood_on() -> anyhow::Result<()> {
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
            None,
        )
        .await?;
        attr(
            &pool,
            &f,
            f.completed,
            serde_json::json!("yes"),
            "2023-06-01T00:00:00Z",
            None,
        )
        .await?;
        let first = rule(
            &pool,
            &f,
            "gas-bearing",
            f.well,
            (f.thc, "gt", Some(serde_json::json!(8.0))),
            f.gas_bearing,
        )
        .await?;
        rule(
            &pool,
            &f,
            "producer",
            f.gas_bearing,
            (f.completed, "present", None),
            f.producer,
        )
        .await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(derived(&pool, &f).await?.len(), 2);

        // 抬到 20：那条 12.3 的读数不再满足第一条规则
        sqlx::query("UPDATE attribute_rule_conditions SET operand = '20.0' WHERE rule_id = $1")
            .bind(first)
            .execute(&pool)
            .await?;
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.invalidated, 2, "含气井与站在它上面的在产井一起退场");
        assert_eq!(report.inserted, 0);

        let rows = derived(&pool, &f).await?;
        assert_eq!(rows.len(), 2, "作废不是删除：记录轴上留着「曾据此推出」");
        assert!(
            rows.iter().all(|d| d.invalidated_at.is_some()),
            "两行都作废"
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

/// 一条规则读自己推出来的类，不动点照样收敛。
///
/// 第一条把井推成含气井；第二条写在含气井上，推出的还是含气井。第二轮算出来的
/// 是同一个键——主语、谓词、值、区间都一样——于是不再进池子，第三轮无事可做。
/// **上限是兜底，不是靠它停的**：这里 `rule_rounds` 远小于 `MAX_DEPTH`。
#[tokio::test]
async fn a_rule_that_feeds_itself_settles() -> anyhow::Result<()> {
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
            None,
        )
        .await?;
        rule(
            &pool,
            &f,
            "gas-bearing",
            f.well,
            (f.thc, "gt", Some(serde_json::json!(8.0))),
            f.gas_bearing,
        )
        .await?;
        // 主类是含气井，结论也是含气井：它读的正是自己推出来的那条
        rule(
            &pool,
            &f,
            "still gas-bearing",
            f.gas_bearing,
            (f.thc, "gt", Some(serde_json::json!(8.0))),
            f.gas_bearing,
        )
        .await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.rule_hits, 1, "两条规则推出的是同一条结论，只算一条");
        assert!(
            report.rule_rounds < utopia_reason::MAX_DEPTH,
            "收敛靠的是「这一轮没有新键」，不是撞上上限：{}",
            report.rule_rounds
        );
        assert!(!report.rule_rounds_capped);
        assert_eq!(derived(&pool, &f).await?.len(), 1, "库里就一行");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 结论没变，理由变了：那一行留着，**证明跟着换**。
///
/// 对账的键是主宾谓加区间，前提不在里面——同一句话换一条读数支撑，行不动。
/// 不重写的话，它会一直挂着上一轮那条已经撤了的读数，而链让这件事更容易撞上：
/// 站在它上面的那条前提可能刚刚作废。
#[tokio::test]
async fn a_conclusion_that_stands_on_a_new_reading_gets_a_new_proof() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        let first = attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(12.3),
            "2023-06-01T00:00:00Z",
            None,
        )
        .await?;
        rule(
            &pool,
            &f,
            "gas-bearing",
            f.well,
            (f.thc, "gt", Some(serde_json::json!(8.0))),
            f.gas_bearing,
        )
        .await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        let rows = derived(&pool, &f).await?;
        assert_eq!(rows.len(), 1);
        let row = rows[0].id;

        // 撤掉那条读数，换一条区间一样、值也过阈值的：结论一字不差，凭的却是
        // 另一句话
        sqlx::query("UPDATE facts SET invalidated_at = now() WHERE id = $1")
            .bind(first)
            .execute(&pool)
            .await?;
        let second = attr(
            &pool,
            &f,
            f.thc,
            serde_json::json!(30.0),
            "2023-06-01T00:00:00Z",
            None,
        )
        .await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.inserted, 0, "结论没变，不该多一行");
        assert_eq!(report.invalidated, 0, "也不该把它判成陈旧");
        assert_eq!(report.reproved, 1, "证明换了一条");

        let now: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT premise_fact_id, premise_derived_id
               FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(row)
        .fetch_all(&pool)
        .await?;
        assert_eq!(now, vec![(Some(second), None)], "挂的是现在成立的那条读数");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
