//! What a turn records as "already identified". The next turn is told to use those ids
//! directly and not to look the names up again, so the record must hold the entities the
//! turn read, or the one a search clearly pointed at, and never the unchosen candidates of
//! an ambiguous name.
use super::*;

/// A tool call's arguments, built at run time: `Reply::Tool` takes static text
fn args(value: serde_json::Value) -> &'static str {
    Box::leak(value.to_string().into_boxed_str())
}

async fn entity(f: &Fx, id: Uuid, ty: Uuid, name: &str) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO entities(id,kb_id,type_id,canonical_name) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(f.kb)
        .bind(ty)
        .bind(name)
        .execute(&f.pool)
        .await?;
    Ok(())
}

async fn entity_type(f: &Fx, key: &str, label: &str) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,$3,$4)")
        .bind(id)
        .bind(f.kb)
        .bind(key)
        .bind(label)
        .execute(&f.pool)
        .await?;
    Ok(id)
}

/// The ids each stored answer recorded, oldest answer first
async fn recorded(f: &Fx) -> anyhow::Result<Vec<Vec<String>>> {
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT m.resolved FROM conversation_messages m
           JOIN conversations c ON c.id = m.conversation_id
          WHERE c.kb_id = $1 AND m.role = 'assistant'
          ORDER BY m.created_at",
    )
    .bind(f.kb)
    .fetch_all(&f.pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| {
            r.as_array()
                .into_iter()
                .flatten()
                .filter_map(|e| e["id"].as_str().map(str::to_string))
                .collect()
        })
        .collect())
}

#[tokio::test]
async fn a_turn_remembers_the_entity_it_read_not_every_candidate() -> anyhow::Result<()> {
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("find_entities", r#"{"name":"Zhang Wei"}"#),
        Reply::Tool("entity_facts", args(json!({ "entity_id": second }))),
        Reply::Text("The Zhang Wei who joined in 2021."),
        Reply::Tool("no_evidence_needed", r#"{"reason":"follow-up"}"#),
        Reply::Text("Follow-up."),
    ]))
    .await?
    else {
        return Ok(());
    };
    let person = entity_type(&f, "person", "Person").await?;
    entity(&f, first, person, "Zhang Wei").await?;
    entity(&f, second, person, "Zhang Wei").await?;

    let sse = f.ask("Which Zhang Wei joined in 2021?").await?;
    assert!(sse.contains("event: done"), "{sse}");
    assert_eq!(
        recorded(&f).await?,
        vec![vec![second.to_string()]],
        "the search's candidates are not identified; the one read by id is"
    );

    let id: Uuid = sqlx::query_scalar("SELECT id FROM conversations WHERE kb_id = $1")
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
    let follow = chat(
        State(f.state.clone()),
        AuthUser(f.user.clone()),
        Path(f.kb),
        Json(ChatReq {
            conversation_id: Some(id),
            message: "When did he join?".into(),
            retry_message_id: None,
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("chat handler refused the follow-up"))?;
    let _ = axum::body::to_bytes(follow.into_response().into_body(), 4 * 1024 * 1024).await?;
    let requests = f.requests();
    let told = requests[3]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|m| m["content"].as_str())
        .find(|c| c.starts_with("Entities already identified"))
        .expect("the next turn is told which entity was identified")
        .to_string();
    assert!(told.contains(&second.to_string()), "{told}");
    assert!(
        !told.contains(&first.to_string()),
        "the unchosen namesake must not be handed to the next turn: {told}"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_search_remembers_only_a_clear_best_match() -> anyhow::Result<()> {
    let Some(f) = fixture(Scripted::new(vec![
        Reply::Tool("find_entities", r#"{"name":"Acme"}"#),
        Reply::Text("Acme is an organization."),
        Reply::Tool("find_entities", r#"{"name":"Zhang Wei"}"#),
        Reply::Text("Which Zhang Wei do you mean?"),
    ]))
    .await?
    else {
        return Ok(());
    };
    let org = entity_type(&f, "organization", "Organization").await?;
    let person = entity_type(&f, "person", "Person").await?;
    let acme = Uuid::now_v7();
    entity(&f, acme, org, "Acme").await?;
    entity(&f, Uuid::now_v7(), org, "Acme Labs").await?;
    entity(&f, Uuid::now_v7(), person, "Zhang Wei").await?;
    entity(&f, Uuid::now_v7(), person, "Zhang Wei").await?;

    // The exact name is the best match the search reports, so it is the one identified
    let sse = f.ask("What is Acme?").await?;
    assert!(sse.contains("event: done"), "{sse}");
    // Two namesakes and no choice made: nothing is identified yet
    let sse = f.ask("Tell me about Zhang Wei.").await?;
    assert!(sse.contains("event: done"), "{sse}");
    assert_eq!(recorded(&f).await?, vec![vec![acme.to_string()], vec![]]);
    f.cleanup().await
}
