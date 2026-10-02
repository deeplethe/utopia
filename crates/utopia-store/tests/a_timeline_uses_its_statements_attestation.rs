//! Chapter attestation orders functional states; a document date cannot replace it (0064).
//! These tests use production materialization and world-axis predicates, without an LLM.

use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::PgPool;
use utopia_store::{graph, materialize, phrase_bindings, temporal, world_axis};
use uuid::Uuid;

struct Fixture {
    org: Uuid,
    kb: Uuid,
    subject: Uuid,
    property: Uuid,
    document: Uuid,
}

fn day(date: &str) -> DateTime<Utc> {
    format!("{date}T00:00:00Z").parse().unwrap()
}

async fn pool() -> anyhow::Result<Option<PgPool>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    Ok(Some(PgPool::connect(&url).await?))
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, workspace, kb, subject, property, document) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','chapter-timeline-test');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{workspace}','{org}','chapter-timeline-test');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{workspace}','chapter-timeline-test');
         INSERT INTO entities(id,kb_id,canonical_name) VALUES ('{subject}','{kb}','Watch');
         INSERT INTO relation_types(id,kb_id,key,label,kind,datatype,temporal,functional)
             VALUES ('{property}','{kb}','market','market','attribute','text','state',TRUE);
         INSERT INTO documents(id,kb_id,filename,sha256,doc_time,doc_time_source)
             VALUES ('{document}','{kb}','chapters.txt','{document}','2026-09-04','content');
         INSERT INTO document_versions(id,document_id,version,sha256,doc_time)
             VALUES ('{}','{document}',1,'{document}','2026-09-04');",
        Uuid::now_v7()
    ))
    .execute(pool)
    .await?;
    Ok(Fixture {
        org,
        kb,
        subject,
        property,
        document,
    })
}

async fn statement(
    pool: &PgPool,
    fixture: &Fixture,
    value: &str,
    attested: Option<&str>,
    version: i32,
) -> anyhow::Result<Uuid> {
    let object = json!({ "value": value });
    let (statement, _) = graph::insert_open_statement(
        pool,
        fixture.kb,
        fixture.subject,
        "market is",
        graph::FactObject::Value(&object),
        None,
        0.9,
    )
    .await?;
    let chunk = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO chunks(id,kb_id,document_id,doc_version,seq,text)
         VALUES ($1,$2,$3,$4,$4,$5)",
    )
    .bind(chunk)
    .bind(fixture.kb)
    .bind(fixture.document)
    .bind(version)
    .bind(value)
    .execute(pool)
    .await?;
    graph::add_evidence(pool, statement, chunk, Some(value), Some("market is")).await?;
    if let Some(date) = attested {
        graph::attest_statement(pool, statement, day(date), &format!("chapter {date}")).await?;
    }
    Ok(statement)
}

async fn bind(pool: &PgPool, fixture: &Fixture) -> anyhow::Result<()> {
    for signature in phrase_bindings::signatures(pool, fixture.kb).await? {
        phrase_bindings::decide(
            pool,
            fixture.kb,
            &signature,
            phrase_bindings::Decision {
                relation_type_id: Some(fixture.property),
                direction: Some("forward"),
                status: "bound",
                votes: &json!({}),
                decided_by: "agent",
                basis: None,
                marks: None,
                marks_asked: false,
            },
        )
        .await?;
    }
    Ok(())
}

#[derive(Debug, sqlx::FromRow)]
struct State {
    id: Uuid,
    valid_from: Option<DateTime<Utc>>,
    valid_to: Option<DateTime<Utc>>,
    valid_to_precision: Option<String>,
    attested_from: Option<DateTime<Utc>>,
    attested_by: Option<String>,
    attested_to: Option<DateTime<Utc>>,
    valid_from_grade: Option<String>,
    end_derived: bool,
    supersedes: Option<Uuid>,
}

async fn state(pool: &PgPool, fixture: &Fixture, value: &str) -> anyhow::Result<State> {
    Ok(sqlx::query_as(
        "SELECT id,valid_from,valid_to,valid_to_precision,attested_from,attested_by,attested_to,valid_from_grade,end_derived,supersedes
           FROM facts WHERE kb_id=$1 AND predicate_id=$2 AND layer='typed'
            AND invalidated_at IS NULL AND object_value #>> '{value}'=$3",
    )
    .bind(fixture.kb)
    .bind(fixture.property)
    .bind(value)
    .fetch_one(pool)
    .await?)
}

