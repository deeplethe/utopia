//! #550: read IDs through the actual authenticated MCP handler, against PostgreSQL.
use super::*;
use std::sync::Arc;
use tools::{ToolCtx, ToolResult};

const CORRECTION: &str = "2026-03-20T12:00:00.123456Z";

struct Fixture {
    state: AppState,
    org: Uuid,
    ws: Uuid,
    kb: Uuid,
    other_kb: Uuid,
    subject: Uuid,
    object: Uuid,
    fact: Uuid,
    corrected: Uuid,
    attribute: Uuid,
    derived: Uuid,
    derived_value: Uuid,
    document: Uuid,
    chunk: Uuid,
    token: String,
    dir: std::path::PathBuf,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let dir = std::env::temp_dir().join(format!("utopia-mcp-{}", Uuid::now_v7()));
        let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
        let config = utopia_core::config::AppConfig {
            data_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut f = Self {
            state: AppState::new(pool.clone(), &config, search, "test-only".into()),
            org: Uuid::now_v7(),
            ws: Uuid::now_v7(),
            kb: Uuid::now_v7(),
            other_kb: Uuid::now_v7(),
            subject: Uuid::now_v7(),
            object: Uuid::now_v7(),
            fact: Uuid::now_v7(),
            corrected: Uuid::now_v7(),
            attribute: Uuid::now_v7(),
            derived: Uuid::now_v7(),
            derived_value: Uuid::now_v7(),
            document: Uuid::now_v7(),
            chunk: Uuid::now_v7(),
            token: String::new(),
            dir,
        };
        let (user, ty, relation, attr, rule, business, chunk2) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // Interpolation is limited to locally generated UUIDs and the fixed timestamp.
        sqlx::raw_sql(&format!(
            r#"
            INSERT INTO organizations(id,name) VALUES ('{org}','mcp-test');
            INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','mcp-test');
            INSERT INTO users(id,org_id,email,password_hash,display_name)
                VALUES ('{user}','{org}','{user}@example.test','unused','MCP reader');
            INSERT INTO knowledge_bases(id,workspace_id,name) VALUES
                ('{kb}','{ws}','mcp-test'), ('{other_kb}','{ws}','other-base');
            INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{user}','viewer');
            INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{ty}','{kb}','thing','Thing');
            INSERT INTO relation_types(id,kb_id,key,label,kind,datatype) VALUES
                ('{relation}','{kb}','works_for','works for','relation',NULL),
                ('{attr}','{kb}','weight','weight','attribute','number');
            INSERT INTO entities(id,kb_id,type_id,canonical_name,created_at) VALUES
                ('{subject}','{kb}','{ty}','Alice','2026-01-01'),
                ('{object}','{kb}','{ty}','Acme','2026-01-01');
            INSERT INTO documents(id,kb_id,filename,sha256,created_at)
                VALUES ('{document}','{kb}','orchard.md',repeat('0',64),'2026-01-01');
            INSERT INTO chunks(id,kb_id,document_id,seq,text,created_at) VALUES
                ('{chunk}','{kb}','{document}',0,repeat('orchard ',120),'2026-01-01'),
                ('{chunk2}','{kb}','{document}',1,'Alice works for Acme.','2026-01-01');
            INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,valid_from,
                valid_from_precision,recorded_at,invalidated_at) VALUES
                ('{fact}','{kb}','{subject}','{relation}','{object}','2026-01-01',
                 'day','2026-03-10','{correction}');
            INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,valid_from,
                valid_from_precision,recorded_at,supersedes) VALUES
                ('{corrected}','{kb}','{subject}','{relation}','{object}','2026-02-01',
                 'day','{correction}','{fact}');
            INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_value,valid_from,
                valid_from_precision,recorded_at) VALUES
                ('{attribute}','{kb}','{subject}','{attr}','{{"value":7,"unit":"kg"}}',
                 '2026-01-01','year','2026-03-10');
            INSERT INTO fact_evidence(fact_id,chunk_id,document_id,doc_version,quote) VALUES
                ('{fact}','{chunk}','{document}',1,'Alice works for Acme.'),
                ('{corrected}','{chunk}','{document}',1,'Alice works for Acme.'),
                ('{corrected}','{chunk2}','{document}',1,'Alice works for Acme.');
            INSERT INTO fact_qualifiers(fact_id,qualifier_type_id,value)
                VALUES ('{corrected}','{attr}','{{"value":3,"unit":"kg"}}');
            INSERT INTO rules(id,kb_id,predicate_id,kind)
                VALUES ('{rule}','{kb}','{relation}','symmetric');
            INSERT INTO derived_facts(id,kb_id,subject_id,predicate_id,object_id,rule_id,
                derived_at,invalidated_at,valid_from,valid_from_precision) VALUES
                ('{derived}','{kb}','{object}','{relation}','{subject}','{rule}',
                 '2026-03-15','2026-04-01','2026-01-01','day');
            INSERT INTO fact_derivations(derived_fact_id,premise_fact_id,seq)
                VALUES ('{derived}','{fact}',0);
            INSERT INTO attribute_rules(id,kb_id,name,subject_type_id,conclusion,
                conclude_predicate_id,conclude_value) VALUES
                ('{business}','{kb}','Weight rule','{ty}','attribute','{attr}',
                 '{{"value":8,"unit":"kg"}}');
            INSERT INTO derived_facts(id,kb_id,subject_id,predicate_id,object_value,
                attribute_rule_id,derived_at,valid_from,valid_from_precision) VALUES
                ('{derived_value}','{kb}','{subject}','{attr}','{{"value":8,"unit":"kg"}}',
                 '{business}','2026-03-15','2026-01-01','day');
            INSERT INTO fact_derivations(derived_fact_id,premise_fact_id,seq)
                VALUES ('{derived_value}','{attribute}',0);
        "#,
            org = f.org,
            ws = f.ws,
            kb = f.kb,
            other_kb = f.other_kb,
            subject = f.subject,
            object = f.object,
            document = f.document,
            chunk = f.chunk,
            fact = f.fact,
            corrected = f.corrected,
            attribute = f.attribute,
            derived = f.derived,
            derived_value = f.derived_value,
            correction = CORRECTION
        ))
        .execute(&pool)
        .await?;
        f.token = utopia_store::tokens::issue(&pool, user, "MCP test", "read", Some(&[f.kb]), None)
            .await?
            .1;
        f.state.search.reindex_document(
            &f.kb.to_string(),
            &f.document.to_string(),
            &[(f.chunk.to_string(), "orchard ".repeat(120))],
        )?;
        Ok(Some(f))
    }

    async fn request(
        &self,
        kb: Uuid,
        method: &str,
        params: Value,
    ) -> crate::error::ApiResult<Json<Value>> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", self.token).parse().unwrap(),
        );
        handle(
            State(self.state.clone()),
            Path(kb),
            headers,
            Json(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params})),
        )
        .await
    }

    async fn call(&self, name: &str, args: Value) -> anyhow::Result<Value> {
        let response = self
            .request(self.kb, "tools/call", json!({"name":name,"arguments":args}))
            .await
            .map_err(|_| anyhow::anyhow!("MCP request failed"))?
            .0;
        assert!(response.get("error").is_none(), "{response}");
        Ok(response["result"].clone())
    }

    async fn clean(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.state.pool)
            .await?;
        let dir = self.dir.clone();
        drop(self);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}

