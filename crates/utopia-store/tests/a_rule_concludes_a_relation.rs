//! A joined rule has to see both ends of the edge in the database (0047).
//!
//! The pure evaluator already knows `Side::Y`. This file pins the loader
//! contract behind it: `X` remains scoped by the rule subject type, while `Y`
//! may be any entity the declared join reaches. It also checks that the
//! persisted row carries its object and the full three-part proof.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_store::business_rules::{self, ConclusionInput, ConditionInput};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    well: Uuid,
    pressure: Uuid,
    depth: Uuid,
    supplies: Uuid,
    upstream_of: Uuid,
    x: Uuid,
    y: Uuid,
}

type DerivedRows = Vec<(
    Uuid,
    Uuid,
    Uuid,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
)>;

type TimedRelationRow = (Uuid, Uuid, Option<Uuid>, DateTime<Utc>, DateTime<Utc>);

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (well, field) = (Uuid::now_v7(), Uuid::now_v7());
    let (pressure, depth) = (Uuid::now_v7(), Uuid::now_v7());
    let (supplies, upstream_of) = (Uuid::now_v7(), Uuid::now_v7());
    let (x, y) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'join-rule-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'join-rule-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'join-rule-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [(well, "well", "Well"), (field, "field", "Field")] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    for (id, key, kind, datatype) in [
        (pressure, "pressure", "attribute", "number"),
        (depth, "depth", "attribute", "number"),
    ] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype)
             VALUES ($1, $2, $3, $3, $4, $5)",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .bind(kind)
        .bind(datatype)
        .execute(pool)
        .await?;
    }
    for (id, key) in [(supplies, "supplies"), (upstream_of, "upstream_of")] {
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label, kind)
             VALUES ($1, $2, $3, $3, 'relation')",
        )
        .bind(id)
        .bind(kb)
        .bind(key)
        .execute(pool)
        .await?;
    }
    for (id, type_id, name) in [(x, well, "W-1"), (y, field, "F-2")] {
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

    Ok(Fixture {
        org,
        kb,
        well,
        pressure,
        depth,
        supplies,
        upstream_of,
        x,
        y,
    })
}

async fn attr(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    predicate: Uuid,
    value: f64,
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
    .bind(subject)
    .bind(predicate)
    .bind(serde_json::json!({ "value": value }))
    .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?)
    .execute(pool)
    .await?;
    Ok(id)
}

async fn edge(
    pool: &PgPool,
    f: &Fixture,
    predicate: Uuid,
    from: &str,
    to: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence)
         VALUES ($1, $2, $3, $4, $5, $6, 'day', $7, 'day', 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.x)
    .bind(predicate)
    .bind(f.y)
    .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?)
    .bind(to.parse::<chrono::DateTime<chrono::Utc>>()?)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 同一条谓词、同一个主语，宾语另指：给 functional 那条公理一个可以撞的对象
async fn edge_to(
    pool: &PgPool,
    f: &Fixture,
    predicate: Uuid,
    object: Uuid,
    from: &str,
    to: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                            valid_from, valid_from_precision,
                            valid_to, valid_to_precision, confidence)
         VALUES ($1, $2, $3, $4, $5, $6, 'day', $7, 'day', 0.9)",
    )
    .bind(id)
    .bind(f.kb)
    .bind(f.x)
    .bind(predicate)
    .bind(object)
    .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?)
    .bind(to.parse::<chrono::DateTime<chrono::Utc>>()?)
    .execute(pool)
    .await?;
    Ok(id)
}

fn conditions(f: &Fixture) -> [ConditionInput; 2] {
    [
        ConditionInput {
            group: 0,
            predicate_id: f.pressure,
            op: "gt".into(),
            operand: Some(serde_json::json!(80.0)),
            side: "x".into(),
        },
        ConditionInput {
            group: 0,
            predicate_id: f.depth,
            op: "lt".into(),
            operand: Some(serde_json::json!(500.0)),
            side: "y".into(),
        },
    ]
}

fn pressure_condition(f: &Fixture, side: &str) -> [ConditionInput; 1] {
    [ConditionInput {
        group: 0,
        predicate_id: f.pressure,
        op: "gt".into(),
        operand: Some(serde_json::json!(80.0)),
        side: side.into(),
    }]
}