async fn values_at(pool: &PgPool, fixture: &Fixture, date: &str) -> anyhow::Result<Vec<String>> {
    let held = world_axis::facts_hold_at("f", 3);
    Ok(sqlx::query_scalar(&format!(
        "SELECT object_value #>> '{{value}}' FROM facts f
          WHERE kb_id=$1 AND predicate_id=$2 AND layer='typed' AND invalidated_at IS NULL
            AND {held} ORDER BY object_value #>> '{{value}}'"
    ))
    .bind(fixture.kb)
    .bind(fixture.property)
    .bind(day(date))
    .fetch_all(pool)
    .await?)
}

async fn cleanup(pool: &PgPool, fixture: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(fixture.org)
        .execute(pool)
        .await?;
    Ok(())
}

async fn assert_chapter_timeline(pool: &PgPool, fixture: &Fixture) -> anyhow::Result<()> {
    let old = state(pool, fixture, "old").await?;
    let new = state(pool, fixture, "new").await?;
    assert_eq!(old.valid_from, None, "as-of is not a start");
    assert_eq!(new.valid_from, None, "as-of is not a start");
    assert_eq!(old.attested_from, Some(day("2015-01-01")));
    assert_eq!(old.attested_by.as_deref(), Some("chapter 2015-01-01"));
    assert_eq!(new.attested_from, Some(day("2026-01-01")));
    assert_eq!(old.valid_to, None, "the text did not state an end");
    assert_eq!(old.valid_to_precision.as_deref(), Some("unknown"));
    assert_eq!(old.attested_to, Some(day("2026-01-01")));
    assert!(old.end_derived);
    assert!(old.supersedes.is_some(), "closure must preserve the ledger");
    let historical: bool =
        sqlx::query_scalar("SELECT invalidated_at IS NOT NULL FROM facts WHERE id=$1")
            .bind(old.supersedes.unwrap())
            .fetch_one(pool)
            .await?;
    assert!(historical);
    assert!(!new.end_derived);
    assert_eq!(new.valid_to_precision, None);
    assert_eq!(
        values_at(pool, fixture, "2014-01-01").await?,
        Vec::<String>::new()
    );
    assert_eq!(values_at(pool, fixture, "2020-01-01").await?, ["old"]);
    assert_eq!(values_at(pool, fixture, "2026-01-01").await?, ["new"]);
    Ok(())
}

