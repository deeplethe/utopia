//! A timeline of a single-valued state stays whole however its values arrive.
//!
//! These are the #679 review reproductions, kept as regression tests: newest-first arrival,
//! re-extraction of the same values, an undated upload, a dated value with no start, a merge
//! followed by an arrival and a revert, a merge that moves two values and is reverted, and two
//! late values reconciled at the same time.
//!
//! Skipped (not failed) without `UTOPIA_DATABASE_URL`. Each test builds and drops its own base.

use serde_json::json;
use sqlx::PgPool;
use utopia_store::graph::Validity;
use utopia_store::temporal::Uniqueness;
use uuid::Uuid;

struct Fixture {
    kb: Uuid,
    etype: Uuid,
    lease: Uuid,
    deadline: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fixture> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (etype, deadline, lease) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'rv')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'rv')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'rv')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, 'lease', 'Lease')",
    )
    .bind(etype)
    .bind(kb)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, kind, datatype, temporal, functional)
         VALUES ($1, $2, 'option_deadline', 'option deadline', 'attribute', 'date', 'state', TRUE)",
    )
    .bind(deadline)
    .bind(kb)
    .execute(pool)
    .await?;
    let lease = entity(pool, kb, etype, "HQ Lease", lease).await?;
    Ok(Fixture {
        kb,
        etype,
        lease,
        deadline,
    })
}

async fn entity(
    pool: &PgPool,
    kb: Uuid,
    etype: Uuid,
    name: &str,
    id: Uuid,
) -> anyhow::Result<Uuid> {
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(kb)
    .bind(etype)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(id)
}

fn t(day: &str) -> chrono::DateTime<chrono::Utc> {
    format!("{day}T00:00:00Z").parse().unwrap()
}

/// Mimics extraction: insert, then reconcile only when created.
#[allow(clippy::too_many_arguments)]
async fn arrive_on(
    pool: &PgPool,
    f: &Fixture,
    subject: Uuid,
    value: &str,
    from: Option<&str>,
    doc: Option<(chrono::DateTime<chrono::Utc>, &str)>,
    confidence: f32,
    reconcile: bool,
) -> anyhow::Result<Uuid> {
    let mut validity = match from {
        Some(from) => Validity::starting(Some(t(from)), Some("day")),
        None => Validity::default(),
    };
    let mut chunk = None;
    if let Some((doc_time, source)) = doc {
        let (d, c) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO documents (id, kb_id, filename, sha256, doc_time, doc_time_source)
             VALUES ($1, $2, $3, $3, $4, $5)",
        )
        .bind(d)
        .bind(f.kb)
        .bind(format!("doc-{d}.html"))
        .bind(doc_time)
        .bind(source)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, $4)",
        )
        .bind(c)
        .bind(f.kb)
        .bind(d)
        .bind(value)
        .execute(pool)
        .await?;
        validity = validity.attested(Some(doc_time));
        chunk = Some(c);
    }
    let object = json!({ "value": value });
    let (id, created) = utopia_store::graph::insert_value_fact(
        pool,
        f.kb,
        subject,
        Some(f.deadline),
        &object,
        validity,
        confidence,
    )
    .await?;
    if let Some(c) = chunk {
        utopia_store::graph::add_evidence(pool, id, c, Some(value), None).await?;
    }
    if created && reconcile {
        utopia_store::temporal::reconcile_new_fact(
            pool,
            f.kb,
            id,
            subject,
            f.deadline,
            None,
            Some(&object),
            Uniqueness::SubjectSide,
            validity,
            confidence,
        )
        .await?;
    }
    Ok(id)
}

async fn arrive(pool: &PgPool, f: &Fixture, value: &str, from: &str) -> anyhow::Result<Uuid> {
    arrive_on(pool, f, f.lease, value, Some(from), None, 0.9, true).await
}

type Seg = (String, Option<String>, Option<String>);

async fn timeline_of(pool: &PgPool, f: &Fixture, subject: Uuid) -> anyhow::Result<Vec<Seg>> {
    Ok(sqlx::query_as(
        "SELECT object_value #>> '{value}', to_char(valid_from, 'YYYY-MM-DD'),
                to_char(valid_to, 'YYYY-MM-DD')
         FROM facts
         WHERE kb_id = $1 AND subject_id = $2 AND predicate_id = $3 AND invalidated_at IS NULL
         ORDER BY valid_from NULLS FIRST, valid_to NULLS LAST",
    )
    .bind(f.kb)
    .bind(subject)
    .bind(f.deadline)
    .fetch_all(pool)
    .await?)
}

/// Pairs of live dated rows whose intervals overlap.
async fn overlaps(pool: &PgPool, f: &Fixture, subject: Uuid) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM facts a JOIN facts b
           ON a.kb_id = b.kb_id AND a.subject_id = b.subject_id AND a.predicate_id = b.predicate_id
          AND a.id < b.id AND a.object_value IS DISTINCT FROM b.object_value
         WHERE a.kb_id = $1 AND a.subject_id = $2 AND a.predicate_id = $3
           AND a.invalidated_at IS NULL AND b.invalidated_at IS NULL
           AND a.valid_from IS NOT NULL AND b.valid_from IS NOT NULL
           AND a.valid_from < COALESCE(b.valid_to, 'infinity')
           AND b.valid_from < COALESCE(a.valid_to, 'infinity')",
    )
    .bind(f.kb)
    .bind(subject)
    .bind(f.deadline)
    .fetch_one(pool)
    .await?)
}

