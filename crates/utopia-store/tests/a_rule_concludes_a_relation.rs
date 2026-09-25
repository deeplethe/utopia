//! A joined rule has to see both ends of the edge in the database (0047).
//!
//! The pure evaluator already knows `Side::Y`. This file pins the loader
//! contract behind it: `X` remains scoped by the rule subject type, while `Y`
//! may be any entity the declared join reaches. It also checks that the
//! persisted row carries its object and the full three-part proof.

use sqlx::PgPool;
use utopia_store::business_rules::ConditionInput;
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
