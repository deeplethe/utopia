use super::content_time;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use utopia_core::models::Document;
use utopia_store::documents;
use uuid::Uuid;

#[test]
fn only_a_complete_opening_dateline_sets_the_date() {
    let expected = "2024-02-29T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    for text in [
        "2024-02-29\nAnnouncement",
        "\u{feff}\r\n  2024-02-29  \r\nAnnouncement",
        "（2024年2月29日）\n公告",
        "(2024-02-29)",
    ] {
        for filename in ["filing.txt", "filing.MD", "filing.markdown"] {
            assert_eq!(content_time(filename, text.as_bytes()), Some(expected));
        }
    }
    for text in [
        "",
        " \n\t",
        "2023-02-29",
        "2024-13-01",
        "2024-02",
        "02/03/2024",
        "24-02-29",
        "2024-02-29 / 2025-02-28",
        "2024-02-29 was the previous meeting",
        "Announcement\n2024-02-29",
        "# 2024-02-29 announcement",
        "2024-02-29T12:00:00+08:00",
        "（2024年2月29日",
    ] {
        assert_eq!(content_time("filing.md", text.as_bytes()), None, "{text:?}");
    }
    for filename in [
        "filing.pdf",
        "filing.docx",
        "filing.csv",
        "2024-02-29",
        "filing",
    ] {
        assert_eq!(content_time(filename, b"2024-02-29"), None);
    }
}

#[test]
fn header_decoding_is_bounded_and_never_accepts_a_truncated_line() {
    let expected = "2024-02-29T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let mut utf16 = vec![0xff, 0xfe];
    for unit in "（2024年2月29日）\r\n公告".encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    assert_eq!(content_time("filing.txt", &utf16), Some(expected));

    let mut long = b"2024-02-29\n".to_vec();
    long.extend("正文".repeat(2000).as_bytes());
    assert_eq!(content_time("filing.md", &long), Some(expected));
    let truncated = format!(
        "{}2024-02-29 is an event, not a dateline",
        "\n".repeat(4086)
    );
    assert_eq!(content_time("filing.txt", truncated.as_bytes()), None);
    let too_late = format!("{}2024-02-29\n", "\n".repeat(4096));
    assert_eq!(content_time("filing.txt", too_late.as_bytes()), None);
}