async fn cleanup(pool: &PgPool, f: &Fixture) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(pool)
        .await?;
    Ok(())
}

fn s(v: &str, from: Option<&str>, to: Option<&str>) -> Seg {
    (v.into(), from.map(Into::into), to.map(Into::into))
}

#[tokio::test]
async fn newest_first_arrival_leaves_no_overlap() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    // 9th, 7th, 5th, 6th: newest first, then the rest
    arrive(&pool, &f, "2020-06-23", "2020-06-08").await?;
    arrive(&pool, &f, "2020-05-26", "2020-04-14").await?;
    arrive(&pool, &f, "2020-03-17", "2020-02-18").await?;
    arrive(&pool, &f, "2020-04-14", "2020-03-17").await?;
    let rows = timeline_of(&pool, &f, f.lease).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(n, 0, "{rows:?}");
    Ok(())
}

#[tokio::test]
async fn extracting_the_same_values_again_changes_nothing() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let chain = [
        ("2020-03-17", "2020-02-18"),
        ("2020-06-23", "2020-06-08"),
        ("2020-05-26", "2020-04-14"),
        ("2020-04-14", "2020-03-17"),
    ];
    for (v, from) in chain {
        arrive(&pool, &f, v, from).await?;
    }
    let before = timeline_of(&pool, &f, f.lease).await?;
    for (v, from) in chain.iter().rev() {
        arrive(&pool, &f, v, from).await?;
    }
    let after = timeline_of(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(before, after);
    Ok(())
}

/// Undated upload: documents.create stores doc_time = now(), doc_time_source = 'upload_time'.
#[tokio::test]
async fn an_undated_upload_is_not_a_document_date() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let now = chrono::Utc::now();
    let bare = arrive_on(
        &pool,
        &f,
        f.lease,
        "BBHQ1",
        None,
        Some((now, "upload_time")),
        0.9,
        true,
    )
    .await?;
    arrive_on(
        &pool,
        &f,
        f.lease,
        "HPBB1",
        Some("2021-01-01"),
        Some((t("2021-01-01"), "content")),
        0.9,
        true,
    )
    .await?;
    let (open,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM facts WHERE id = $1 AND invalidated_at IS NULL AND valid_to IS NULL)",
    )
    .bind(bare)
    .fetch_one(&pool)
    .await?;
    let (conflicts,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM fact_conflicts WHERE old_fact_id = $1")
            .bind(bare)
            .fetch_one(&pool)
            .await?;
    let rows = timeline_of(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert!(
        !open,
        "undated upload's start-less row was not closed; conflicts={conflicts}, rows={rows:?}"
    );
    Ok(())
}

/// Dated start-less X (2020-08-13) left open beside Y (2016) after a no_time conflict;
/// Z (2021) arrives. X held on 2020-08-13 < 2021, so Z is its successor.
#[tokio::test]
async fn a_dated_value_with_no_start_ends_at_the_next_start_after_its_date() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let x = arrive_on(
        &pool,
        &f,
        f.lease,
        "BBHQ1",
        None,
        Some((t("2020-08-13"), "content")),
        0.9,
        true,
    )
    .await?;
    arrive_on(
        &pool,
        &f,
        f.lease,
        "HPBB1",
        Some("2016-05-16"),
        Some((t("2016-05-16"), "content")),
        0.9,
        true,
    )
    .await?;
    arrive_on(
        &pool,
        &f,
        f.lease,
        "NEWCO",
        Some("2021-01-01"),
        Some((t("2021-01-01"), "content")),
        0.9,
        true,
    )
    .await?;
    let rows = timeline_of(&pool, &f, f.lease).await?;
    let (open,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM facts WHERE id = $1 AND invalidated_at IS NULL AND valid_to IS NULL)",
    )
    .bind(x)
    .fetch_one(&pool)
    .await?;
    cleanup(&pool, &f).await?;
    assert!(!open, "{rows:?}");
    Ok(())
}