fn uuid(value: &Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}

#[tokio::test]
async fn refused_and_executed_calls_are_each_audited_once() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
    for (args, is_error, count) in [(json!({}), true, 1), (json!({"query":"orchard"}), false, 2)] {
        let result = f.call("search_chunks", args).await?;
        assert_eq!(result["isError"], is_error);
        if is_error {
            assert!(result.get("structuredContent").is_none());
        }
        let rows: Vec<(Uuid, Uuid, String, Uuid, Value)> = sqlx::query_as(
            "SELECT kb_id, actor_id, target_kind, target_id, detail FROM audit_events
             WHERE kb_id=$1 AND action='mcp.tool_called'",
        )
        .bind(f.kb)
        .fetch_all(&f.state.pool)
        .await?;
        assert_eq!(rows.len(), count);
        for row in rows {
            assert_eq!(
                row,
                (
                    f.kb,
                    auth.user_id,
                    "personal_token".into(),
                    auth.token_id,
                    json!({"tool":"search_chunks"}),
                )
            );
        }
    }
    f.clean().await
}

#[tokio::test]
async fn find_entities_returns_ranked_ids_and_keeps_text() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = f.call("find_entities", json!({"name":"Alice"})).await?;
    assert_eq!(result["isError"], false);
    assert_eq!(
        uuid(&result["structuredContent"]["entities"][0]["id"]),
        f.subject
    );
    assert_eq!(
        result["structuredContent"]["entities"][0]["type_key"],
        "thing"
    );
    assert_eq!(
        result["content"][0]["text"],
        format!("Best match: {} | Alice | Thing | 2 facts", f.subject)
    );
    let empty = f.call("find_entities", json!({"name":"Nobody"})).await?;
    assert_eq!(empty["isError"], false);
    assert_eq!(empty["structuredContent"]["entities"], json!([]));
    assert_eq!(empty["content"][0]["text"], "No matching entities.");
    let invalid = f.call("find_entities", json!({})).await?;
    assert_eq!(invalid["isError"], true);
    assert!(invalid.get("structuredContent").is_none());
    assert!(f
        .request(
            f.other_kb,
            "tools/call",
            json!({"name":"find_entities","arguments":{"name":"Alice"}})
        )
        .await
        .is_err());
    let listed = f
        .request(f.kb, "tools/list", json!({}))
        .await
        .map_err(|e| e.0)?
        .0;
    assert!(!listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "remember"));
    f.clean().await
}

