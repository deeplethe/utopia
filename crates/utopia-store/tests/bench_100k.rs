//! 100k-document benchmark — first cut (#713).
//!
//! Times `graph::entity_detail` (the read path a base opens with) against
//! a corpus of N documents populated through the real write functions.
//! Gated behind `UTOPIA_BENCH=1` so it does not run in CI.
//!
//! ## Running
//!
//! ```text
//! UTOPIA_DATABASE_URL=postgres://utopia:utopia@127.0.0.1:1517/utopia \
//!     UTOPIA_BENCH=1 \
//!     cargo test -p utopia-store --test bench_100k -- --nocapture
//! ```
//!
//! ## Corpus shape (default)
//!
//! - `UTOPIA_BENCH_DOCS` (default 1000): number of `documents` rows.
//! - Each document has 3 chunks (the maintainer's "few hundred thousand
//!   chunks" range, scaled down).
//! - Each chunk carries 2 derived facts (with merge history on hubs).
//! - 2 hub entities each with `UTOPIA_BENCH_HUB_FACTS` facts (default 1000)
//!   are also created; the timed scenario targets one of them.
//!
//! The corpus is intentionally small enough to populate in a few minutes
//! on a local Postgres, so a bench round is a "do this once a month"
//! thing rather than an overnight job. The numbers are *relative*: the
//! report captures the same machine's numbers so subsequent runs can
//! be diffed.
//!
//! ## Output
//!
//! Writes a markdown report to `docs/benchmarks/<utc-date>-100k.md`.
//! The bench fails loud if the directory does not exist or is not
//! writable — it is the operator's signal that today's run did not
//! land anywhere.
//!
//! ## Cleanup
//!
//! The bench runs inside a savepoint and rolls back, leaving the
//! database untouched. Repeated runs do not accumulate state.

use chrono::Utc;
use sqlx::PgPool;
use std::time::{Duration, Instant};
use utopia_ingest::chunk_text;
use utopia_ingest::Provenance;
use uuid::Uuid;

const DEFAULT_DOCS: usize = 1000;
const CHUNKS_PER_DOC: usize = 3;
const FACTS_PER_CHUNK: usize = 2;
const DEFAULT_HUB_FACTS: usize = 1000;
const ITERATIONS: usize = 30;
const REPORT_HEADER: &str = "# 100k benchmark — cold `entity_detail`\n\n";

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn report_path() -> std::path::PathBuf {
    // The bench runs under `cargo test` from the crate directory; resolve
    // the workspace root from CARGO_MANIFEST_DIR so the report lands
    // next to its peers in docs/benchmarks/ regardless of cwd.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("test crate lives at crates/<crate>/");
    workspace_root
        .join("docs/benchmarks")
        .join(format!("{}-100k.md", Utc::now().format("%Y-%m-%d")))
}

async fn kb(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let user = Uuid::now_v7();
    let user_email = format!("bench-{}@local", Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'bench-100k')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'bench-100k')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'bench-100k')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, password_hash, display_name) \
         VALUES ($1, $2, $3, 'unused', 'bench')",
    )
    .bind(user)
    .bind(org)
    .bind(&user_email)
    .execute(pool)
    .await?;
    Ok((kb, user, org))
}

async fn source_id(pool: &PgPool, kb_id: Uuid) -> anyhow::Result<Uuid> {
    let src = utopia_store::sources::create(
        pool,
        kb_id,
        "custom",
        "bench-100k-source",
        &serde_json::json!({}),
        None,
        None,
        None,
    )
    .await?;
    Ok(src.id)
}