/// Merge moves a value that slots into the target's history; a later arrival slots into the
/// merge-made boundary; revert then leaves that arrival ending at a boundary that no longer exists.
#[tokio::test]
async fn a_revert_releases_a_value_that_arrived_after_the_merge() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    arrive(&pool, &f, "A", "2020-01-01").await?;
    arrive(&pool, &f, "B", "2020-06-01").await?;
    arrive_on(&pool, &f, source, "D", Some("2020-04-01"), None, 0.9, true).await?;
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "rv").await?;
    let after_merge = timeline_of(&pool, &f, f.lease).await?;
    arrive(&pool, &f, "N", "2020-02-01").await?;
    let after_arrival = timeline_of(&pool, &f, f.lease).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let after_revert = timeline_of(&pool, &f, f.lease).await?;
    let source_rows = timeline_of(&pool, &f, source).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(
        after_revert,
        vec![
            s("A", Some("2020-01-01"), Some("2020-02-01")),
            s("N", Some("2020-02-01"), Some("2020-06-01")),
            s("B", Some("2020-06-01"), None),
        ],
        "N no longer ends at D's start once D goes back; merge={after_merge:?} arrival={after_arrival:?}"
    );
    assert_eq!(source_rows, vec![s("D", Some("2020-04-01"), None)]);
    Ok(())
}

/// Two open values on the source (e.g. written before the predicate was declared functional):
/// a merge re-cuts a correction it made itself, and revert cannot restore the chain.
#[tokio::test]
async fn a_revert_restores_both_timelines_a_merge_rewrote() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    arrive(&pool, &f, "A", "2020-01-01").await?;
    arrive(&pool, &f, "B", "2020-06-01").await?;
    let d = arrive_on(&pool, &f, source, "D", Some("2020-03-01"), None, 0.9, false).await?;
    let e = arrive_on(&pool, &f, source, "E", Some("2020-04-01"), None, 0.9, false).await?;
    let before_target = timeline_of(&pool, &f, f.lease).await?;
    let merge =
        utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "rv").await?;
    let after_merge = timeline_of(&pool, &f, f.lease).await?;
    utopia_store::resolution::revert_merge(&pool, f.kb, merge).await?;
    let after_revert = timeline_of(&pool, &f, f.lease).await?;
    let source_rows = timeline_of(&pool, &f, source).await?;
    let alive: Vec<(Uuid, bool)> =
        sqlx::query_as("SELECT id, invalidated_at IS NULL FROM facts WHERE id = ANY($1)")
            .bind(vec![d, e])
            .fetch_all(&pool)
            .await?;
    cleanup(&pool, &f).await?;
    assert_eq!(
        after_revert, before_target,
        "target history not restored; after merge it was {after_merge:?}"
    );
    assert_eq!(source_rows.len(), 2, "source lost a value: {source_rows:?}");
    assert!(
        alive.iter().all(|(_, live)| *live),
        "both source values are live again: {alive:?}"
    );
    Ok(())
}

/// Two late values that slot into the same closed interval, reconciled concurrently.
#[tokio::test]
async fn two_late_values_reconciled_at_once_leave_one_timeline() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await?;
    let mut bad = 0;
    let trials = 40;
    let mut example = None;
    for _ in 0..trials {
        let f = seed(&pool).await?;
        arrive(&pool, &f, "2020-03-17", "2020-02-18").await?;
        arrive(&pool, &f, "2020-06-23", "2020-06-08").await?;
        let (a, b) = tokio::join!(
            arrive(&pool, &f, "2020-05-26", "2020-04-14"),
            arrive(&pool, &f, "2020-04-14", "2020-03-17"),
        );
        a?;
        b?;
        let n = overlaps(&pool, &f, f.lease).await?;
        let rows = timeline_of(&pool, &f, f.lease).await?;
        if n > 0 || rows.len() != 4 {
            bad += 1;
            example.get_or_insert(rows);
        }
        cleanup(&pool, &f).await?;
    }
    println!("RV concurrent: {bad}/{trials} trials ended with overlaps or extra rows; example={example:?}");
    assert_eq!(bad, 0);
    Ok(())
}

/// A merge that moves only closed rows still rearranges the target's timeline: the source's
/// D [03-01, 05-01) and E [05-01, 07-01) arrive beside the target's open A from 01-01, and A
/// must end where D begins.
#[tokio::test]
async fn a_merge_of_closed_rows_still_ends_the_open_one_they_follow() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let source = entity(&pool, f.kb, f.etype, "Lease Agreement", Uuid::now_v7()).await?;
    arrive(&pool, &f, "A", "2020-01-01").await?;
    arrive_on(&pool, &f, source, "D", Some("2020-03-01"), None, 0.9, true).await?;
    arrive_on(&pool, &f, source, "E", Some("2020-05-01"), None, 0.9, true).await?;
    // E's end is one the text states
    sqlx::query(
        "UPDATE facts SET valid_to = '2020-07-01', valid_to_precision = 'day'
         WHERE subject_id = $1 AND object_value #>> '{value}' = 'E' AND invalidated_at IS NULL",
    )
    .bind(source)
    .execute(&pool)
    .await?;
    utopia_store::resolution::merge_entities(&pool, f.kb, source, f.lease, None, "closed rows")
        .await?;
    let rows = timeline_of(&pool, &f, f.lease).await?;
    let n = overlaps(&pool, &f, f.lease).await?;
    cleanup(&pool, &f).await?;
    assert_eq!(n, 0, "{rows:?}");
    assert!(
        rows.contains(&s("A", Some("2020-01-01"), Some("2020-03-01"))),
        "{rows:?}"
    );
    Ok(())
}
