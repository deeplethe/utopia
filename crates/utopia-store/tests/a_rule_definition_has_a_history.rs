//! A rule's definition has a history (0060, #912).
//!
//! Editing what a rule says opens a version and closes the previous one; renaming it
//! does not. A derivation names the version it was drawn under, a kept conclusion
//! moves to the new version, and the history endpoint reads all of it back with labels.

use sqlx::PgPool;
use utopia_store::business_rules::{self, ConclusionInput, ConditionInput};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    well: Uuid,
    gas_well: Uuid,
    depth: Uuid,
    w1: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (well, gas_well, depth, w1) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'rule-history-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'rule-history-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'rule-history-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (well, "well", "Well"),
        (gas_well, "gas_well", "Gas-bearing well"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype)
         VALUES ($1, $2, 'depth', 'Depth', 'attribute', 'number')",
    )
    .bind(depth)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, 'W-1')",
    )
    .bind(w1)
    .bind(kb)
    .bind(well)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                            valid_from, valid_from_precision, confidence)
         VALUES ($1, $2, $3, $4, $5, '2024-01-01T00:00:00Z', 'day', 0.9)",
    )
    .bind(Uuid::now_v7())
    .bind(kb)
    .bind(w1)
    .bind(depth)
    .bind(serde_json::json!({ "value": 3200.0 }))
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        well,
        gas_well,
        depth,
        w1,
    })
}

fn deeper_than(f: &Fixture, threshold: f64) -> [ConditionInput; 1] {
    [ConditionInput {
        group: 0,
        predicate_id: f.depth,
        op: "gt".into(),
        operand: Some(serde_json::json!(threshold)),
        side: "x".into(),
    }]
}

async fn versions_of(pool: &PgPool, rule: Uuid) -> anyhow::Result<Vec<(i32, bool)>> {
    Ok(sqlx::query_as(
        "SELECT seq, superseded_at IS NULL FROM attribute_rule_versions
          WHERE rule_id = $1 ORDER BY seq",
    )
    .bind(rule)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn editing_what_a_rule_says_opens_a_version_and_a_conclusion_names_it() -> anyhow::Result<()>
{
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
            "deep well",
            "",
            f.well,
            "typing",
            Some(f.gas_well),
            None,
            None,
            None,
            None,
            &deeper_than(&f, 3000.0),
        )
        .await?;
        assert_eq!(
            versions_of(&pool, rule).await?,
            vec![(1, true)],
            "creating is version 1"
        );

        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.inserted, 1, "{report:?}");
        let v1: Uuid = sqlx::query_scalar(
            "SELECT id FROM attribute_rule_versions WHERE rule_id = $1 AND seq = 1",
        )
        .bind(rule)
        .fetch_one(&pool)
        .await?;
        let (derived, under): (Uuid, Option<Uuid>) = sqlx::query_as(
            "SELECT id, attribute_rule_version_id FROM derived_facts
              WHERE kb_id = $1 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            under,
            Some(v1),
            "the conclusion names the version it was drawn under"
        );

        // 改名、改描述、开关：定义没变，不开版本
        business_rules::update(
            &pool,
            f.kb,
            rule,
            Some("deep well (renamed)"),
            Some("a note"),
            Some(true),
            None,
            None,
        )
        .await?;
        assert_eq!(
            versions_of(&pool, rule).await?,
            vec![(1, true)],
            "a label is not the definition"
        );

        // 阈值 3000 → 2500：定义变了，版本 2 开、版本 1 关。W-1（3200）仍然满足，
        // 结论没变，行留着，改指版本 2
        business_rules::update(
            &pool,
            f.kb,
            rule,
            None,
            None,
            None,
            Some(&deeper_than(&f, 2500.0)),
            None,
        )
        .await?;
        assert_eq!(versions_of(&pool, rule).await?, vec![(1, false), (2, true)]);
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!(report.inserted, 0, "{report:?}");
        assert_eq!(report.invalidated, 0, "{report:?}");
        assert_eq!(
            report.redefined, 1,
            "the kept row moved to the new version: {report:?}"
        );
        let v2: Uuid = sqlx::query_scalar(
            "SELECT id FROM attribute_rule_versions WHERE rule_id = $1 AND seq = 2",
        )
        .bind(rule)
        .fetch_one(&pool)
        .await?;
        let (same_row, under): (Uuid, Option<Uuid>) = sqlx::query_as(
            "SELECT id, attribute_rule_version_id FROM derived_facts
              WHERE kb_id = $1 AND invalidated_at IS NULL",
        )
        .bind(f.kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(same_row, derived, "the row is kept, not replaced");
        assert_eq!(under, Some(v2));

        // 证明说得出凭哪一版、那一版怎么说
        let proof = utopia_store::reasoning::proof(&pool, f.kb, derived)
            .await?
            .expect("the conclusion stands");
        assert_eq!(proof.derived.rule_version, Some(2));
        let definition = proof
            .derived
            .rule_definition
            .expect("the version's definition rides along");
        assert_eq!(
            definition["conditions"][0]["operand"],
            serde_json::json!(2500.0)
        );

        // 换个结论类：版本 3；结论换了，旧行作废、新行凭版本 3
        business_rules::update(
            &pool,
            f.kb,
            rule,
            None,
            None,
            None,
            None,
            Some(&ConclusionInput {
                kind: "typing".into(),
                type_id: Some(f.well),
                predicate_id: None,
                value: None,
                expr: None,
                join_predicate_id: None,
            }),
        )
        .await?;
        assert_eq!(
            versions_of(&pool, rule).await?,
            vec![(1, false), (2, false), (3, true)]
        );
        let report = utopia_store::reasoning::materialize(&pool, f.kb).await?;
        assert_eq!((report.invalidated, report.inserted), (1, 1), "{report:?}");

        // 历史：新的在前，带每一版此刻成立的条数和定义里提到的名字
        let history = business_rules::versions(&pool, f.kb, rule).await?;
        assert_eq!(history.len(), 3);
        assert_eq!(history[0]["seq"], 3);
        assert_eq!(history[0]["derived_count"], 1);
        assert_eq!(history[1]["seq"], 2);
        assert_eq!(
            history[1]["derived_count"], 0,
            "the row that stood under v2 was withdrawn"
        );
        assert!(history[1]["superseded_at"].is_string());
        assert!(history[0]["superseded_at"].is_null());
        assert_eq!(
            history[2]["definition"]["conditions"][0]["operand"],
            serde_json::json!(3000.0)
        );
        assert_eq!(history[0]["labels"][f.depth.to_string()], "Depth");
        assert_eq!(
            history[2]["labels"][f.gas_well.to_string()],
            "Gas-bearing well"
        );

        // 不在这个库的规则：404，而不是空历史
        assert!(business_rules::versions(&pool, f.kb, Uuid::now_v7())
            .await
            .is_err());
        let _ = f.w1;
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