/// 只改结论、不带条件的更新会清掉连接而把 Y 侧条件留下（#1109）：整条更新被拒，什么都不写
#[tokio::test]
async fn dropping_the_join_is_refused_while_the_rule_keeps_y_conditions() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let run = async {
        let rule = business_rules::create(
            &pool,
            f.kb,
            "high pressure on the joined entity",
            "",
            f.well,
            "relation",
            None,
            Some(f.upstream_of),
            None,
            None,
            Some(f.supplies),
            &pressure_condition(&f, "y"),
        )
        .await?;
        let attribute = ConclusionInput {
            kind: "attribute".into(),
            type_id: None,
            predicate_id: Some(f.depth),
            value: Some(serde_json::json!(1.0)),
            expr: None,
            join_predicate_id: None,
        };
        let saved = || async {
            sqlx::query_as::<_, (String, Option<Uuid>, String)>(
                "SELECT r.conclusion, r.join_predicate_id, c.subject_side
                   FROM attribute_rules r JOIN attribute_rule_conditions c ON c.rule_id = r.id
                  WHERE r.id = $1",
            )
            .bind(rule)
            .fetch_one(&pool)
            .await
        };

        let refused =
            business_rules::update(&pool, f.kb, rule, None, None, None, None, Some(&attribute))
                .await
                .expect_err("the Y condition the rule keeps still needs its join");
        assert!(
            matches!(
                &refused,
                utopia_core::AppError::Invalid {
                    code: "condition_side_without_join",
                    ..
                }
            ),
            "{refused:?}"
        );
        assert_eq!(
            saved().await?,
            ("relation".into(), Some(f.supplies), "y".into())
        );

        // 同一次更新里把条件换到 X 侧，去掉连接就是合法的
        let on_x = pressure_condition(&f, "x");
        business_rules::update(
            &pool,
            f.kb,
            rule,
            None,
            None,
            None,
            Some(&on_x),
            Some(&attribute),
        )
        .await?;
        assert_eq!(saved().await?, ("attribute".into(), None, "x".into()));

        // 修复之前存下的非法规则仍然能改名、停用：只有动了条件或结论才校验
        sqlx::query("UPDATE attribute_rule_conditions SET subject_side = 'y' WHERE rule_id = $1")
            .bind(rule)
            .execute(&pool)
            .await?;
        business_rules::update(
            &pool,
            f.kb,
            rule,
            Some("disabled"),
            None,
            Some(false),
            None,
            None,
        )
        .await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn a_y_only_join_scopes_x_without_requiring_x_attributes() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let run = async {
        let (horizontal_well, plant_type) = (Uuid::now_v7(), Uuid::now_v7());
        let (subclass_subject, plant_subject) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label)
             VALUES ($1, $2, 'horizontal_well', 'HorizontalWell'), ($3, $2, 'plant', 'Plant')",
        )
        .bind(horizontal_well)
        .bind(f.kb)
        .bind(plant_type)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO entity_type_parents (child_id, parent_id) VALUES ($1, $2)")
            .bind(horizontal_well)
            .bind(f.well)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name)
             VALUES ($1, $2, $3, 'HW-1'), ($4, $2, $5, 'P-1')",
        )
        .bind(subclass_subject)
        .bind(f.kb)
        .bind(horizontal_well)
        .bind(plant_subject)
        .bind(plant_type)
        .execute(&pool)
        .await?;
        // None of the three subjects has attributes; only Y supplies the reading.
        attr(&pool, &f, f.y, f.pressure, 120.0, "2024-01-15T00:00:00Z").await?;
        let join = edge(
            &pool,
            &f,
            f.supplies,
            "2024-01-01T00:00:00Z",
            "2024-02-01T00:00:00Z",
        )
        .await?;
        for subject in [subclass_subject, plant_subject] {
            sqlx::query(
                "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id,
                                    valid_from, valid_from_precision, valid_to, valid_to_precision,
                                    confidence)
                 SELECT $1, kb_id, $2, predicate_id, object_id,
                        valid_from, valid_from_precision, valid_to, valid_to_precision, confidence
                   FROM facts WHERE id = $3",
            )
            .bind(Uuid::now_v7())
            .bind(subject)
            .bind(join)
            .execute(&pool)
            .await?;
        }
        let rule = business_rules::create(
            &pool,
            f.kb,
            "a Well connected to a high-pressure Field",
            "",
            f.well,
            "relation",
            None,
            Some(f.upstream_of),
            None,
            None,
            Some(f.supplies),
            &pressure_condition(&f, "y"),
        )
        .await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        let subjects: Vec<Uuid> = sqlx::query_scalar(
            "SELECT subject_id FROM derived_facts
              WHERE kb_id = $1 AND attribute_rule_id = $2 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .bind(rule)
        .fetch_all(&pool)
        .await?;
        assert_eq!(subjects.len(), 2, "only Well and its subclass may conclude");
        assert!(subjects.contains(&f.x));
        assert!(subjects.contains(&subclass_subject));
        assert!(!subjects.contains(&plant_subject));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn a_y_only_join_keeps_a_concluded_class_in_its_proof() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let run = async {
        let gas_well = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'gas_well', 'GasWell')",
        )
        .bind(gas_well)
        .bind(f.kb)
        .execute(&pool)
        .await?;
        let x_pressure = attr(&pool, &f, f.x, f.pressure, 120.0, "2024-01-01T00:00:00Z").await?;
        sqlx::query(
            "UPDATE facts SET valid_to = '2024-02-01T00:00:00Z', valid_to_precision = 'day' WHERE id = $1",
        )
        .bind(x_pressure)
        .execute(&pool)
        .await?;
        let y_pressure = attr(&pool, &f, f.y, f.pressure, 120.0, "2024-01-15T00:00:00Z").await?;
        let join = edge(
            &pool,
            &f,
            f.supplies,
            "2024-01-01T00:00:00Z",
            "2024-03-01T00:00:00Z",
        )
        .await?;
        // The same pair's later edge falls entirely outside its concluded GasWell interval.
        edge(
            &pool,
            &f,
            f.supplies,
            "2024-04-01T00:00:00Z",
            "2024-05-01T00:00:00Z",
        )
        .await?;
        let typing_rule = business_rules::create(
            &pool,
            f.kb,
            "high pressure makes a GasWell",
            "",
            f.well,
            "typing",
            Some(gas_well),
            None,
            None,
            None,
            None,
            &pressure_condition(&f, "x"),
        )
        .await?;
        let relation_rule = business_rules::create(
            &pool,
            f.kb,
            "a GasWell connected to a high-pressure Field",
            "",
            gas_well,
            "relation",
            None,
            Some(f.upstream_of),
            None,
            None,
            Some(f.supplies),
            &pressure_condition(&f, "y"),
        )
        .await?;
        utopia_store::reasoning::materialize(&pool, f.kb).await?;
        let rows: Vec<TimedRelationRow> = sqlx::query_as(
            "SELECT id, subject_id, object_id, valid_from, valid_to FROM derived_facts
              WHERE kb_id = $1 AND attribute_rule_id = $2 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .bind(relation_rule)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows.len(), 1, "the disjoint join must not conclude a relation");
        let (derived, subject, object, from, to) = rows[0];
        assert_eq!((subject, object), (f.x, Some(f.y)));
        assert_eq!(from.to_rfc3339(), "2024-01-15T00:00:00+00:00");
        assert_eq!(to.to_rfc3339(), "2024-02-01T00:00:00+00:00");
        let concluded_class: Uuid = sqlx::query_scalar(
            "SELECT id FROM derived_facts
              WHERE kb_id = $1 AND attribute_rule_id = $2 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .bind(typing_rule)
        .fetch_one(&pool)
        .await?;
        let premises: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT premise_fact_id, premise_derived_id
               FROM fact_derivations WHERE derived_fact_id = $1 ORDER BY seq",
        )
        .bind(derived)
        .fetch_all(&pool)
        .await?;
        assert_eq!(premises.len(), 3);
        assert!(premises.contains(&(Some(y_pressure), None)));
        assert!(premises.contains(&(Some(join), None)));
        assert!(premises.contains(&(None, Some(concluded_class))));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn a_joined_rule_reads_the_other_side_of_a_declared_edge() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let run = async {
        // An older assertion of the same edge must only win where it holds.
        // The rule reads a later supply, so this earlier interval must not
        // suppress the derived conclusion for February onward.
        let _asserted_upstream = edge(
            &pool,
            &f,
            f.upstream_of,
            "2024-01-01T00:00:00Z",
            "2024-01-15T00:00:00Z",
        )
        .await?;
        let pressure = attr(&pool, &f, f.x, f.pressure, 120.0, "2024-01-01T00:00:00Z").await?;
        let depth = attr(&pool, &f, f.y, f.depth, 300.0, "2024-01-15T00:00:00Z").await?;
        let join = edge(
            &pool,
            &f,
            f.supplies,
            "2024-01-01T00:00:00Z",
            "2024-02-01T00:00:00Z",
        )
        .await?;

        utopia_store::business_rules::create(
            &pool,
            f.kb,
            "upstream high pressure",
            "",
            f.well,
            "relation",
            None,
            Some(f.upstream_of),
            None,
            None,
            Some(f.supplies),
            &conditions(&f),
        )
        .await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.attribute_rules, 1);
        assert_eq!(
            report.rule_hits, 1,
            "the Y reading is outside the rule subject type but inside the declared join"
        );

        let rows: DerivedRows = sqlx::query_as(
            "SELECT id, subject_id, predicate_id, object_id, valid_from
                   FROM derived_facts
                  WHERE kb_id = $1 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows.len(), 1);
        let (derived, subject, predicate, object, from) = rows[0];
        assert_eq!(subject, f.x);
        assert_eq!(predicate, f.upstream_of);
        assert_eq!(object, Some(f.y));
        assert_eq!(
            from.to_rfc3339(),
            "2024-01-15T00:00:00+00:00",
            "validity starts at the latest of the three premises"
        );

        let premises: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
            "SELECT premise_fact_id, premise_derived_id
               FROM fact_derivations
              WHERE derived_fact_id = $1
              ORDER BY seq",
        )
        .bind(derived)
        .fetch_all(&pool)
        .await?;
        let asserted: Vec<Uuid> = premises.iter().filter_map(|(f, _)| *f).collect();
        assert_eq!(
            asserted,
            vec![pressure, depth, join],
            "both sides and the edge are the complete proof"
        );
        assert!(premises.iter().all(|(_, d)| d.is_none()));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}

