use axum::body::{to_bytes, Body};
use axum::http::Request;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

async fn send(
    app: &axum::Router,
    token: &str,
    kb: Uuid,
    conflict: Uuid,
    body: Value,
) -> anyhow::Result<(u16, Value)> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/kbs/{kb}/conflicts/{conflict}"))
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?,
        )
        .await?;
    let status = response.status().as_u16();
    Ok((
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await?)?,
    ))
}

async fn pair(
    pool: &PgPool,
    kb: Uuid,
    predicate: Uuid,
    marked: bool,
) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let subject = Uuid::now_v7();
    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Lease')")
        .bind(subject)
        .bind(kb)
        .execute(pool)
        .await?;
    let [old, new, conflict] = std::array::from_fn(|_| Uuid::now_v7());
    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_value,layer,confidence,
                                  valid_from,valid_from_precision,valid_to,valid_to_precision,end_after_period)
                 VALUES ($1,$3,$4,$5,'\"old\"','typed',0.9,'2024-01-01','year',NULL,NULL,FALSE),
                        ($2,$3,$4,$5,'\"new\"','typed',0.9,'2024-01-01','month',
                         CASE WHEN $6 THEN '2025-01-01'::timestamptz END,
                         CASE WHEN $6 THEN 'year' END,$6)")
        .bind(old).bind(new).bind(kb).bind(subject).bind(predicate).bind(marked).execute(pool).await?;
    sqlx::query("INSERT INTO fact_conflicts(id,kb_id,old_fact_id,new_fact_id,reason) VALUES ($1,$2,$3,$4,'simultaneous')")
        .bind(conflict).bind(kb).bind(old).bind(new).execute(pool).await?;
    Ok((old, new, conflict))
}