async fn populate_corpus(
    pool: &PgPool,
    kb_id: Uuid,
    _org_id: Uuid,
    source_id: Uuid,
    n_docs: usize,
    hub_facts: usize,
) -> anyhow::Result<(Uuid, Uuid)> {
    // Two hubs share the corpus's facts; the bench targets one of them.
    // We need subject_id and object_id to call insert_fact; build a
    // small graph where each document's facts are between two entity
    // rows we create up front. To keep the fixture realistic we make
    // every document's "subject" be the same hub and the "object" be
    // its own per-document entity, so the hub accumulates facts.
    let hub_id = Uuid::now_v7();
    let spare_id = Uuid::now_v7();
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'hub')")
        .bind(hub_id)
        .bind(kb_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, 'spare')")
        .bind(spare_id)
        .bind(kb_id)
        .execute(pool)
        .await?;

    for i in 0..n_docs {
        let body = format!(
            "{i}. A document with enough text to produce three chunks of meaningful prose. \
             The hub appeared here on date T. Some facts about the hub follow. \
             Pad out the body so chunk_text has work to to do; chunk_text ignores \
             short documents because of the TINY_TOKENS guard.\n\n\
             A second paragraph with another mention of the hub, a second fact, \
             and a third mention. The hub did something else on date T.\n\n\
             A third paragraph to ensure we hit three chunks. The hub was last \
             seen on date T, doing a third thing.",
            i = i
        );
        populate_document(pool, kb_id, source_id, hub_id, spare_id, &body, i).await?;
    }

    // Push the hub past `hub_facts` total — the per-document loop above
    // contributes ~2 facts per document. The remainder is direct
    // insert_fact calls; doing this rather than scaling the per-document
    // fact count keeps the per-document shape realistic.
    let contributed = (n_docs * FACTS_PER_CHUNK) as i64;
    let remaining = (hub_facts as i64).saturating_sub(contributed);
    for i in 0..remaining {
        utopia_store::graph::insert_fact(
            pool,
            kb_id,
            hub_id,
            None,
            spare_id,
            utopia_store::graph::Validity::default(),
            0.95,
        )
        .await?;
        let _ = i;
    }

    Ok((hub_id, spare_id))
}

async fn populate_document(
    pool: &PgPool,
    kb_id: Uuid,
    source_id: Uuid,
    hub_id: Uuid,
    spare_id: Uuid,
    body: &str,
    i: usize,
) -> anyhow::Result<()> {
    // Document row first, in its own tx. It has to be committed before
    // replace_chunks opens its own tx, or the chunks FK has nothing to
    // point at. (The chunks write is short and idempotent under retry;
    // the document insert is the longer one.)
    let mut doc_tx = pool.begin().await?;
    let doc = utopia_store::documents::upsert_source_document_tx(
        &mut doc_tx,
        kb_id,
        source_id,
        &format!("bench-{}-{}", i, std::process::id()),
        &format!("bench-{}-{}.txt", i, std::process::id()),
        "text/plain",
        body.len() as i64,
        &format!("bench-sha-{}", Uuid::now_v7()),
        None,
    )
    .await?;
    doc_tx.commit().await?;
    let chunks = chunk_text(body);
    let typed_pieces: Vec<utopia_ingest::ChunkPiece> = chunks
        .iter()
        .enumerate()
        .map(|(idx, p)| utopia_ingest::ChunkPiece {
            seq: idx as i32,
            text: p.text.clone(),
            char_start: p.char_start,
            char_end: p.char_end,
            heading: p.heading.clone(),
            provenance: Provenance::stated(),
        })
        .collect();
    let inserted =
        utopia_store::documents::replace_chunks(pool, kb_id, doc.id, &typed_pieces).await?;
    utopia_store::documents::set_ready(pool, doc.id, body.len() as i32, inserted.len() as i32)
        .await?;
    for _ in 0..FACTS_PER_CHUNK {
        utopia_store::graph::insert_fact(
            pool,
            kb_id,
            hub_id,
            None,
            spare_id,
            utopia_store::graph::Validity::default(),
            0.9,
        )
        .await?;
    }
    Ok(())
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let n = sorted.len();
    if n == 0 {
        return Duration::ZERO;
    }
    let idx = ((p / 100.0) * (n as f64 - 1.0)).round() as usize;
    sorted[idx.min(n - 1)]
}

fn write_report(path: &std::path::Path, lines: &[String]) -> anyhow::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, lines.join("\n") + "\n")?;
    Ok(())
}

