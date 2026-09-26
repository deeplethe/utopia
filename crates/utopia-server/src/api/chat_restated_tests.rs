//! A restatement keeps the sources it repeats (#943).
//!
//! Asked to shorten or translate the previous answer, the assistant answers from the
//! transcript without gathering anything, as the prompt tells it to, and copies the answer's
//! `[n]`. Each turn starts with no sources, so those marks used to open nothing, and the
//! answer was labelled as citing no sources. A turn that gathers nothing now takes the previous
//! answer's sources for the numbers it repeats, and only when every number it cites was cited
//! there; otherwise it keeps none rather than guess which answer a number came from.
use super::history_tests::ask_in;
use super::*;

async fn seed_document(f: &Fx, document: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO documents(id,kb_id,filename,sha256) VALUES($1,$2,'target.md',repeat('0',64))",
    )
    .bind(document)
    .bind(f.kb)
    .execute(&f.pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks(id,kb_id,document_id,seq,text) \
         VALUES($1,$2,$3,0,'The planned target is 95%, not a measured result.')",
    )
    .bind(Uuid::now_v7())
    .bind(f.kb)
    .bind(document)
    .execute(&f.pool)
    .await?;
    Ok(())
}

/// Ask the first question, then the follow-up in the same conversation
async fn two_turns(f: &Fx) -> anyhow::Result<String> {
    let first = f.ask("What is the documented target?").await?;
    assert!(first.contains("event: done"), "{first}");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id = $1")
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
    let second = ask_in(f, id, "Say it shorter").await?;
    assert!(second.contains("event: done"), "{second}");
    Ok(second)
}

/// The sources stored with each answer, oldest first
async fn stored_sources(f: &Fx) -> anyhow::Result<Vec<serde_json::Value>> {
    Ok(sqlx::query_scalar(
        "SELECT m.sources FROM conversation_messages m
           JOIN conversations c ON c.id = m.conversation_id
          WHERE c.kb_id = $1 AND m.role = 'assistant'
          ORDER BY m.created_at",
    )
    .bind(f.kb)
    .fetch_all(&f.pool)
    .await?)
}

/// The last `sources` frame the reader was sent
fn live_sources(sse: &str) -> serde_json::Value {
    let data = sse
        .split("\n\n")
        .filter_map(|frame| frame.strip_prefix("event: sources\ndata: "))
        .last()
        .expect("a sources frame");
    serde_json::from_str(data).expect("sources are JSON")
}

const FIRST: Reply = Reply::Text("The documented target is 95% [1].");
const RESTATE: Reply = Reply::Tool("no_evidence_needed", r#"{"reason":"shorter"}"#);

#[tokio::test]
async fn a_restatement_keeps_the_sources_it_repeats() -> anyhow::Result<()> {
    let document = Uuid::now_v7();
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Document(document),
        FIRST,
        RESTATE,
        Reply::Text("Target: 95% [1]."),
    ]))
    .await?
    else {
        return Ok(());
    };
    seed_document(&f, document).await?;

    let second = two_turns(&f).await?;

    let stored = stored_sources(&f).await?;
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0][0]["n"], 1, "{stored:?}");
    assert_eq!(
        stored[1], stored[0],
        "the restated [1] opens the passage the first answer cited"
    );
    assert_eq!(
        live_sources(&second),
        stored[1],
        "live and after reload alike"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_restatement_citing_a_number_the_last_answer_did_not_keeps_none() -> anyhow::Result<()> {
    let document = Uuid::now_v7();
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Document(document),
        FIRST,
        RESTATE,
        Reply::Text("Target: 95% [2]."),
    ]))
    .await?
    else {
        return Ok(());
    };
    seed_document(&f, document).await?;

    let second = two_turns(&f).await?;

    let stored = stored_sources(&f).await?;
    assert_eq!(
        stored[1],
        serde_json::json!([]),
        "[2] was never cited: no guessing"
    );
    assert_eq!(live_sources(&second), serde_json::json!([]));
    f.cleanup().await
}

#[tokio::test]
async fn a_turn_that_gathers_keeps_only_its_own_sources() -> anyhow::Result<()> {
    let document = Uuid::now_v7();
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Document(document),
        FIRST,
        Reply::Tool("search_chunks", r#"{"query":"target"}"#),
        Reply::Text("Target: 95% [1]."),
    ]))
    .await?
    else {
        return Ok(());
    };
    seed_document(&f, document).await?;

    let second = two_turns(&f).await?;

    assert!(
        second.contains("event: step"),
        "the second turn searched: {second}"
    );
    let stored = stored_sources(&f).await?;
    assert_eq!(
        stored[1],
        serde_json::json!([]),
        "a turn that searched answers for its own sources"
    );
    f.cleanup().await
}

/// The server reads citation numbers in the shape the page draws as marks (`citeRe`)
#[test]
fn cited_numbers_reads_the_shapes_the_page_marks() {
    let read = |text: &str| cited_numbers(text).into_iter().collect::<Vec<_>>();
    assert_eq!(
        read("见 [1][3] 与 [4, 6]，另见 [2，5]"),
        vec![1, 2, 3, 4, 5, 6]
    );
    for text in ["[ 1]", "[1 ]", "rows[0]", "[a1]", "[1,]", "[+1]", "[]"] {
        assert!(read(text).is_empty(), "{text}");
    }
    assert_eq!(read("[[2]]"), vec![2]);
}