/// 一条规则推出的关系边撞上断言时不落地，而审核队列要看得见它（0017，0047 决定 3）：
/// `run()` 与 `materialize()` 从同一次求解取候选，被拦下的关系候选留在候选里，
/// 队列那一行说明它是哪条业务规则推出来的、撞在哪条断言上
#[tokio::test]
async fn a_relation_the_graph_refuses_still_reaches_the_review_queue() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let f = seed(&pool).await?;

    let run = async {
        // upstream_of 是 functional：X 已经断言了另一个上游 Z，规则推出的 X → Y
        // 与它区间相交，asserted > derived，这条派生不落地
        sqlx::query("UPDATE relation_types SET functional = true WHERE id = $1")
            .bind(f.upstream_of)
            .execute(&pool)
            .await?;
        let z = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name)
             VALUES ($1, $2, (SELECT type_id FROM entities WHERE id = $3), 'F-3')",
        )
        .bind(z)
        .bind(f.kb)
        .bind(f.y)
        .execute(&pool)
        .await?;
        let other_upstream = edge_to(
            &pool,
            &f,
            f.upstream_of,
            z,
            "2024-01-01T00:00:00Z",
            "2024-12-31T00:00:00Z",
        )
        .await?;
        attr(&pool, &f, f.x, f.pressure, 120.0, "2024-01-01T00:00:00Z").await?;
        attr(&pool, &f, f.y, f.depth, 300.0, "2024-01-15T00:00:00Z").await?;
        let join = edge(
            &pool,
            &f,
            f.supplies,
            "2024-01-01T00:00:00Z",
            "2024-02-01T00:00:00Z",
        )
        .await?;
        let rule = utopia_store::business_rules::create(
            &pool,
            f.kb,
            "upstream high pressure",
            "",
            f.well,
            "relation",
            None,
            Some(f.upstream_of),
            None,
            None,
            Some(f.supplies),
            &conditions(&f),
        )
        .await?;

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(
            report.blocked, 1,
            "the relation lost to the asserted upstream"
        );
        assert_eq!(report.inserted, 0, "{report:?}");
        assert_eq!(
            report.rule_hits, 0,
            "a refused conclusion is not a conclusion that stands"
        );
        let (derived_rows,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM derived_facts WHERE kb_id = $1 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(derived_rows, 0, "nothing lands");

        let check = utopia_store::reasoning::run(&pool, f.kb).await?;
        assert_eq!(check.contradictions, 1, "{check:?}");
        let rows: Vec<(Uuid, Uuid, serde_json::Value)> = sqlx::query_as(
            "SELECT left_fact, right_fact, detail FROM axiom_violations
              WHERE kb_id = $1 AND kind = 'derived_contradiction' AND status = 'open'",
        )
        .bind(f.kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(rows.len(), 1, "{rows:?}");
        let (left, right, detail) = &rows[0];
        assert_eq!(*left, other_upstream, "against the asserted upstream");
        assert_eq!(
            *right, join,
            "keyed by the last asserted premise: the join edge"
        );
        assert_eq!(detail["rule"], "business_rule");
        assert_eq!(detail["axiom"], "functional");
        assert_eq!(detail["attribute_rule_id"], serde_json::json!(rule));
        assert_eq!(detail["object_id"], serde_json::json!(f.y));

        // 同一次求解，第二遍不多不少：队列跟图对得上
        let again = utopia_store::reasoning::run(&pool, f.kb).await?;
        assert_eq!(again.inserted, 0, "{again:?}");
        assert_eq!(again.cleared, 0, "{again:?}");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