#[tokio::test]
async fn search_chunks_returns_chunk_and_document_ids_with_the_same_excerpt() -> anyhow::Result<()>
{
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = f.call("search_chunks", json!({"query":"orchard"})).await?;
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["limit"], 6);
    assert_eq!(result["structuredContent"]["limit_reached"], false);
    let chunk = &result["structuredContent"]["chunks"][0];
    assert_eq!(uuid(&chunk["chunk_id"]), f.chunk);
    assert_eq!(uuid(&chunk["document_id"]), f.document);
    assert_eq!(chunk["seq"], 0);
    assert_eq!(chunk["truncated"], true);
    assert_eq!(
        result["content"][0]["text"],
        format!(
            "[1] \"orchard.md\" section 1 (document_id: {}):\n{}",
            f.document,
            chunk["text"].as_str().unwrap()
        )
    );
    let empty = f
        .call(
            "search_chunks",
            json!({"query":"orchard","as_of":"2025-01-01"}),
        )
        .await?;
    assert_eq!(empty["structuredContent"]["chunks"], json!([]));
    assert_eq!(empty["isError"], false);
    assert_eq!(empty["structuredContent"]["limit_reached"], false);
    assert_eq!(empty["content"][0]["text"], "No results.");
    let mut indexed = vec![(f.chunk.to_string(), "orchard ".repeat(120))];
    for seq in 2..8 {
        let id = Uuid::now_v7();
        let text = format!("orchard section {seq}");
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ($1,$2,$3,$4,$5)")
            .bind(id)
            .bind(f.kb)
            .bind(f.document)
            .bind(seq)
            .bind(&text)
            .execute(&f.state.pool)
            .await?;
        indexed.push((id.to_string(), text));
    }
    f.state
        .search
        .reindex_document(&f.kb.to_string(), &f.document.to_string(), &indexed)?;
    let capped = f.call("search_chunks", json!({"query":"orchard"})).await?;
    assert_eq!(capped["isError"], false);
    assert_eq!(capped["structuredContent"]["limit"], 6);
    assert_eq!(capped["structuredContent"]["limit_reached"], true);
    let chunks = capped["structuredContent"]["chunks"].as_array().unwrap();
    assert_eq!(chunks.len(), 6);
    let ids: std::collections::HashSet<Uuid> =
        chunks.iter().map(|c| uuid(&c["chunk_id"])).collect();
    assert_eq!(ids.len(), 6);
    for chunk in chunks {
        assert_eq!(uuid(&chunk["document_id"]), f.document);
        assert!(indexed.iter().any(|(id, _)| chunk["chunk_id"] == *id));
    }
    f.clean().await
}

