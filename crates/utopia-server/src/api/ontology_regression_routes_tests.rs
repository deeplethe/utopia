use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use utopia_store::graph;
use uuid::Uuid;

async fn post(
    app: &Router,
    kb: Uuid,
    token: &str,
    statement: Uuid,
    property: Uuid,
) -> anyhow::Result<axum::response::Response> {
    Ok(app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/kbs/{kb}/ontology/regressions"))
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(
                    json!({
                        "statement_id": statement,
                        "relation_type_id": property,
                        "direction": "forward"
                    })
                    .to_string(),
                ))?,
        )
        .await?)
}

#[tokio::test]
async fn editor_adds_regression_case_while_viewer_and_cross_kb_statement_are_rejected(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, workspace, kb, foreign_kb, editor, viewer, subject, object, foreign_entity, property) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    // Only locally generated UUIDs are interpolated into this fixture.
    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO organizations(id,name) VALUES ('{org}','case-route-test');
        INSERT INTO workspaces(id,org_id,name) VALUES ('{workspace}','{org}','case-route-test');
        INSERT INTO knowledge_bases(id,workspace_id,name,visibility) VALUES
            ('{kb}','{workspace}','case-route-test','restricted'),
            ('{foreign_kb}','{workspace}','foreign-case-route-test','restricted');
        INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES
            ('{editor}','{org}','{editor}@example.test','unused','Editor'),
            ('{viewer}','{org}','{viewer}@example.test','unused','Viewer');
        INSERT INTO kb_members(kb_id,user_id,role) VALUES
            ('{kb}','{editor}','editor'), ('{kb}','{viewer}','viewer');
        INSERT INTO entities(id,kb_id,canonical_name) VALUES
            ('{subject}','{kb}','Alice'), ('{object}','{kb}','Acme'),
            ('{foreign_entity}','{foreign_kb}','Foreign entity');
        INSERT INTO relation_types(id,kb_id,key,label,kind) VALUES
            ('{property}','{kb}','worksFor','works for','relation');
        "#
    ))
    .execute(&pool)
    .await?;
    let result = async {
        let statement = graph::insert_open_statement(
            &pool,
            kb,
            subject,
            "works for",
            graph::FactObject::Entity(object),
            None,
            1.0,
        )
        .await?
        .0;
        let foreign_statement = graph::insert_open_statement(
            &pool,
            foreign_kb,
            foreign_entity,
            "refers to",
            graph::FactObject::Entity(foreign_entity),
            None,
            1.0,
        )
        .await?
        .0;
        let directory = tempfile::tempdir()?;
        let config = utopia_core::config::AppConfig {
            data_dir: directory.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &directory.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool.clone(), &config, search, "test-only".into());
        let editor_token = crate::auth::issue_token(&state, editor)?;
        let viewer_token = crate::auth::issue_token(&state, viewer)?;
        let app = crate::api::router(state, &config);

        let response = post(&app, kb, &editor_token, statement, property).await?;
        anyhow::ensure!(response.status() == StatusCode::CREATED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
        let id: Uuid = serde_json::from_value(body["id"].clone())?;
        let saved: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM ontology_regression_cases WHERE id=$1 AND kb_id=$2
             AND statement_id=$3 AND expected_property_id=$4 AND expected_direction='forward'
             AND created_by=$5 AND origin='person')",
        )
        .bind(id)
        .bind(kb)
        .bind(statement)
        .bind(property)
        .bind(editor)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(saved);

        for (token, statement, expected) in [
            (&viewer_token, statement, StatusCode::FORBIDDEN),
            (&editor_token, foreign_statement, StatusCode::NOT_FOUND),
        ] {
            let response = post(&app, kb, token, statement, property).await?;
            anyhow::ensure!(response.status() == expected);
        }
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}