#[tokio::test]
async fn chapter_dates_before_materialization_close_the_previous_value() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    statement(&pool, &fixture, "old", Some("2015-01-01"), 1).await?;
    statement(&pool, &fixture, "new", Some("2026-01-01"), 1).await?;
    bind(&pool, &fixture).await?;
    let result = materialize::materialize(&pool, fixture.kb).await?;
    assert_eq!(result.corrected, 1, "{result:?}");
    assert_eq!(result.conflicts, 0, "different chapter dates are ordered");
    assert_chapter_timeline(&pool, &fixture).await?;
    assert_eq!(
        materialize::materialize(&pool, fixture.kb).await?,
        materialize::Outcome::default()
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn chapter_dates_synced_after_materialization_reconcile_the_existing_timeline(
) -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    let old = statement(&pool, &fixture, "old", None, 1).await?;
    let new = statement(&pool, &fixture, "new", None, 1).await?;
    bind(&pool, &fixture).await?;
    materialize::materialize(&pool, fixture.kb).await?;
    graph::attest_statement(&pool, old, day("2015-01-01"), "chapter 2015-01-01").await?;
    graph::attest_statement(&pool, new, day("2026-01-01"), "chapter 2026-01-01").await?;
    let result = materialize::materialize(&pool, fixture.kb).await?;
    assert_eq!(
        result.added, 0,
        "date synchronization must reconcile existing rows"
    );
    assert_eq!(result.corrected, 1, "{result:?}");
    assert_chapter_timeline(&pool, &fixture).await?;
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn equal_chapter_dates_remain_a_real_conflict() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    for value in ["old", "new"] {
        statement(&pool, &fixture, value, Some("2026-01-01"), 1).await?;
    }
    bind(&pool, &fixture).await?;
    let result = materialize::materialize(&pool, fixture.kb).await?;
    assert_eq!(result.corrected, 0);
    assert_eq!(result.conflicts, 1);
    assert_eq!(
        values_at(&pool, &fixture, "2026-01-01").await?,
        ["new", "old"]
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn an_undated_statement_does_not_borrow_a_document_or_version_date() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    statement(&pool, &fixture, "old", Some("2015-01-01"), 1).await?;
    statement(&pool, &fixture, "new", None, 1).await?;
    bind(&pool, &fixture).await?;
    let result = materialize::materialize(&pool, fixture.kb).await?;
    assert_eq!(result.corrected, 0, "NULL explicitly means no attestation");
    assert_eq!(state(&pool, &fixture, "new").await?.attested_from, None);
    assert_eq!(
        state(&pool, &fixture, "old").await?.valid_to_precision,
        None
    );
    assert_eq!(values_at(&pool, &fixture, "2014-01-01").await?, ["new"]);
    assert_eq!(
        values_at(&pool, &fixture, "2026-01-01").await?,
        ["new", "old"]
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn explicit_starts_take_priority_and_unresolved_starts_close_nothing() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    for grade in ["A", "C"] {
        let fixture = seed(&pool).await?;
        statement(&pool, &fixture, "old", Some("2015-01-01"), 1).await?;
        let attested = if grade == "A" {
            "2010-01-01"
        } else {
            "2026-01-01"
        };
        let new = statement(&pool, &fixture, "new", Some(attested), 1).await?;
        graph::set_open_validity(
            &pool,
            new,
            (grade == "A").then(|| day("2020-01-01")),
            (grade == "A").then_some("day"),
            None,
            None,
            Some(grade),
        )
        .await?;
        bind(&pool, &fixture).await?;
        let result = materialize::materialize(&pool, fixture.kb).await?;
        let old = state(&pool, &fixture, "old").await?;
        if grade == "A" {
            assert_eq!(result.corrected, 1);
            assert_eq!(old.valid_to, Some(day("2020-01-01")));
            assert_eq!(old.valid_to_precision.as_deref(), Some("day"));
            assert_eq!(old.attested_to, None);
            assert_eq!(values_at(&pool, &fixture, "2016-01-01").await?, ["old"]);
            assert_eq!(values_at(&pool, &fixture, "2020-01-01").await?, ["new"]);
        } else {
            assert_eq!(result.corrected, 0);
            assert_eq!(result.conflicts, 0, "grade C waits for resolution");
            assert_eq!(old.valid_to_precision, None);
        }
        cleanup(&pool, &fixture).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_new_document_version_cannot_redate_earlier_chapter_evidence() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    statement(&pool, &fixture, "old", Some("2015-01-01"), 1).await?;
    sqlx::query("UPDATE documents SET doc_time='2030-01-01' WHERE id=$1")
        .bind(fixture.document)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO document_versions(id,document_id,version,sha256,doc_time) VALUES ($1,$2,2,$3,'2030-01-01')")
        .bind(Uuid::now_v7()).bind(fixture.document).bind(Uuid::now_v7().to_string()).execute(&pool).await?;
    statement(&pool, &fixture, "new", Some("2026-01-01"), 2).await?;
    bind(&pool, &fixture).await?;
    materialize::materialize(&pool, fixture.kb).await?;
    assert_chapter_timeline(&pool, &fixture).await?;
    let old = state(&pool, &fixture, "old").await?;
    let versions: Vec<i32> =
        sqlx::query_scalar("SELECT doc_version FROM fact_evidence WHERE fact_id=$1")
            .bind(old.id)
            .fetch_all(&pool)
            .await?;
    assert_eq!(
        versions,
        [1],
        "closure preserves the old evidence's version"
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn closing_an_unresolved_predecessor_preserves_its_grade_and_attestation_name(
) -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    let old = statement(&pool, &fixture, "old", Some("2015-01-01"), 1).await?;
    graph::set_open_validity(&pool, old, None, None, None, None, Some("C")).await?;
    statement(&pool, &fixture, "new", Some("2026-01-01"), 1).await?;
    bind(&pool, &fixture).await?;
    materialize::materialize(&pool, fixture.kb).await?;
    assert_chapter_timeline(&pool, &fixture).await?;
    assert_eq!(
        state(&pool, &fixture, "old")
            .await?
            .valid_from_grade
            .as_deref(),
        Some("C")
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn controlled_reconciliation_reopens_an_undated_derived_end_without_overwriting_history(
) -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    statement(&pool, &fixture, "old", None, 1).await?;
    bind(&pool, &fixture).await?;
    materialize::materialize(&pool, fixture.kb).await?;
    let original = state(&pool, &fixture, "old").await?;
    // Simulate a legacy engine boundary based only on document time. The repair
    // must replace this live projection through the ledger, not erase its past.
    sqlx::query("UPDATE facts SET valid_to_precision='unknown',attested_to='2026-09-04',end_derived=TRUE WHERE id=$1")
        .bind(original.id).execute(&pool).await?;
    let repaired = temporal::reconcile_predicate(&pool, fixture.kb, fixture.property).await?;
    assert_eq!(repaired.corrected.len(), 1);
    let reopened = state(&pool, &fixture, "old").await?;
    assert_eq!(reopened.supersedes, Some(original.id));
    assert_eq!(reopened.attested_from, None);
    assert_eq!(reopened.valid_to_precision, None);
    assert_eq!(reopened.attested_to, None);
    assert!(!reopened.end_derived);
    assert_eq!(values_at(&pool, &fixture, "2030-01-01").await?, ["old"]);
    let historical: (bool, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
        "SELECT invalidated_at IS NOT NULL,valid_to_precision,attested_to FROM facts WHERE id=$1",
    )
    .bind(original.id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        historical,
        (true, Some("unknown".to_string()), Some(day("2026-09-04")))
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn merge_conflict_checks_use_the_same_chapter_dates_as_timeline_reconciliation(
) -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    sqlx::query("UPDATE relation_types SET kind='relation',datatype=NULL WHERE id=$1")
        .bind(fixture.property)
        .execute(&pool)
        .await?;
    let (other_subject, old_market, new_market) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    for (entity, name) in [
        (other_subject, "Other Watch"),
        (old_market, "Old Market"),
        (new_market, "New Market"),
    ] {
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,$3)")
            .bind(entity)
            .bind(fixture.kb)
            .bind(name)
            .execute(&pool)
            .await?;
    }
    let mut later_statement = None;
    for (subject, object, date) in [
        (fixture.subject, old_market, "2015-01-01"),
        (other_subject, new_market, "2026-01-01"),
    ] {
        let (statement, _) = graph::insert_open_statement(
            &pool,
            fixture.kb,
            subject,
            "market is",
            graph::FactObject::Entity(object),
            None,
            0.9,
        )
        .await?;
        graph::attest_statement(&pool, statement, day(date), &format!("chapter {date}")).await?;
        let chunk = Uuid::now_v7();
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,doc_version,seq,text) VALUES ($1,$2,$3,1,0,$4)")
            .bind(chunk).bind(fixture.kb).bind(fixture.document).bind(date).execute(&pool).await?;
        graph::add_evidence(&pool, statement, chunk, Some(date), Some("market is")).await?;
        later_statement = Some(statement);
    }
    bind(&pool, &fixture).await?;
    materialize::materialize(&pool, fixture.kb).await?;
    assert!(
        temporal::merge_would_overlap(&pool, fixture.kb, fixture.subject, other_subject)
            .await?
            .is_empty(),
        "different chapter dates can form one timeline after a merge"
    );
    graph::attest_statement(
        &pool,
        later_statement.unwrap(),
        day("2015-01-01"),
        "chapter 2015-01-01",
    )
    .await?;
    materialize::materialize(&pool, fixture.kb).await?;
    assert_eq!(
        temporal::merge_would_overlap(&pool, fixture.kb, fixture.subject, other_subject).await?,
        ["market"],
        "equal chapter dates remain a merge conflict"
    );
    cleanup(&pool, &fixture).await
}

#[tokio::test]
async fn chapter_attestation_reconciliation_preserves_a_stated_end() -> anyhow::Result<()> {
    let Some(pool) = pool().await? else {
        return Ok(());
    };
    let fixture = seed(&pool).await?;
    let old = statement(&pool, &fixture, "old", Some("2015-01-01"), 1).await?;
    graph::set_open_validity(
        &pool,
        old,
        None,
        None,
        Some(day("2018-01-01")),
        Some("day"),
        Some("A"),
    )
    .await?;
    statement(&pool, &fixture, "new", Some("2026-01-01"), 1).await?;
    bind(&pool, &fixture).await?;
    materialize::materialize(&pool, fixture.kb).await?;
    temporal::reconcile_predicate(&pool, fixture.kb, fixture.property).await?;
    let old = state(&pool, &fixture, "old").await?;
    assert_eq!(old.valid_to, Some(day("2018-01-01")));
    assert_eq!(old.valid_to_precision.as_deref(), Some("day"));
    assert!(
        !old.end_derived,
        "only engine-derived ends may be recomputed"
    );
    assert_eq!(
        values_at(&pool, &fixture, "2020-01-01").await?,
        Vec::<String>::new()
    );
    assert_eq!(values_at(&pool, &fixture, "2026-01-01").await?, ["new"]);
    cleanup(&pool, &fixture).await
}
