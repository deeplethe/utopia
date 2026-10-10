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
