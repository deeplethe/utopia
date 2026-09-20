//! Exercise expression thresholds through authenticated writes, then the real materializer.
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
async fn expression_operands_round_trip_and_execute_through_authenticated_routes(
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
        let expr = json!({"op":"sub","l":{"attr":input},"r":{"attr":output}});
        let definition = json!({"name":"Threshold", "subject_type_id":class, "conclusion":"attribute", "conclude_predicate_id":output,"conclude_value":"passed","conditions":[{"predicate_id":input,"op":"gt","operand":expr,"group":2}]});
        let (status, created) = call("POST",base.clone(),definition.clone()).await?;
        anyhow::ensure!(status.is_success(), "expression POST: {status} {created}");
        let id = created["id"].as_str().unwrap();
        let path = format!("{base}/{id}");
        let entity = Uuid::now_v7();
        sqlx::query("INSERT INTO entities(id,kb_id,type_id,canonical_name) VALUES($1,$2,$3,'Subject')")
            .bind(entity).bind(kb).bind(class).execute(&pool).await?;
        let mut facts = Vec::new();
        for (predicate, value) in [(input,100), (output,40)] {
            let fact = Uuid::now_v7();
            sqlx::query("INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_value,valid_from,valid_from_precision,confidence) VALUES($1,$2,$3,$4,$5,'2023-01-01','day',0.9)")
                .bind(fact).bind(kb).bind(entity).bind(predicate).bind(json!({"value":value})).execute(&pool).await?;
            facts.push(fact);
        }
        // A non-computed conclusion ensures this exercises the condition operand.
        for (op, hits) in [("gt",1),("gte",1),("lt",0),("lte",0)] {
            let conditions = json!([{"predicate_id":input,"op":op,"operand":expr,"group":2}]);
            anyhow::ensure!(call("PATCH",path.clone(),json!({"conditions":conditions})).await?.0.is_success());
            let (_, read) = call("GET",base.clone(),json!(null)).await?;
            anyhow::ensure!(read["rules"][0]["conditions"][0]["operand"] == expr);
            let report = utopia_store::reasoning::materialize(&pool,kb).await?;
            anyhow::ensure!(report.rule_hits == hits, "{op}: {report:?}");
            if hits == 1 {
                let premises: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT fd.premise_fact_id FROM fact_derivations fd JOIN derived_facts d ON d.id=fd.derived_fact_id WHERE d.kb_id=$1 AND d.invalidated_at IS NULL AND fd.premise_fact_id IS NOT NULL")
                    .bind(kb).fetch_all(&pool).await?;
                anyhow::ensure!(facts.iter().all(|f| premises.contains(f)));
            }
        }
        let nested = json!({"op":"div","l":{"attr":input},"r":{"op":"div","l":{"attr":output},"r":{"const":"2"}}});
        let conditions = json!([{"predicate_id":input,"op":"gt","operand":nested,"group":2},{"predicate_id":output,"op":"present","group":7}]);
        anyhow::ensure!(call("PATCH",path.clone(),json!({"conditions":conditions})).await?.0.is_success());
        let (_, before) = call("GET",base.clone(),json!(null)).await?;
        anyhow::ensure!(before["rules"][0]["conditions"][0]["operand"] == nested);
        anyhow::ensure!(before["rules"][0]["conditions"][1]["group"] == 7);
        // A bad second condition must not commit the name or first condition.
        for bad in [json!({"attr":Uuid::now_v7()}),json!({"op":"pow","l":{"attr":input},"r":{"const":2}}),json!({"const":"NaN"}),json!({"op":"add","l":{"attr":input}})] {
            let patch = json!({"name":"Must not commit", "conditions":[{"predicate_id":input,"op":"gt","operand":5,"group":2},{"predicate_id":input,"op":"gt","operand":bad,"group":7}]});
            anyhow::ensure!(call("PATCH",path.clone(),patch).await?.0 == StatusCode::UNPROCESSABLE_ENTITY);
            anyhow::ensure!(call("GET",base.clone(),json!(null)).await?.1 == before);
        }
        // Existing scalar, set, range, presence contracts stay separate.
        for (op, operand) in [("gt",json!(5)),("gte",json!("5")),("lt",json!(500)),("lte",json!("500")),("between",json!([0,100])),("in",json!([100])),("not_in",json!([0])),("present",Value::Null)] {
            let c = json!([{"predicate_id":input,"op":op,"operand":operand}]);
            anyhow::ensure!(call("PATCH",path.clone(),json!({"conditions":c})).await?.0.is_success(), "scalar {op}");
            if ["between","in","not_in","present"].contains(&op) {
                let c=json!([{"predicate_id":input,"op":op,"operand":expr}]);
                anyhow::ensure!(call("PATCH",path.clone(),json!({"conditions":c})).await?.0 == StatusCode::UNPROCESSABLE_ENTITY);
            }
        }
        let mut deep=json!({"attr":input});
        for _ in 0..utopia_reason::rules::MAX_EXPR_DEPTH { deep=json!({"op":"sub","l":deep,"r":{"const":1}}); }
        let c=json!([{"predicate_id":input,"op":"gt","operand":deep}]);
        anyhow::ensure!(call("PATCH",path.clone(),json!({"conditions":c})).await?.0.is_success());
        anyhow::ensure!(utopia_store::reasoning::materialize(&pool,kb).await?.rule_hits == 1);
        deep=json!({"op":"sub","l":deep,"r":{"const":1}});
        let c=json!([{"predicate_id":input,"op":"gt","operand":deep}]);
        let (status,error)=call("PATCH",path.clone(),json!({"conditions":c})).await?;
        anyhow::ensure!(status == StatusCode::UNPROCESSABLE_ENTITY && error["code"] == "expression_too_deep");
        sqlx::query("UPDATE kb_members SET role='viewer' WHERE user_id=$1").bind(user).execute(&pool).await?;
        anyhow::ensure!(call("POST",base.clone(),definition).await?.0 == StatusCode::FORBIDDEN);
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
