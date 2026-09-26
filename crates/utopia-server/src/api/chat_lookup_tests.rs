//! A name reaches the entity called exactly that, even behind busier partial matches.
//!
//! The graph tools read the first eight entities whose name contains the one asked for.
//! Those used to be the eight with the most facts, so "Apple" beside nine busier
//! "Apple Store …" entities never reached the one named Apple: `find_entities` listed the
//! stores, and `entity_facts("Apple")` read Apple Store 1 and handed its facts over as
//! Apple's.
use super::*;

/// The newest tool result the scripted endpoint was shown in one request
fn tool_result(request: &serde_json::Value) -> String {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|m| m["role"] == "tool")
        .and_then(|m| m["content"].as_str())
        .unwrap_or_default()
        .to_string()
}

async fn entity(f: &Fx, ty: Uuid, name: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO entities(id,kb_id,type_id,canonical_name) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(f.kb)
        .bind(ty)
        .bind(name)
        .execute(&f.pool)
        .await?;
    Ok(id)
}

#[tokio::test]
async fn a_name_reaches_the_entity_called_exactly_that() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("find_entities", r#"{"name":"Apple"}"#),
        Reply::Tool("entity_facts", r#"{"entity_id":"Apple"}"#),
        Reply::Text("Apple has no recorded facts."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let ty = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,'organization','Organization')",
    )
    .bind(ty)
    .bind(f.kb)
    .execute(&f.pool)
    .await?;
    let located = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types(id,kb_id,key,label,kind)
         VALUES ($1,$2,'located_in','located in','relation')",
    )
    .bind(located)
    .bind(f.kb)
    .execute(&f.pool)
    .await?;
    let city = entity(&f, ty, "Cupertino").await?;
    // The entity named Apple has no facts; each of the nine stores has one
    let apple = entity(&f, ty, "Apple").await?;
    for i in 1..=9 {
        let store = entity(&f, ty, &format!("Apple Store {i}")).await?;
        sqlx::query(
            "INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,recorded_at)
             VALUES ($1,$2,$3,$4,$5,now())",
        )
        .bind(Uuid::now_v7())
        .bind(f.kb)
        .bind(store)
        .bind(located)
        .bind(city)
        .execute(&f.pool)
        .await?;
    }

    let sse = f.ask("Tell me about Apple.").await?;
    assert!(sse.contains("event: done"), "{sse}");
    let requests = f.requests();
    let found = tool_result(&requests[1]);
    assert!(
        found.starts_with(&format!("Best match: {apple} | Apple | ")),
        "find_entities must list the entity named Apple first:\n{found}"
    );
    let facts = tool_result(&requests[2]);
    assert!(facts.contains("\"Apple\" = Apple ("), "{facts}");
    assert!(facts.contains("Apple: no recorded facts."), "{facts}");
    assert!(
        !facts.contains("Cupertino"),
        "a store's facts must not be read as Apple's:\n{facts}"
    );
    f.cleanup().await
}