struct Fixture {
    pool: sqlx::PgPool,
    state: crate::state::AppState,
    app: axum::Router,
    org: Uuid,
    kb: Uuid,
    other_kb: Uuid,
    user: Uuid,
    folder: Uuid,
    foreign_folder: Uuid,
    source: Uuid,
    token: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, other_kb, user, folder, foreign_folder, source) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // Only locally generated UUIDs are interpolated into fixture SQL.
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','upload-date-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','upload-date-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash)
                 VALUES ('{user}','{org}','{user}@upload.test','upload-test','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES
                 ('{kb}','{ws}','uploads'), ('{other_kb}','{ws}','other-uploads');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES
                 ('{kb}','{user}','editor'), ('{other_kb}','{user}','editor');
             INSERT INTO sources(id,kb_id,kind,name) VALUES
                 ('{folder}','{kb}','folder','uploads'),
                 ('{foreign_folder}','{other_kb}','folder','other-folder'),
                 ('{source}','{kb}','api','api-source');"
        ))
        .execute(&pool)
        .await?;
        // Processing may queue extraction, but no test calls a live model.
        utopia_store::settings::upsert(
            &pool,
            ws,
            Some("http://127.0.0.1:9"),
            None,
            Some("test-model"),
            None,
            None,
            None,
            None,
        )
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
        let token = crate::auth::issue_token(&state, user)?;
        let app = super::super::router(state.clone(), &cfg);
        Ok(Some(Self {
            pool,
            state,
            app,
            org,
            kb,
            other_kb,
            user,
            folder,
            foreign_folder,
            source,
            token,
            _dir: dir,
        }))
    }

    async fn upload(
        &self,
        kb: Uuid,
        query: &str,
        files: &[(&str, &str)],
    ) -> anyhow::Result<(StatusCode, Value)> {
        let mut body = String::new();
        for (name, text) in files {
            body.push_str(&format!(
                "--upload-boundary\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{name}\"\r\nContent-Type: text/plain\r\n\r\n{text}\r\n"
            ));
        }
        body.push_str("--upload-boundary--\r\n");
        let response = self
            .app
            .clone()
            .oneshot(
                Request::post(format!("/api/v1/kbs/{kb}/documents{query}"))
                    .header("Authorization", format!("Bearer {}", self.token))
                    .header(
                        "Content-Type",
                        "multipart/form-data; boundary=upload-boundary",
                    )
                    .body(Body::from(body))?,
            )
            .await?;
        let status = response.status();
        let body = serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
        Ok((status, body))
    }

    async fn created_docs(&self, response: &Value) -> anyhow::Result<Vec<Document>> {
        let mut docs = Vec::new();
        for value in response["created"].as_array().unwrap() {
            let id = serde_json::from_value(value["id"].clone())?;
            let doc = documents::get(&self.pool, id).await?;
            assert_eq!(serde_json::to_value(&doc)?, *value);
            docs.push(doc);
        }
        Ok(docs)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM jobs WHERE payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id IN ($1,$2))")
            .bind(self.kb).bind(self.other_kb).execute(&self.pool).await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn each_upload_keeps_its_own_date_before_processing_and_extraction() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let files = [
        ("one.txt", "2024-02-29\nFirst announcement"),
        ("two.md", "（2025年3月1日）\nSecond announcement"),
    ];
    let (status, response) = f
        .upload(
            f.kb,
            &format!("?source={}", f.folder),
            &[files[0], files[1], files[0], ("empty.txt", "")],
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{response}");
    let created = f.created_docs(&response).await?;
    assert_eq!(created.len(), 2);
    assert_eq!(response["skipped"].as_array().unwrap().len(), 2);
    let independent = sqlx::PgPool::connect(&utopia_store::test_db::url().unwrap()).await?;
    for (doc, expected) in created
        .iter()
        .zip(["2024-02-29T00:00:00Z", "2025-03-01T00:00:00Z"])
    {
        let date = Some(expected.parse::<DateTime<Utc>>()?);
        assert_eq!(
            (doc.doc_time, doc.doc_time_source.as_str(), doc.source_id),
            (date, "content", Some(f.folder))
        );
        // Claim only this test's job, using an independent connection as a worker would.
        let id: i64 = sqlx::query_scalar(
            "UPDATE jobs SET status='running', locked_at=now() WHERE kind='process_document'
             AND payload->>'document_id'=$1 AND status='queued' AND run_at<=now() RETURNING id",
        )
        .bind(doc.id.to_string())
        .fetch_one(&independent)
        .await?;
        let stored = documents::get(&independent, doc.id).await?;
        assert_eq!(
            (stored.doc_time, stored.doc_time_source.as_str()),
            (date, "content")
        );
        crate::pipeline::process_document(&f.state, doc.id).await?;
        sqlx::query("UPDATE jobs SET status='done' WHERE id=$1")
            .bind(id)
            .execute(&independent)
            .await?;
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind='extract_document' AND status='queued' AND payload->>'document_id'=$1"
        ).bind(doc.id.to_string()).fetch_one(&independent).await?;
        assert_eq!(queued, 1);
        let stored = documents::get(&independent, doc.id).await?;
        assert_eq!(
            (stored.doc_time, stored.doc_time_source.as_str()),
            (date, "content")
        );
    }
    let (_, duplicate) = f.upload(f.kb, "", &files).await?;
    assert_eq!(duplicate["created"], json!([]));
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='process_document' AND payload->>'document_id'=ANY($1)")
        .bind(created.iter().map(|d| d.id.to_string()).collect::<Vec<_>>()).fetch_one(&f.pool).await?;
    assert_eq!(jobs, 2, "duplicates must not queue more processing");
    let (status, other) = f.upload(f.other_kb, "", &[files[0]]).await?;
    assert_eq!(status, StatusCode::OK, "{other}");
    let other = f.created_docs(&other).await?;
    assert_eq!(other.len(), 1);
    assert_ne!(other[0].id, created[0].id);
    assert_eq!(other[0].kb_id, f.other_kb);
    assert_eq!(other[0].doc_time, created[0].doc_time);
    assert_eq!(other[0].doc_time_source, "content");
    f.cleanup().await
}