#[tokio::test]
async fn bench_entity_detail_against_real_tables() -> anyhow::Result<()> {
    if std::env::var_os("UTOPIA_BENCH").is_none() {
        eprintln!(
            "跳过：未设 UTOPIA_BENCH（设 UTOPIA_BENCH=1 启用；bench 装载 ~2.5M 行到本地 Postgres）"
        );
        return Ok(());
    }
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };

    let n_docs = env_usize("UTOPIA_BENCH_DOCS", DEFAULT_DOCS);
    let hub_facts = env_usize("UTOPIA_BENCH_HUB_FACTS", DEFAULT_HUB_FACTS);
    let iterations = env_usize("UTOPIA_BENCH_ITERATIONS", ITERATIONS);

    eprintln!(
        "[bench-100k] config: docs={n_docs} chunks_per_doc={CHUNKS_PER_DOC} \
         facts_per_chunk={FACTS_PER_CHUNK} hub_facts={hub_facts} iterations={iterations}"
    );

    let pool = PgPool::connect(&url).await?;
    let started = Instant::now();
    let (kb_id, _user_id, _org_id) = kb(&pool).await?;
    let source_id = source_id(&pool, kb_id).await?;
    let (hub_id, _spare_id) =
        populate_corpus(&pool, kb_id, _org_id, source_id, n_docs, hub_facts).await?;
    let populate_elapsed = started.elapsed();
    eprintln!("[bench-100k] populated corpus in {:.2?}", populate_elapsed);

    // Warmup
    let now = Utc::now();
    let _ = utopia_store::graph::entity_detail(&pool, kb_id, hub_id, Some(now), Some(now)).await?;

    let mut samples: Vec<Duration> = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t0 = Instant::now();
        let _ =
            utopia_store::graph::entity_detail(&pool, kb_id, hub_id, Some(now), Some(now)).await?;
        samples.push(t0.elapsed());
    }
    samples.sort();

    // 自建自拆：装的这一份语料到此为止。组织一删，工作区、库、文档、分块、事实
    // 一路级联跟着走（外键都是 ON DELETE CASCADE）。**量完立刻拆**，不放到写报告
    // 之后——报告写不出来也不该把上百万行留在别人的库里
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(_org_id)
        .execute(&pool)
        .await?;

    let p50 = percentile(&samples, 50.0);
    let p95 = percentile(&samples, 95.0);
    let p99 = percentile(&samples, 99.0);
    let mean: Duration = {
        let total: Duration = samples.iter().sum();
        total / samples.len() as u32
    };
    let throughput = iterations as f64 / samples.iter().map(|d| d.as_secs_f64()).sum::<f64>();

    let mut report = vec![REPORT_HEADER.to_string()];
    report.push(format!(
        "- **Date (UTC):** {}",
        Utc::now().format("%Y-%m-%d")
    ));
    report.push(format!(
        "- **Corpus:** docs={n_docs} chunks_per_doc={CHUNKS_PER_DOC} \
         facts_per_chunk={FACTS_PER_CHUNK} hub_facts={hub_facts}"
    ));
    report.push(
        "- **Scenario:** `graph::entity_detail(kb_id, hub_id, at=now, as_of=now)`".to_string(),
    );
    report.push(format!(
        "- **Iterations:** {iterations} (1 warmup discarded)"
    ));
    report.push(String::new());
    report.push("## Populate".to_string());
    report.push(format!("- **Elapsed:** {:.2?}", populate_elapsed));
    report.push(String::new());
    report.push("## Read path latency".to_string());
    report.push("| metric | value |".to_string());
    report.push("|---|---|".to_string());
    report.push(format!("| p50 | {:.2?} |", p50));
    report.push(format!("| p95 | {:.2?} |", p95));
    report.push(format!("| p99 | {:.2?} |", p99));
    report.push(format!("| mean | {:.2?} |", mean));
    report.push(format!("| throughput | {:.1} req/s |", throughput));

    let path = report_path();
    write_report(&path, &report)?;
    eprintln!("[bench-100k] wrote report to {}", path.display());

    // Sanity: result must be present
    assert!(path.exists(), "report path {} should exist", path.display());
    Ok(())
}
