use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use utopia_store::graph;
use uuid::Uuid;

struct Fixture {
    pool: PgPool,
    app: Router,
    _directory: tempfile::TempDir,
    org: Uuid,
    kb: Uuid,
    foreign_kb: Uuid,
    editor: Uuid,
    editor_token: String,
    viewer_token: String,
    statement: Uuid,
    value_statement: Uuid,
    foreign_statement: Uuid,
    property: Uuid,
    attribute: Uuid,
    foreign_property: Uuid,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, workspace, kb, foreign_kb, editor, viewer) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let (subject, object, foreign_subject, foreign_object) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let (property, attribute, foreign_property) =
            (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
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
                ('{foreign_subject}','{foreign_kb}','Foreign Alice'),
                ('{foreign_object}','{foreign_kb}','Foreign Acme');
            INSERT INTO relation_types(id,kb_id,key,label,kind,datatype) VALUES
                ('{property}','{kb}','worksFor','works for','relation',NULL),
                ('{attribute}','{kb}','salary','salary','attribute','number'),
                ('{foreign_property}','{foreign_kb}','worksFor','works for','relation',NULL);
        "#
        ))
        .execute(&pool)
        .await?;
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
        let value = json!({"type":"number","value":42});
        let value_statement = graph::insert_open_statement(
            &pool,
            kb,
            subject,
            "earns",
            graph::FactObject::Value(&value),
            None,
            1.0,
        )
        .await?
        .0;
        let foreign_statement = graph::insert_open_statement(
            &pool,
            foreign_kb,
            foreign_subject,
            "works for",
            graph::FactObject::Entity(foreign_object),
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
        Ok(Some(Self {
            pool,
            app,
            _directory: directory,
            org,
            kb,
            foreign_kb,
            editor,
            editor_token,
            viewer_token,
            statement,
            value_statement,
            foreign_statement,
            property,
            attribute,
            foreign_property,
        }))
    }

    async fn post(
        &self,
        kb: Uuid,
        token: Option<&str>,
        payload: Value,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let mut request = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/kbs/{kb}/ontology/regressions"))
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(payload.to_string()))?)
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 65536).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    async fn case_count(&self) -> anyhow::Result<i64> {
        Ok(
            sqlx::query_scalar(
                "SELECT count(*) FROM ontology_regression_cases WHERE kb_id=ANY($1)",
            )
            .bind(vec![self.kb, self.foreign_kb])
            .fetch_one(&self.pool)
            .await?,
        )
    }

    async fn business_counts(&self) -> anyhow::Result<(i64, i64, i64, i64)> {
        Ok(sqlx::query_as("SELECT
            (SELECT count(*) FROM facts WHERE kb_id=ANY($1)),
            (SELECT count(*) FROM entities WHERE kb_id=ANY($1)),
            (SELECT count(*) FROM phrase_bindings WHERE kb_id=ANY($1)),
            (SELECT count(*) FROM jobs WHERE payload->>'kb_id'=ANY(ARRAY(SELECT unnest($1::uuid[])::text)))")
            .bind(vec![self.kb, self.foreign_kb]).fetch_one(&self.pool).await?)
    }

    async fn finish(self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        result
    }
}

fn input(statement: Uuid, property: Uuid, direction: &str) -> Value {
    json!({"statement_id": statement, "relation_type_id": property, "direction": direction})
}