#[tokio::test]
async fn viewer_reads_only_live_cases_in_the_authorized_kb() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (
        org,
        workspace,
        kb,
        foreign_kb,
        viewer,
        subject,
        foreign_subject,
        property,
        foreign_property,
    ) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO organizations(id,name) VALUES ('{org}','case-list-test');
        INSERT INTO workspaces(id,org_id,name) VALUES ('{workspace}','{org}','case-list-test');
        INSERT INTO knowledge_bases(id,workspace_id,name,visibility) VALUES
            ('{kb}','{workspace}','case-list-test','restricted'),
            ('{foreign_kb}','{workspace}','foreign-case-list-test','restricted');
        INSERT INTO users(id,org_id,email,password_hash,display_name)
            VALUES ('{viewer}','{org}','{viewer}@example.test','unused','Viewer');
        INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{viewer}','viewer');
        INSERT INTO entities(id,kb_id,canonical_name) VALUES
            ('{subject}','{kb}','Alice'), ('{foreign_subject}','{foreign_kb}','Foreign');
        INSERT INTO relation_types(id,kb_id,key,label,kind) VALUES
            ('{property}','{kb}','knows','knows','relation'),
            ('{foreign_property}','{foreign_kb}','foreign','foreign','relation');
        "#
    ))
    .execute(&pool)
    .await?;
    let result = async {
        let mut statement = Uuid::nil();
        for (case_kb, entity, expected) in [
            (kb, subject, property),
            (foreign_kb, foreign_subject, foreign_property),
        ] {
            let id = graph::insert_open_statement(
                &pool, case_kb, entity, "knows", graph::FactObject::Entity(entity), None, 1.0,
            ).await?.0;
            utopia_store::ontology_regressions::add(&pool, case_kb,
                utopia_store::ontology_regressions::NewCase {
                    statement_id: id, expected_property_id: expected,
                    expected_direction: "forward", created_by: viewer, origin: "person",
                },
            ).await?;
            if case_kb == kb {
                statement = id;
            }
        }
        let cached = json!({"passed": true, "human_bound": true, "actual_property_id": property,
            "actual_direction": "forward", "status": "bound"});
        sqlx::query("UPDATE ontology_regression_cases SET last_result=$2,last_checked_at=now() WHERE kb_id=$1")
            .bind(kb).bind(&cached).execute(&pool).await?;
        let checked_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT last_checked_at FROM ontology_regression_cases WHERE kb_id=$1",
        ).bind(kb).fetch_one(&pool).await?;
        let directory = tempfile::tempdir()?;
        let config = utopia_core::config::AppConfig {
            data_dir: directory.path().to_string_lossy().into_owned(), ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(&directory.path().join("search"))?);
        let state = crate::state::AppState::new(pool.clone(), &config, search, "test-only".into());
        let token = crate::auth::issue_token(&state, viewer)?;
        let app = crate::api::router(state, &config);
        for (requested_kb, expected) in [(kb, StatusCode::OK), (foreign_kb, StatusCode::NOT_FOUND)] {
            let response = app.clone().oneshot(Request::builder()
                .uri(format!("/api/v1/kbs/{requested_kb}/ontology/regressions"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())?).await?;
            anyhow::ensure!(response.status() == expected);
            if requested_kb == kb {
                let body: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
                let cases = body["cases"].as_array().unwrap();
                anyhow::ensure!(cases.len() == 1);
                let case = &cases[0];
                anyhow::ensure!(case["statement_id"] == json!(statement) && case["kb_id"] == json!(kb));
                anyhow::ensure!(case["subject_label"] == "Alice" && case["object_label"] == "Alice" && case["phrase"] == "knows");
                anyhow::ensure!(case["expected_property_key"] == "knows" && case["actual_property_label"] == "knows" && case["created_by_label"] == "Viewer");
                anyhow::ensure!(case["last_result"] == cached && case["last_checked_at"] == json!(checked_at));
            }
        }
        let unchanged: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT last_checked_at FROM ontology_regression_cases WHERE kb_id=$1",
        ).bind(kb).fetch_one(&pool).await?;
        anyhow::ensure!(unchanged == checked_at);
        sqlx::query("UPDATE facts SET invalidated_at=now() WHERE id=$1").bind(statement).execute(&pool).await?;
        let response = app.oneshot(Request::builder()
            .uri(format!("/api/v1/kbs/{kb}/ontology/regressions"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())?).await?;
        anyhow::ensure!(response.status() == StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&to_bytes(response.into_body(), 65536).await?)?;
        anyhow::ensure!(body["cases"] == json!([]));
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
