//! The metadata-only form must not replace any part of a computed definition.
use crate::state::AppState;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn metadata_patch_preserves_computed_definition_through_authenticated_routes(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb, user, class, input, output) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let dir = tempfile::tempdir()?;
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = AppState::new(
        pool.clone(),
        &cfg,
        Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?),
        "test-only".into(),
    );
    let result = async {
        sqlx::raw_sql(&format!("INSERT INTO organizations(id,name) VALUES('{org}','metadata-test');
        INSERT INTO workspaces(id,org_id,name) VALUES('{ws}','{org}','metadata-test');
        INSERT INTO knowledge_bases(id,workspace_id,name) VALUES('{kb}','{ws}','metadata-test');
        INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES('{user}','{org}','{user}@example.test','unused','Test');
        INSERT INTO kb_members(kb_id,user_id,role) VALUES('{kb}','{user}','editor');
        INSERT INTO entity_types(id,kb_id,key,label) VALUES('{class}','{kb}','thing','Thing');
        INSERT INTO relation_types(id,kb_id,key,label,kind,datatype) VALUES('{input}','{kb}','input','Input','attribute','number'),('{output}','{kb}','output','Output','attribute','number');"))
            .execute(&pool).await?;
        let token = crate::auth::issue_token(&state, user)?;
        let call = |method: &'static str, path: String, body: Value| {
            let state = state.clone(); let token = token.clone();
            async move {
                let request = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"))
                    .header("content-type","application/json").body(Body::from(body.to_string()))?;
                let response = super::router(state, &Default::default()).oneshot(request).await?;
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 65536).await?;
                anyhow::Ok((status, serde_json::from_slice::<Value>(&body)?))
            }
        };
        let base = format!("/api/v1/kbs/{kb}/rules");
        let expr = json!({"op":"div","l":{"op":"sub","l":{"attr":input},"r":{"const":2}},"r":{"attr":input}});
        let definition = json!({"name":"Original", "subject_type_id":class, "conclusion":"computed", "conclude_predicate_id":output,"conclude_expr":expr,"conditions":[{"predicate_id":input,"op":"gt","operand":3,"group":2},{"predicate_id":input,"op":"lt","operand":-1,"group":7}]});
        let (status, created) = call("POST",base.clone(),definition.clone()).await?;
        anyhow::ensure!(status.is_success(), "create: {status} {created}");
        let id = created["id"].as_str().unwrap();
        let (_, before) = call("GET",base.clone(),json!(null)).await?;
        let snapshot = |value: &Value| { let r=&value["rules"][0]; json!({"expr":r["conclude_expr"],"conditions":r["conditions"],"conclusion":r["conclusion"],"predicate":r["conclude_predicate_id"],"subject":r["subject_type_id"]}) };
        let path = format!("{base}/{id}");
        // The previous form sent the entire conclusion without its expression.
        let mut old_editor = definition;
        old_editor.as_object_mut().unwrap().remove("conclude_expr");
        let (old_status, old_error) = call("PATCH", path.clone(), old_editor).await?;
        anyhow::ensure!(old_status == StatusCode::UNPROCESSABLE_ENTITY);
        anyhow::ensure!(old_error["code"] == "no_expression");

        let (status, _) = call("PATCH", path.clone(), json!({"name":"Renamed","description":"Only metadata"})).await?;
        anyhow::ensure!(status == StatusCode::OK);
        let (_, after) = call("GET",base.clone(),json!(null)).await?;
        anyhow::ensure!(snapshot(&before) == snapshot(&after));
        anyhow::ensure!(after["rules"][0]["name"] == "Renamed");
        // Rejecting a name or permission never writes a partial definition.
        anyhow::ensure!(call("PATCH",path.clone(),json!({"name":" "})).await?.0 == StatusCode::UNPROCESSABLE_ENTITY);
        sqlx::query("UPDATE kb_members SET role='viewer' WHERE user_id=$1").bind(user).execute(&pool).await?;
        anyhow::ensure!(call("PATCH",path,json!({"name":"Forbidden"})).await?.0 == StatusCode::FORBIDDEN);
        let (_, unchanged) = call("GET",base,json!(null)).await?;
        anyhow::ensure!(unchanged == after);
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