#[tokio::test]
async fn fallback_restore_and_other_ingest_dates_do_not_change() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, response) = f
        .upload(
            f.kb,
            "",
            &[
                ("no-date.txt", "Announcement without a date"),
                ("invalid.md", "2023-02-29\nImpossible date"),
                ("ambiguous.md", "2024-03-01 / 2025-03-01\nTwo dates"),
            ],
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{response}");
    let docs = f.created_docs(&response).await?;
    assert_eq!(docs.len(), 3);
    for doc in &docs {
        assert_eq!(doc.doc_time_source, "upload_time");
        assert_eq!(doc.doc_time, Some(doc.created_at));
    }
    // A pre-existing, dated body with upload_time must retain that historical fallback on restoration.
    let body = b"2024-02-29\nHistorical announcement";
    use sha2::{Digest, Sha256};
    let sha = super::hex(&Sha256::digest(body));
    f.state.blob.put(&sha, body).await?;
    let old = documents::create(
        &f.pool,
        f.kb,
        "old.txt",
        "text/plain",
        body.len() as i64,
        &sha,
        None,
        None,
        None,
    )
    .await?;
    documents::delete(&f.pool, f.kb, old.id, None).await?;
    let (status, restored) = f
        .upload(f.kb, "", &[("old.txt", std::str::from_utf8(body)?)])
        .await?;
    assert_eq!(status, StatusCode::OK, "{restored}");
    let restored = f.created_docs(&restored).await?;
    assert_eq!(restored.len(), 1);
    let restored = &restored[0];
    assert_eq!(restored.id, old.id);
    assert_eq!(
        (restored.doc_time, &restored.doc_time_source),
        (old.doc_time, &old.doc_time_source)
    );
    crate::pipeline::process_document(&f.state, old.id).await?;
    let reprocessed = documents::get(&f.pool, old.id).await?;
    assert_eq!(
        (reprocessed.doc_time, reprocessed.doc_time_source),
        (old.doc_time, old.doc_time_source)
    );

    let explicit = "2022-01-01T08:00:00+08:00".parse::<DateTime<Utc>>()?;
    crate::ingest_sources::ingest_upload(
        &f.state,
        f.kb,
        "json.txt",
        "text/plain",
        b"2025-03-01\nJSON",
        Some(explicit),
    )
    .await?;
    crate::ingest_sources::ingest_item(
        &f.state,
        f.kb,
        f.source,
        "api:key",
        "sync.md",
        "text/markdown",
        b"2025-03-01\nSync",
        Some(explicit),
    )
    .await?;
    for name in ["json.txt", "sync.md"] {
        let doc: Document =
            sqlx::query_as("SELECT * FROM documents WHERE kb_id=$1 AND filename=$2")
                .bind(f.kb)
                .bind(name)
                .fetch_one(&f.pool)
                .await?;
        crate::pipeline::process_document(&f.state, doc.id).await?;
        let doc = documents::get(&f.pool, doc.id).await?;
        assert_eq!(
            (doc.doc_time, doc.doc_time_source.as_str()),
            (Some(explicit), "source")
        );
    }
    f.cleanup().await
}

#[tokio::test]
async fn date_detection_keeps_upload_access_and_folder_checks() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    for source in [f.foreign_folder, f.source] {
        let (status, body) = f
            .upload(
                f.kb,
                &format!("?source={source}"),
                &[("dated.txt", "2024-03-01")],
            )
            .await?;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(body["code"], "upload_needs_folder");
    }
    sqlx::query("UPDATE kb_members SET role='viewer' WHERE kb_id=$1 AND user_id=$2")
        .bind(f.kb)
        .bind(f.user)
        .execute(&f.pool)
        .await?;
    let (status, _) = f.upload(f.kb, "", &[("dated.txt", "2024-03-01")]).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE kb_id=$1")
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
    assert_eq!(count, 0);
    f.cleanup().await
}