async fn counts(pool: &PgPool, kb: Uuid) -> anyhow::Result<(i64, i64, i64)> {
    Ok(sqlx::query_as("SELECT (SELECT count(*) FROM facts WHERE kb_id=$1),
                             (SELECT count(*) FROM facts WHERE kb_id=$1 AND invalidated_at IS NOT NULL),
                             (SELECT count(*) FROM audit_events WHERE kb_id=$1)")
        .bind(kb).fetch_one(pool).await?)
}

#[tokio::test]
async fn human_conflict_dates_and_settlement_commit_with_the_actor() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let [org, ws, kb, editor, viewer, predicate] = std::array::from_fn(|_| Uuid::now_v7());
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','human-conflict');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','human-conflict');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','human-conflict');
         INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
             ('{editor}','{org}','{editor}@conflict.test','editor','unused'),
             ('{viewer}','{org}','{viewer}@conflict.test','viewer','unused');
         INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{editor}','editor'),('{kb}','{viewer}','viewer');
         INSERT INTO relation_types(id,kb_id,key,label,kind,datatype,temporal,functional) VALUES
             ('{predicate}','{kb}','status','status','attribute','text','state',TRUE);"
    )).execute(&pool).await?;
    let result = async {
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig { data_dir: dir.path().to_string_lossy().into_owned(), ..Default::default() };
        let search = Arc::new(utopia_search::SearchIndex::open(&dir.path().join("search"))?);
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "conflict-test".into());
        let token = crate::auth::issue_token(&state, editor)?;
        let viewer_token = crate::auth::issue_token(&state, viewer)?;
        let app = crate::api::router(state, &cfg);
        let (_, new, conflict) = pair(&pool, kb, predicate, true).await?;
        let [statement, document, chunk] = std::array::from_fn(|_| Uuid::now_v7());
        sqlx::raw_sql(&format!(
            "INSERT INTO documents(id,kb_id,filename,sha256,status) VALUES ('{document}','{kb}','amendment.md','{document}','ready');
             INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ('{chunk}','{kb}','{document}',0,'The lease starts in June 2024.');
             INSERT INTO facts(id,kb_id,subject_id,object_value,layer,phrase,confidence)
                 SELECT '{statement}',kb_id,subject_id,object_value,'open','starts',confidence FROM facts WHERE id='{new}';
             INSERT INTO typed_fact_sources(fact_id,statement_id) VALUES ('{new}','{statement}');
             UPDATE facts SET from_statement_id='{statement}' WHERE id='{new}';
             INSERT INTO fact_evidence(fact_id,chunk_id,quote) VALUES ('{new}','{chunk}','The lease starts in June 2024.');
             INSERT INTO fact_qualifiers(fact_id,qualifier_type_id,value) VALUES ('{new}','{predicate}','\"approved\"');"
        )).execute(&pool).await?;
        let before = counts(&pool, kb).await?;
        for (body, code) in [
            (json!({"action":"retime_new"}), "new_from_required"),
            (json!({"action":"retime_new","new_from":"2026-01-01T00:00:00Z"}), "interval_inverted"),
            (json!({"action":"retime_new","new_from":"2024-06-01T00:00:00Z","new_from_precision":"unknown"}), "bad_precision"),
        ] {
            let (status, reply) = send(&app, &token, kb, conflict, body).await?;
            assert_eq!((status, reply["code"].as_str()), (422, Some(code)));
            assert_eq!(counts(&pool, kb).await?, before);
        }
        assert_eq!(send(&app, &viewer_token, kb, conflict, json!({"action":"keep"})).await?.0, 403);
        assert_eq!(send(&app, &token, Uuid::now_v7(), conflict, json!({"action":"keep"})).await?.0, 404);
        // A rejected audit insert must roll back even a successful fact rewrite.
        // Actors are deliberately not foreign keys in the append-only ledger;
        // inject the failure only for this fixture's event instead.
        let trigger = format!("fail_conflict_audit_{}", kb.simple());
        sqlx::raw_sql(&format!(
            "CREATE FUNCTION {trigger}() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN IF NEW.kb_id='{kb}' THEN RAISE EXCEPTION 'conflict audit failure'; END IF;
             RETURN NEW; END $$;
             CREATE TRIGGER {trigger} BEFORE INSERT ON audit_events FOR EACH ROW EXECUTE FUNCTION {trigger}();"
        )).execute(&pool).await?;
        let failed = utopia_store::temporal::resolve_review_conflict(&pool, kb, conflict,
            utopia_store::temporal::ConflictDecision { action: "retime_new", close_at: None, close_precision: "day",
                new_from: Some("2024-06-01T00:00:00Z".parse()?), new_precision: "month" }, editor).await;
        sqlx::raw_sql(&format!("DROP TRIGGER {trigger} ON audit_events; DROP FUNCTION {trigger}();"))
            .execute(&pool).await?;
        assert!(failed.is_err());
        assert_eq!(counts(&pool, kb).await?, before);
        let listed = utopia_store::temporal::list_conflicts(&pool, kb, 10, 0).await?;
        assert_eq!(listed[0].new_valid_from_precision.as_deref(), Some("month"));
        assert_eq!(send(&app, &token, kb, conflict, json!({"action":"retime_new",
            "new_from":"2024-06-19T14:00:00Z","new_from_precision":"month"})).await?.0, 200);
        let rewritten: (DateTime<Utc>, String, DateTime<Utc>, String, bool, bool, String) = sqlx::query_as(
            "SELECT valid_from,valid_from_precision,valid_to,valid_to_precision,end_after_period,end_derived,corrected_ends
             FROM facts WHERE supersedes=$1 AND invalidated_at IS NULL")
            .bind(new).fetch_one(&pool).await?;
        assert_eq!(rewritten, ("2024-06-01T00:00:00Z".parse()?, "month".into(),
            "2025-01-01T00:00:00Z".parse()?, "year".into(), true, false, "start".into()));
        let provenance: (Uuid, Uuid, Uuid, String, Value) = sqlx::query_as(
            "SELECT f.from_statement_id,s.statement_id,e.chunk_id,e.quote,q.value FROM facts f
             JOIN typed_fact_sources s ON s.fact_id=f.id JOIN fact_evidence e ON e.fact_id=f.id
             JOIN fact_qualifiers q ON q.fact_id=f.id WHERE f.supersedes=$1 AND f.invalidated_at IS NULL")
            .bind(new).fetch_one(&pool).await?;
        assert_eq!(provenance, (statement, statement, chunk, "The lease starts in June 2024.".into(), json!("approved")));
        let (resolution, actor, detail): (String, Uuid, Value) = sqlx::query_as(
            "SELECT c.resolution,a.actor_id,a.detail FROM fact_conflicts c JOIN audit_events a ON a.target_id=c.id
             WHERE c.id=$1 AND a.action='conflict.retime_new'")
            .bind(conflict).fetch_one(&pool).await?;
        assert_eq!((resolution.as_str(), actor, &detail["new_from_precision"]), ("retimed_new", editor, &json!("month")));
        // 实体历史把重定时那一行记在人头上，不记给引擎
        let subject: Uuid = sqlx::query_scalar("SELECT subject_id FROM facts WHERE id=$1")
            .bind(new).fetch_one(&pool).await?;
        let retimed: Uuid = sqlx::query_scalar("SELECT id FROM facts WHERE supersedes=$1 AND invalidated_at IS NULL")
            .bind(new).fetch_one(&pool).await?;
        let (history, _) = utopia_store::graph::entity_history(&pool, kb, subject, 50, 0).await?;
        for event in history.iter().filter(|e| e.kind == "corrected") {
            if event.fact_id == Some(retimed) {
                assert_eq!((event.actor_name.as_deref(), event.action.as_deref()),
                    (Some("editor"), Some("conflict.retime_new")));
            } else {
                assert_ne!(event.action.as_deref(), Some("conflict.retime_new"));
            }
        }
        assert!(history.iter().any(|e| e.kind == "corrected" && e.fact_id == Some(retimed)));

        for explicit in [false, true] {
            let (old, _, c) = pair(&pool, kb, predicate, false).await?;
            let body = if explicit { json!({"action":"close","close_at":"2024-08-13T00:00:00Z","close_at_precision":"month"}) }
                else { json!({"action":"close"}) };
            // The simultaneous default is an empty state; an explicit date can answer it.
            let (status, _) = send(&app, &token, kb, c, body).await?;
            if !explicit { assert_eq!(status, 422); continue; }
            assert_eq!(status, 200);
            let (to, precision): (DateTime<Utc>, String) = sqlx::query_as("SELECT valid_to,valid_to_precision FROM facts WHERE supersedes=$1 AND invalidated_at IS NULL")
                .bind(old).fetch_one(&pool).await?;
            assert_eq!((to, precision.as_str()), ("2024-08-01T00:00:00Z".parse()?, "month"));
        }
        let (_, new, c) = pair(&pool, kb, predicate, false).await?;
        sqlx::query("UPDATE facts SET valid_to='2024-05-01',valid_to_precision='month',end_derived=TRUE WHERE id=$1")
            .bind(new).execute(&pool).await?;
        assert_eq!(send(&app, &token, kb, c, json!({"action":"retime_new","new_from":"2024-06-01T00:00:00Z","new_from_precision":"month"})).await?.0, 200);
        let (derived, correction, to): (bool, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as("SELECT end_derived,corrected_ends,valid_to FROM facts
             WHERE subject_id=(SELECT subject_id FROM facts WHERE id=$1) AND object_value='\"new\"' AND invalidated_at IS NULL")
            .bind(new).fetch_one(&pool).await?;
        assert!(!derived, "an end reopened by reconciliation is no longer derived");
        assert_eq!(to, None, "retiming lets the engine redraw its old end");
        assert_eq!(correction.as_deref(), Some("start"));

        let (old, new, c) = pair(&pool, kb, predicate, false).await?;
        let sibling = Uuid::now_v7();
        sqlx::query("INSERT INTO fact_conflicts(id,kb_id,old_fact_id,new_fact_id,reason) VALUES ($1,$2,$3,$4,'no_time')")
            .bind(sibling).bind(kb).bind(new).bind(old).execute(&pool).await?;
        // Reversed sibling has old as successor, so it withdraws when new dies.
        assert_eq!(send(&app, &token, kb, c, json!({"action":"reject_new"})).await?.0, 200);
        let settled: (String, String) = sqlx::query_as("SELECT status,resolution FROM fact_conflicts WHERE id=$1")
            .bind(c).fetch_one(&pool).await?;
        assert_eq!(settled, ("resolved".into(), "rejected_new".into()));
        let withdrawn: (String, Option<String>) = sqlx::query_as("SELECT status,resolution FROM fact_conflicts WHERE id=$1")
            .bind(sibling).fetch_one(&pool).await?;
        assert_eq!(withdrawn, ("withdrawn".into(), None));
        let (_, _, c) = pair(&pool, kb, predicate, false).await?;
        let before = counts(&pool, kb).await?;
        let (a, b) = tokio::join!(send(&app, &token, kb, c, json!({"action":"keep"})), send(&app, &token, kb, c, json!({"action":"keep"})));
        let mut statuses = [a?.0, b?.0]; statuses.sort();
        assert_eq!(statuses[0], 200);
        assert!(matches!(statuses[1], 404 | 409));
        assert_eq!(counts(&pool, kb).await?, (before.0, before.1, before.2 + 1));
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