#[tokio::test]
async fn authenticated_editor_records_entity_and_value_examples_without_scheduling_or_binding(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let before = f.business_counts().await?;
        let mut payload = input(f.statement, f.property, "reverse");
        // Attribution is derived from authentication even if a client supplies these fields.
        payload["created_by"] = json!(Uuid::now_v7());
        payload["origin"] = json!("adoption");
        let (status, body) = f.post(f.kb, Some(&f.editor_token), payload.clone()).await?;
        anyhow::ensure!(status == StatusCode::CREATED, "{status}: {body}");
        let id: Uuid = serde_json::from_value(body["id"].clone())?;
        let case = utopia_store::ontology_regressions::get(&f.pool, f.kb, id)
            .await?
            .unwrap();
        anyhow::ensure!(case.statement_id == f.statement);
        anyhow::ensure!(case.expected_property_id == f.property);
        anyhow::ensure!(case.expected_direction == "reverse");
        anyhow::ensure!(case.created_by == Some(f.editor) && case.origin == "person");
        anyhow::ensure!(case.last_checked_at.is_none() && case.last_result.is_none());

        let (status, retry) = f.post(f.kb, Some(&f.editor_token), payload).await?;
        anyhow::ensure!(status == StatusCode::CREATED && retry["id"] == body["id"]);
        let (status, body) = f
            .post(
                f.kb,
                Some(&f.editor_token),
                input(f.value_statement, f.attribute, "forward"),
            )
            .await?;
        anyhow::ensure!(status == StatusCode::CREATED, "{status}: {body}");
        let id: Uuid = serde_json::from_value(body["id"].clone())?;
        let value_case = utopia_store::ontology_regressions::get(&f.pool, f.kb, id)
            .await?
            .unwrap();
        anyhow::ensure!(value_case.statement_id == f.value_statement);
        anyhow::ensure!(value_case.expected_property_id == f.attribute);
        anyhow::ensure!(value_case.expected_direction == "forward");
        anyhow::ensure!(value_case.created_by == Some(f.editor) && value_case.origin == "person");
        anyhow::ensure!(f.case_count().await? == 2);
        anyhow::ensure!(
            f.business_counts().await? == before,
            "creating an example changed production state"
        );
        anyhow::Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn regression_creation_rejects_missing_access_cross_kb_and_invalid_shapes_without_writes(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let before = f.business_counts().await?;
        for (kb, token, statement, property, direction, expected, code) in [
            (
                f.kb,
                None,
                f.statement,
                f.property,
                "forward",
                StatusCode::UNAUTHORIZED,
                None,
            ),
            (
                f.kb,
                Some(f.viewer_token.as_str()),
                f.statement,
                f.property,
                "forward",
                StatusCode::FORBIDDEN,
                None,
            ),
            (
                f.foreign_kb,
                Some(f.editor_token.as_str()),
                f.foreign_statement,
                f.foreign_property,
                "forward",
                StatusCode::NOT_FOUND,
                None,
            ),
            (
                f.kb,
                Some(f.editor_token.as_str()),
                f.foreign_statement,
                f.property,
                "forward",
                StatusCode::NOT_FOUND,
                None,
            ),
            (
                f.kb,
                Some(f.editor_token.as_str()),
                f.statement,
                f.foreign_property,
                "forward",
                StatusCode::NOT_FOUND,
                None,
            ),
            (
                f.kb,
                Some(f.editor_token.as_str()),
                f.statement,
                f.property,
                "sideways",
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("bad_direction"),
            ),
            (
                f.kb,
                Some(f.editor_token.as_str()),
                f.statement,
                f.attribute,
                "forward",
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("bad_property_shape"),
            ),
            (
                f.kb,
                Some(f.editor_token.as_str()),
                f.value_statement,
                f.property,
                "forward",
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("bad_property_shape"),
            ),
            (
                f.kb,
                Some(f.editor_token.as_str()),
                f.value_statement,
                f.attribute,
                "reverse",
                StatusCode::UNPROCESSABLE_ENTITY,
                Some("bad_property_shape"),
            ),
        ] {
            let (status, body) = f
                .post(kb, token, input(statement, property, direction))
                .await?;
            anyhow::ensure!(status == expected, "{direction}: {status}: {body}");
            if let Some(code) = code {
                anyhow::ensure!(body["code"] == code, "{body}");
            }
            anyhow::ensure!(body.get("id").is_none());
            anyhow::ensure!(f.case_count().await? == 0);
            anyhow::ensure!(f.business_counts().await? == before);
        }
        anyhow::Ok(())
    }
    .await;
    f.finish(result).await
}