#[tokio::test]
async fn entity_facts_keeps_identity_values_filters_and_both_clocks() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let broker = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types(id,kb_id,key,label,kind)
         VALUES ($1,$2,'broker','Broker','relation')",
    )
    .bind(broker)
    .bind(f.kb)
    .execute(&f.state.pool)
    .await?;
    sqlx::query(
        "INSERT INTO fact_qualifiers(fact_id,qualifier_type_id,entity_id)
         VALUES ($1,$2,$3)",
    )
    .bind(f.corrected)
    .bind(broker)
    .bind(f.object)
    .execute(&f.state.pool)
    .await?;
    let result = f
        .call("entity_facts", json!({"entity_id":f.subject}))
        .await?;
    let data = &result["structuredContent"];
    assert_eq!(uuid(&data["entity"]["id"]), f.subject);
    let facts = data["facts"].as_array().unwrap();
    let corrected = facts
        .iter()
        .find(|r| uuid(&r["id"]) == f.corrected)
        .unwrap();
    assert_eq!(corrected["recorded_at"], CORRECTION);
    let qualifiers = corrected["qualifiers"].as_array().unwrap();
    let weight = qualifiers.iter().find(|q| q["key"] == "weight").unwrap();
    assert_eq!(weight["value"], json!({"value":3,"unit":"kg"}));
    assert!(weight["entity_id"].is_null());
    assert!(weight["entity_name"].is_null());
    assert_eq!(
        qualifiers.iter().find(|q| q["key"] == "broker").unwrap(),
        &json!({
            "qualifier_type_id":broker,"key":"broker","label":"Broker",
            "value":null,"entity_id":f.object,"entity_name":"Acme",
        })
    );
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("broker: Acme"));
    assert_eq!(uuid(&corrected["supersedes"]), f.fact);
    assert_eq!(
        corrected["document_ids"],
        json!([f.document]),
        "same source is deduplicated"
    );
    let attribute = facts
        .iter()
        .find(|r| uuid(&r["id"]) == f.attribute)
        .unwrap();
    assert_eq!(attribute["object_value"], json!({"value":7,"unit":"kg"}));
    assert_eq!(attribute["valid_from_precision"], "year");
    let derived = &data["derived_facts"][0];
    assert_eq!(uuid(&derived["id"]), f.derived_value);
    assert_eq!(derived["object_value"], json!({"value":8,"unit":"kg"}));
    assert_eq!(derived["rule"], "business");
    assert!(derived["rule_id"].is_null());
    uuid(&derived["attribute_rule_id"]);
    // The same UUID is the RDF statement's identity, not a newly minted response ID.
    let exported = utopia_store::export::facts_page(&f.state.pool, f.kb, None).await?;
    assert!(exported
        .iter()
        .any(|r| r.id == uuid(&corrected["id"]) && r.documents == vec![f.document]));
    let incoming = f
        .call("entity_facts", json!({"entity_id":f.object}))
        .await?;
    assert_eq!(incoming["structuredContent"]["facts"][0]["direction"], "in");
    assert_eq!(
        uuid(&incoming["structuredContent"]["facts"][0]["other_id"]),
        f.subject
    );
    let limited = f
        .call("entity_facts", json!({"entity_id":f.subject,"limit":1}))
        .await?;
    assert_eq!(
        limited["structuredContent"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(limited["structuredContent"]["truncated"], true);
    let filtered = f
        .call(
            "entity_facts",
            json!({"entity_id":f.subject,"predicate":"weight"}),
        )
        .await?;
    assert_eq!(
        filtered["structuredContent"]["facts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        uuid(&filtered["structuredContent"]["facts"][0]["id"]),
        f.attribute
    );
    let empty = f
        .call(
            "entity_facts",
            json!({"entity_id":f.subject,"at":"2025-01-01","as_of":CORRECTION}),
        )
        .await?;
    assert_eq!(empty["structuredContent"]["facts"], json!([]));
    assert_eq!(empty["structuredContent"]["derived_facts"], json!([]));
    assert_eq!(empty["isError"], false);
    let history = f
        .call(
            "entity_facts",
            json!({"entity_id":f.subject,"before":CORRECTION,"as_of":"2026-05-01"}),
        )
        .await?;
    let old = history["structuredContent"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| uuid(&r["id"]) == f.fact)
        .unwrap();
    assert_eq!(old["invalidated_at"], CORRECTION);
    assert_eq!(
        history["structuredContent"]["as_of"],
        "2026-03-20T12:00:00.123455Z"
    );
    assert!(history["structuredContent"]["derived_facts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| uuid(&r["id"]) == f.derived));
    let early = f
        .call(
            "entity_facts",
            json!({"entity_id":f.subject,"as_of":"2026-03-12"}),
        )
        .await?;
    assert_eq!(early["structuredContent"]["derived_facts"], json!([]));
    // Supplying a foreign entity UUID must not reveal its name or facts.
    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Hidden')")
        .bind(f.other_kb)
        .bind(f.other_kb)
        .execute(&f.state.pool)
        .await?;
    let foreign = f
        .call("entity_facts", json!({"entity_id":f.other_kb}))
        .await?;
    assert_eq!(foreign["isError"], true);
    assert_eq!(foreign["content"][0]["text"], "Entity not found.");
    assert!(foreign.get("structuredContent").is_none());
    f.clean().await
}

#[tokio::test]
async fn changes_returns_fact_ids_and_a_reusable_correction_timestamp() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = f
        .call(
            "changes",
            json!({"since":"2026-03-20","until":"2026-03-20","kinds":["corrected"]}),
        )
        .await?;
    let change = &result["structuredContent"]["changes"][0];
    assert_eq!(uuid(&change["fact_id"]), f.corrected);
    assert_eq!(uuid(&change["document_id"]), f.document);
    assert_eq!(change["at"], CORRECTION);
    assert_eq!(result["structuredContent"]["until"], "2026-03-21T00:00:00Z");
    assert!(result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains(CORRECTION));
    let before = f
        .call(
            "entity_facts",
            json!({"entity_id":f.object,"before":change["at"]}),
        )
        .await?;
    assert_eq!(uuid(&before["structuredContent"]["facts"][0]["id"]), f.fact);
    let empty = f
        .call("changes", json!({"since":"2025","until":"2025"}))
        .await?;
    assert_eq!(empty["structuredContent"]["changes"], json!([]));
    assert_eq!(empty["structuredContent"]["limit_reached"], false);
    assert_eq!(empty["isError"], false);
    let started = chrono::Utc::now();
    let open = f.call("changes", json!({"since":"2026-03-20"})).await?;
    let finished = chrono::Utc::now();
    let data = &open["structuredContent"];
    let until: chrono::DateTime<chrono::Utc> = data["until"].as_str().unwrap().parse()?;
    let since: chrono::DateTime<chrono::Utc> = data["since"].as_str().unwrap().parse()?;
    assert_eq!(open["isError"], false);
    assert_eq!(data["since"], "2026-03-20T00:00:00Z");
    assert!(started <= until && until <= finished);
    assert_eq!(data["limit_reached"], false);
    let changes = data["changes"].as_array().unwrap();
    assert!(changes
        .iter()
        .any(|c| uuid(&c["fact_id"]) == f.corrected && c["at"] == CORRECTION));
    for change in changes {
        let at: chrono::DateTime<chrono::Utc> = change["at"].as_str().unwrap().parse()?;
        assert!(since <= at && at < until);
    }
    sqlx::query(
        "INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_value,recorded_at)
                 SELECT gen_random_uuid(),kb_id,subject_id,predicate_id,
                        jsonb_build_object('value',n),'2026-03-21'::timestamptz
                 FROM facts CROSS JOIN generate_series(1,41) n WHERE id=$1",
    )
    .bind(f.attribute)
    .execute(&f.state.pool)
    .await?;
    let capped = f
        .call(
            "changes",
            json!({"since":"2026-03-21","until":"2026-03-21"}),
        )
        .await?;
    assert_eq!(
        capped["structuredContent"]["changes"]
            .as_array()
            .unwrap()
            .len(),
        40
    );
    assert_eq!(capped["structuredContent"]["limit_reached"], true);
    f.clean().await
}

#[tokio::test]
async fn missing_entities_and_empty_graph_reads_keep_their_results() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let isolated = Uuid::now_v7();
    sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Isolated')")
        .bind(isolated)
        .bind(f.kb)
        .execute(&f.state.pool)
        .await?;
    for (name, args, is_error, text) in [
        (
            "entity_facts",
            json!({"entity_id":"Nobody"}),
            true,
            "Invalid entity: no entity named \"Nobody\" in this base (expected a name, or the uuid returned by find_entities).",
        ),
        (
            "neighbors",
            json!({"entity":"Nobody"}),
            false,
            "Unknown entity: no entity named \"Nobody\" in this base.",
        ),
        (
            "timeline",
            json!({"entity":"Nobody"}),
            false,
            "Unknown entity: no entity named \"Nobody\" in this base.",
        ),
        (
            "paths_between",
            json!({"from":"Nobody","to":f.object}),
            false,
            "Unknown `from`: no entity named \"Nobody\" in this base.",
        ),
        (
            "paths_between",
            json!({"from":f.subject,"to":"Nobody"}),
            false,
            "Unknown `to`: no entity named \"Nobody\" in this base.",
        ),
        ("neighbors", json!({"entity":f.other_kb}), false, "Entity not found."),
        ("timeline", json!({"entity":f.other_kb}), false, "Entity not found."),
        (
            "neighbors",
            json!({"entity":isolated}),
            false,
            "Isolated (untyped): no linked entities.",
        ),
        (
            "timeline",
            json!({"entity":isolated}),
            false,
            "Isolated (untyped): no dated facts; 0 facts carry no date (entity_facts lists them).",
        ),
    ] {
        let result = f.call(name, args).await?;
        assert_eq!(result["isError"], is_error, "{name}: {result}");
        assert_eq!(result["content"][0]["text"], text, "{name}");
        assert!(result.get("structuredContent").is_none());
    }
    let empty = f
        .call("paths_between", json!({"from":f.subject,"to":isolated}))
        .await?;
    assert_eq!(empty["isError"], false);
    assert!(empty["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("No path of up to 3 hops between "));
    assert!(empty.get("structuredContent").is_none());
    f.clean().await
}

#[tokio::test]
async fn failed_reads_do_not_become_successful_empty_results() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    f.state.pool.close().await;
    let ctx = ToolCtx {
        state: &f.state,
        kb_id: f.kb,
        workspace_id: f.ws,
        mounted_sources: &[],
        can_write: false,
        actor: None,
        via_token: None,
        question: None,
    };
    for (name, args, text) in [
        ("list_rules", json!({}), "Could not read the rules."),
        (
            "rule_matches",
            json!({"rule_id":Uuid::now_v7()}),
            "Could not read what that rule marks.",
        ),
        (
            "get_document",
            json!({"document_id":f.document}),
            "Could not read the document.",
        ),
        (
            "find_entities",
            json!({"name":"Alice"}),
            "Could not look up entities.",
        ),
        (
            "search_chunks",
            json!({"query":"orchard"}),
            "Could not search the documents.",
        ),
        (
            "entity_facts",
            json!({"entity_id":f.subject}),
            "Could not read the entity facts.",
        ),
        (
            "entity_facts",
            json!({"entity_id":"Alice"}),
            "Could not look up entities.",
        ),
        (
            "neighbors",
            json!({"entity":"Alice"}),
            "Could not look up entities.",
        ),
        (
            "timeline",
            json!({"entity":"Alice"}),
            "Could not look up entities.",
        ),
        (
            "neighbors",
            json!({"entity":f.subject}),
            "Could not read the entity facts.",
        ),
        (
            "timeline",
            json!({"entity":f.subject}),
            "Could not read the entity facts.",
        ),
        (
            "paths_between",
            json!({"from":"Alice","to":f.object}),
            "Could not look up entities.",
        ),
        (
            "paths_between",
            json!({"from":f.subject,"to":"Acme"}),
            "Could not look up entities.",
        ),
        // Equal UUIDs return before reading the database; these must be different.
        (
            "paths_between",
            json!({"from":f.subject,"to":f.object}),
            "Could not search paths.",
        ),
        (
            "changes",
            json!({"since":"2026"}),
            "Could not read the graph changes.",
        ),
    ] {
        let result =
            tool_result(tools::dispatch(&ctx, &mut ToolSink::default(), name, &args).await);
        assert_eq!(result["isError"], true, "{name}: {result}");
        assert_eq!(result["content"][0]["text"], text, "{name}: {args}");
        assert!(result.get("structuredContent").is_none());
    }
    // Reconnect only for fixture cleanup.
    let mut f = f;
    f.state.pool = sqlx::PgPool::connect(&utopia_store::test_db::url().unwrap()).await?;
    f.clean().await
}

#[tokio::test]
async fn document_reads_preserve_text_empty_and_unavailable_results() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let read = f
        .call("get_document", json!({"document_id":f.document}))
        .await?;
    assert_eq!(read["isError"], false);
    let text = read["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("orchard.md"));
    assert!(text.contains("2 section(s)"));
    assert!(text.contains("Alice works for Acme."));
    assert!(read.get("structuredContent").is_none());

    let foreign = Uuid::now_v7();
    let empty = Uuid::now_v7();
    for (id, kb) in [(foreign, f.other_kb), (empty, f.kb)] {
        sqlx::query(
            "INSERT INTO documents(id,kb_id,filename,sha256) VALUES ($1,$2,'empty.md',repeat('1',64))",
        )
        .bind(id)
        .bind(kb)
        .execute(&f.state.pool)
        .await?;
    }
    let read = f.call("get_document", json!({"document_id":empty})).await?;
    assert_eq!(read["isError"], false);
    assert!(read["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("0 section(s):\n(no text)"));

    sqlx::query("UPDATE documents SET deleted_at=now() WHERE id=$1")
        .bind(f.document)
        .execute(&f.state.pool)
        .await?;
    // Missing, foreign and deleted IDs remain indistinguishable; a read failure
    // must not change that boundary or turn a genuinely empty document into an error.
    for id in [Uuid::now_v7(), foreign, f.document] {
        let result = f.call("get_document", json!({"document_id":id})).await?;
        assert_eq!(result["isError"], false);
        assert_eq!(
            result["content"][0]["text"],
            "No document with that id in this knowledge base."
        );
        assert!(result.get("structuredContent").is_none());
    }
    f.clean().await
}

#[tokio::test]
async fn failed_document_chunks_do_not_become_a_successful_empty_document() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let schema = format!("mcp_chunks_failure_{}", Uuid::now_v7().simple());
    // Shadow chunks only for this connection: the document lookup succeeds but
    // its subsequent chunk query fails, without altering tables used by other tests.
    sqlx::raw_sql(&format!(
        "CREATE SCHEMA {schema}; CREATE VIEW {schema}.chunks AS SELECT NULL::uuid AS id;"
    ))
    .execute(&f.state.pool)
    .await?;
    let options: sqlx::postgres::PgConnectOptions =
        utopia_store::test_db::url().unwrap().parse()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.options([("search_path", format!("{schema},public"))]))
        .await?;
    assert!(utopia_store::documents::find_in_kb(&pool, f.kb, f.document)
        .await?
        .is_some());
    let mut state = f.state.clone();
    state.pool = pool.clone();
    let ctx = ToolCtx {
        state: &state,
        kb_id: f.kb,
        workspace_id: f.ws,
        mounted_sources: &[],
        can_write: false,
        actor: None,
        via_token: None,
        question: None,
    };
    let mut sink = ToolSink::default();
    let result = tool_result(
        tools::dispatch(
            &ctx,
            &mut sink,
            "get_document",
            &json!({"document_id":f.document}),
        )
        .await,
    );
    pool.close().await;
    sqlx::raw_sql(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&f.state.pool)
        .await?;
    f.clean().await?;
    assert_eq!(result["isError"], true);
    assert_eq!(result["content"][0]["text"], "Could not read the document.");
    assert!(result.get("structuredContent").is_none());
    assert!(sink.sources.is_empty());
    Ok(())
}

#[test]
fn text_only_results_do_not_acquire_a_structured_payload() {
    let result = tool_result(ToolResult::new("existing text".into(), json!({})));
    assert_eq!(
        result,
        json!({"content":[{"type":"text","text":"existing text"}],"isError":false})
    );
}

#[tokio::test]
async fn computed_rule_descriptions_keep_the_expression_tree_and_identity() -> anyhow::Result<()> {
    use utopia_store::business_rules::{self, ConditionInput};
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let ty: Uuid = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(f.subject)
        .fetch_one(&f.state.pool)
        .await?;
    let (revenue, cost, margin) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    for (id, key) in [(revenue, "revenue"), (cost, "cost"), (margin, "margin")] {
        sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,kind,datatype) VALUES ($1,$2,$3,$3,'attribute','number')")
            .bind(id).bind(f.kb).bind(key).execute(&f.state.pool).await?;
    }
    let conditions = [ConditionInput {
        group: 2,
        predicate_id: revenue,
        op: "present".into(),
        operand: None,
    }];
    let sub = json!({"op":"sub","l":{"attr":revenue},"r":{"attr":cost}});
    for (name, expr, expected) in [
        ("difference", sub.clone(), "(revenue - cost)"),
        (
            "ratio",
            json!({"op":"div","l":sub,"r":{"attr":revenue}}),
            "((revenue - cost) / revenue)",
        ),
        (
            "nested",
            json!({"op":"sub","l":{"attr":revenue},"r":{"op":"sub","l":{"attr":cost},"r":{"const":2}}}),
            "(revenue - (cost - 2))",
        ),
        (
            "zero",
            json!({"op":"add","l":{"attr":revenue},"r":{"const":0}}),
            "(revenue + 0)",
        ),
        (
            "negative",
            json!({"op":"mul","l":{"attr":revenue},"r":{"const":"-2.5"}}),
            "(revenue * -2.5)",
        ),
    ] {
        business_rules::create(
            &f.state.pool,
            f.kb,
            name,
            "",
            ty,
            "computed",
            None,
            Some(margin),
            None,
            Some(expr),
            &conditions,
        )
        .await?;
        let before = business_rules::list(&f.state.pool, f.kb).await?;
        let derived_before: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY id),'[]') FROM derived_facts d WHERE kb_id=$1")
            .bind(f.kb).fetch_one(&f.state.pool).await?;
        let jobs_before: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY id),'[]') FROM jobs j WHERE payload->>'kb_id'=$1 OR payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$2)")
            .bind(f.kb.to_string()).bind(f.kb).fetch_one(&f.state.pool).await?;
        let result = f.call("list_rules", json!({})).await?;
        assert_eq!(result["isError"], false);
        let text = result["content"][0]["text"].as_str().unwrap();
        let line = text
            .lines()
            .find(|l| l.starts_with(&format!("{name} [")))
            .unwrap();
        assert!(line.contains(&format!("⇒ margin = {expected} ·")), "{line}");
        assert!(text.contains("⇒ weight = {\"unit\":\"kg\",\"value\":8}"));
        assert_eq!(business_rules::list(&f.state.pool, f.kb).await?, before);
        let derived_after: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY id),'[]') FROM derived_facts d WHERE kb_id=$1")
            .bind(f.kb).fetch_one(&f.state.pool).await?;
        assert_eq!(derived_before, derived_after);
        let jobs_after: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY id),'[]') FROM jobs j WHERE payload->>'kb_id'=$1 OR payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$2)")
            .bind(f.kb.to_string()).bind(f.kb).fetch_one(&f.state.pool).await?;
        assert_eq!(jobs_before, jobs_after);
    }
    business_rules::create(
        &f.state.pool,
        f.kb,
        "typing control",
        "",
        ty,
        "typing",
        Some(ty),
        None,
        None,
        None,
        &conditions,
    )
    .await?;
    sqlx::query("UPDATE relation_types SET label='收入' WHERE id=ANY($1)")
        .bind(vec![revenue, cost])
        .execute(&f.state.pool)
        .await?;
    let result = f.call("list_rules", json!({})).await?;
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("(收入 [revenue] - 收入 [cost])"), "{text}");
    assert!(text
        .lines()
        .find(|l| l.starts_with("typing control ["))
        .unwrap()
        .contains("⇒ Thing ·"));
    // Corrupt/stale stored references must not expose another base's label or
    // fabricate a formula. Creation itself continues to reject such inputs.
    let foreign = Uuid::now_v7();
    sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,kind,datatype) VALUES ($1,$2,'hidden','Foreign secret','attribute','number')")
        .bind(foreign).bind(f.other_kb).execute(&f.state.pool).await?;
    for expr in [
        json!({"attr":foreign}),
        json!({"op":"unknown"}),
        json!({"const":null}),
    ] {
        sqlx::query(
            "UPDATE attribute_rules SET conclude_expr=$2 WHERE kb_id=$1 AND name='difference'",
        )
        .bind(f.kb)
        .bind(expr)
        .execute(&f.state.pool)
        .await?;
        let result = f.call("list_rules", json!({})).await?;
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.lines()
                .find(|l| l.starts_with("difference ["))
                .unwrap()
                .contains("margin = (expression unavailable)"),
            "{text}"
        );
        assert!(!text.contains("Foreign secret"));
    }
    f.clean().await
}

#[tokio::test]
async fn rule_reads_preserve_matches_and_empty_results() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let rule: Uuid = sqlx::query_scalar("SELECT id FROM attribute_rules WHERE kb_id=$1")
        .bind(f.kb)
        .fetch_one(&f.state.pool)
        .await?;
    let listed = f.call("list_rules", json!({})).await?;
    assert_eq!(listed["isError"], false);
    assert!(listed["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Weight rule"));
    let matched = f.call("rule_matches", json!({"rule_id":rule})).await?;
    assert_eq!(matched["isError"], false);
    assert!(matched["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Alice"));

    let absent = f
        .call("rule_matches", json!({"rule_id":Uuid::now_v7()}))
        .await?;
    assert_eq!(absent["isError"], false);
    assert_eq!(
        absent["content"][0]["text"],
        "That rule marks nothing right now."
    );
    sqlx::query("DELETE FROM attribute_rules WHERE kb_id=$1")
        .bind(f.kb)
        .execute(&f.state.pool)
        .await?;
    let empty = f.call("list_rules", json!({})).await?;
    assert_eq!(empty["isError"], false);
    assert_eq!(
        empty["content"][0]["text"],
        "This base has no business rules."
    );
    f.clean().await
}
