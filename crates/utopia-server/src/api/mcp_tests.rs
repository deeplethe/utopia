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
async fn record_axis_subseconds_survive_authenticated_rdf_export() -> anyhow::Result<()> {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use oxrdf::{vocab::xsd, Term};
    use tower::ServiceExt;

    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    async fn check(f: &Fixture) -> anyhow::Result<()> {
        let generated: chrono::DateTime<chrono::Utc> = "2026-03-01T00:00:00.100001Z".parse()?;
        let invalidated: chrono::DateTime<chrono::Utc> = "2026-03-01T00:00:00.100002Z".parse()?;
        for (table, created, deleted, id) in [
            ("facts", "recorded_at", "invalidated_at", f.fact),
            ("derived_facts", "derived_at", "invalidated_at", f.derived),
            ("documents", "created_at", "deleted_at", f.document),
        ] {
            sqlx::query(&format!(
                "UPDATE {table} SET {created}=$2,{deleted}=$3 WHERE id=$1"
            ))
            .bind(id)
            .bind(generated)
            .bind(invalidated)
            .execute(&f.state.pool)
            .await?;
        }
        let snapshot_sql = "SELECT jsonb_build_array(
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM facts t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM documents t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM derived_facts t WHERE kb_id=$1))";
        let before: Value = sqlx::query_scalar(snapshot_sql)
            .bind(f.kb)
            .fetch_one(&f.state.pool)
            .await?;
        let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
        let jwt = crate::auth::issue_token(&f.state, auth.user_id)?;
        let app = crate::api::router(f.state.clone(), &Default::default());
        let names = crate::rdf::Names::new(f.kb, None).map_err(anyhow::Error::msg)?;
        let subjects = [
            names.fact(f.fact),
            names.derived(f.derived),
            names.document(f.document),
        ];
        let mut exports = Vec::new();
        for (format, parser_format) in [
            ("turtle", oxrdfio::RdfFormat::Turtle),
            (
                "jsonld",
                oxrdfio::RdfFormat::JsonLd {
                    profile: oxrdfio::JsonLdProfileSet::empty(),
                },
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/kbs/{}/export?format={format}", f.kb))
                        .header("authorization", format!("Bearer {jwt}"))
                        .body(Body::empty())?,
                )
                .await?;
            anyhow::ensure!(response.status() == StatusCode::OK, "export rejected");
            let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
            let quads = oxrdfio::RdfParser::from_format(parser_format)
                .for_slice(&bytes)
                .collect::<Result<std::collections::HashSet<_>, _>>()?;
            for subject in &subjects {
                for (predicate, expected) in [
                    ("generatedAtTime", generated),
                    ("invalidatedAtTime", invalidated),
                ] {
                    let q = quads
                        .iter()
                        .find(|q| {
                            q.subject == subject.clone().into()
                                && q.predicate.as_str()
                                    == format!("http://www.w3.org/ns/prov#{predicate}")
                        })
                        .ok_or_else(|| anyhow::anyhow!("missing {predicate} for {subject}"))?;
                    let Term::Literal(literal) = &q.object else {
                        anyhow::bail!("timestamp is not literal");
                    };
                    anyhow::ensure!(
                        literal.datatype() == xsd::DATE_TIME,
                        "timestamp type changed"
                    );
                    let actual: chrono::DateTime<chrono::Utc> = literal.value().parse()?;
                    anyhow::ensure!(
                        actual == expected,
                        "record timestamp truncated: {actual} != {expected}"
                    );
                }
            }
            exports.push(quads);
        }
        anyhow::ensure!(exports[0] == exports[1], "formats disagree");
        let after: Value = sqlx::query_scalar(snapshot_sql)
            .bind(f.kb)
            .fetch_one(&f.state.pool)
            .await?;
        anyhow::ensure!(before == after, "export changed records");
        Ok(())
    }
    let result = check(&f).await;
    let cleanup = f.clean().await;
    result.and(cleanup)
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
async fn wrong_string_types_are_refused_and_audited_without_writing_memory() -> anyhow::Result<()> {
    let Some(mut f) = Fixture::new().await? else {
        return Ok(());
    };
    let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
    sqlx::query("UPDATE kb_members SET role='editor' WHERE kb_id=$1 AND user_id=$2")
        .bind(f.kb)
        .bind(auth.user_id)
        .execute(&f.state.pool)
        .await?;
    f.token = utopia_store::tokens::issue(
        &f.state.pool,
        auth.user_id,
        "argument types",
        "write",
        Some(&[f.kb]),
        None,
    )
    .await?
    .1;
    let mut calls = 0_i64;
    for (name, key) in [("search_chunks", "query"), ("remember", "text")] {
        for value in [
            json!(123),
            json!(false),
            json!(["pressure"]),
            json!({"text":"pressure"}),
        ] {
            let response = f.call(name, json!({key:value})).await?;
            assert_eq!(response["isError"], true, "{response}");
            assert!(response["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("must be a string"));
            assert!(response.get("structuredContent").is_none());
            calls += 1;
            let audited: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audit_events WHERE kb_id=$1 AND action='mcp.tool_called'",
            )
            .bind(f.kb)
            .fetch_one(&f.state.pool)
            .await?;
            assert_eq!(audited, calls);
        }
    }
    let memories: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE kb_id=$1 AND external_key='memory:log'",
    )
    .bind(f.kb)
    .fetch_one(&f.state.pool)
    .await?;
    assert_eq!(memories, 0);
    let good = f.call("search_chunks", json!({"query":"orchard"})).await?;
    assert_eq!(good["isError"], false);
    assert!(!good["structuredContent"]["chunks"]
        .as_array()
        .unwrap()
        .is_empty());
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
async fn entity_fact_qualifier_text_preserves_the_stored_string() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let value = json!({"value":"等级 \"A\" / C:\\reports\\a.txt\n第二行\""});
    sqlx::query("UPDATE fact_qualifiers SET value=$2 WHERE fact_id=$1")
        .bind(f.corrected)
        .bind(&value)
        .execute(&f.state.pool)
        .await?;
    let result = f
        .call("entity_facts", json!({"entity_id":f.subject}))
        .await?;
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains(&format!("[weight: {}]", value["value"].as_str().unwrap())),
        "{text}"
    );
    let fact = result["structuredContent"]["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|fact| fact["id"] == f.corrected.to_string())
        .unwrap();
    assert_eq!(fact["qualifiers"][0]["value"], value);
    let stored: Value = sqlx::query_scalar("SELECT value FROM fact_qualifiers WHERE fact_id=$1")
        .bind(f.corrected)
        .fetch_one(&f.state.pool)
        .await?;
    assert_eq!(stored, value);
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
async fn opposite_directions_reach_authenticated_path_output() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    async fn check(f: &Fixture) -> anyhow::Result<()> {
        let (a, b, p) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO relation_types(id,kb_id,key,label) VALUES ($1,$2,'supplies','supplies')",
        )
        .bind(p)
        .bind(f.kb)
        .execute(&f.state.pool)
        .await?;
        for (id, name) in [(a, "A"), (b, "B")] {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,$3)")
                .bind(id)
                .bind(f.kb)
                .bind(name)
                .execute(&f.state.pool)
                .await?;
        }
        for (s, o) in [(a, b), (b, a)] {
            utopia_store::graph::insert_fact(
                &f.state.pool,
                f.kb,
                s,
                Some(p),
                o,
                utopia_store::graph::Validity::starting(
                    Some("2026-01-01T00:00:00Z".parse()?),
                    Some("day"),
                ),
                0.9,
            )
            .await?;
        }
        let snapshot_sql = "SELECT jsonb_build_array(
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM facts t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM entities t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM relation_types t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM derived_facts t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM rules t WHERE kb_id=$1),
            (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM jobs t WHERE payload->>'kb_id'=$1::text))";
        let before: Value = sqlx::query_scalar(snapshot_sql)
            .bind(f.kb)
            .fetch_one(&f.state.pool)
            .await?;
        for (from, to, left, right) in [
            (a, b, "A —supplies→ B", "A ←supplies— B"),
            (b, a, "B —supplies→ A", "B ←supplies— A"),
        ] {
            let result = f
                .call(
                    "paths_between",
                    json!({"from":from,"to":to,"max_hops":1,"at":"2026-06-01"}),
                )
                .await?;
            let text = result["content"][0]["text"].as_str().unwrap_or_default();
            anyhow::ensure!(
                text.contains(left) && text.contains(right),
                "opposite path lost: {text}"
            );
            anyhow::ensure!(text.contains("2 paths"), "unexpected path count: {text}");
        }
        let after: Value = sqlx::query_scalar(snapshot_sql)
            .bind(f.kb)
            .fetch_one(&f.state.pool)
            .await?;
        anyhow::ensure!(before == after, "path read changed business data");
        Ok(())
    }
    let result = check(&f).await;
    let cleanup = f.clean().await;
    result.and(cleanup)
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
async fn remembered_clock_times_do_not_receive_the_date_only_offset() -> anyhow::Result<()> {
    let Some(mut f) = Fixture::new().await? else {
        return Ok(());
    };
    let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
    sqlx::query("UPDATE kb_members SET role='editor' WHERE kb_id=$1 AND user_id=$2")
        .bind(f.kb)
        .bind(auth.user_id)
        .execute(&f.state.pool)
        .await?;
    f.token = utopia_store::tokens::issue(
        &f.state.pool,
        auth.user_id,
        "memory time test",
        "write",
        Some(&[f.kb]),
        None,
    )
    .await?
    .1;
    for (input, stored, echoed) in [
        ("2026", "2026-01-01 12:00", "2026"),
        ("2026-09", "2026-09-01 12:00", "2026-09"),
        ("2026-09-20", "2026-09-20 12:00", "2026-09-20"),
        (
            "2026-09-20T18:30:00Z",
            "2026-09-20 18:30",
            "2026-09-20T18:30:00Z",
        ),
        (
            "2026-09-20T18:30:45.123Z",
            "2026-09-20 18:30",
            "2026-09-20T18:30:45Z",
        ),
        (
            "2026-09-20T18:30:00+08:00",
            "2026-09-20 10:30",
            "2026-09-20T10:30:00Z",
        ),
        (
            "2026-09-20T18:30:00-04:00",
            "2026-09-20 22:30",
            "2026-09-20T22:30:00Z",
        ),
        ("2026-09-20T23Z", "2026-09-20 23:00", "2026-09-20T23Z"),
        ("2026-09-20T23:45Z", "2026-09-20 23:45", "2026-09-20T23:45Z"),
        ("2026-09-20T23+02:00", "2026-09-20 21:00", "2026-09-20T21Z"),
        (
            "2026-09-20T23:45-02:00",
            "2026-09-21 01:45",
            "2026-09-21T01:45Z",
        ),
        // The shared parser deliberately falls back to day precision without a zone.
        ("2026-09-20T18:30:00", "2026-09-20 12:00", "2026-09-20"),
    ] {
        let sentence = format!("inspection at {input}");
        let result = f
            .call("remember", json!({"text":sentence,"occurred_at":input}))
            .await?;
        assert_eq!(result["isError"], false);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&format!("(effective {echoed})")));
        let text: String = sqlx::query_scalar(
            "SELECT text FROM chunks WHERE kb_id=$1 ORDER BY created_at DESC,seq DESC LIMIT 1",
        )
        .bind(f.kb)
        .fetch_one(&f.state.pool)
        .await?;
        assert_eq!(text, format!("[{stored}] {sentence}"), "input: {input}");
    }
    // Missing/invalid input keeps the existing 'now' fallback.
    for input in [Value::Null, json!(""), json!("not-a-date")] {
        let before = chrono::Utc::now();
        let result = f
            .call("remember", json!({"text":"fallback","occurred_at":input}))
            .await?;
        let after = chrono::Utc::now();
        assert_eq!(result["isError"], false);
        let reply = result["content"][0]["text"].as_str().unwrap();
        let echoed = reply
            .split("(effective ")
            .nth(1)
            .unwrap()
            .split(')')
            .next()
            .unwrap();
        let time: chrono::DateTime<chrono::Utc> = echoed.parse()?;
        assert!(before <= time && time <= after);
        let text: String = sqlx::query_scalar(
            "SELECT text FROM chunks WHERE kb_id=$1 ORDER BY created_at DESC,seq DESC LIMIT 1",
        )
        .bind(f.kb)
        .fetch_one(&f.state.pool)
        .await?;
        assert_eq!(
            text,
            format!("[{}] fallback", time.format("%Y-%m-%d %H:%M"))
        );
    }
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind='memory_ingest' AND payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$1)")
        .bind(f.kb).fetch_one(&f.state.pool).await?;
    assert_eq!(queued, 15);
    sqlx::query("DELETE FROM jobs WHERE kind='memory_ingest' AND payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$1)")
        .bind(f.kb).execute(&f.state.pool).await?;
    f.clean().await
}

#[tokio::test]
async fn declared_property_links_survive_authenticated_rdf_export() -> anyhow::Result<()> {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use utopia_core::models::RelationAxioms;
    use utopia_store::ontology::{
        create_relation_type, create_relation_with_iri, update_relation_type,
    };

    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    async fn check(f: &Fixture) -> anyhow::Result<()> {
        const IMPORTED: &str = "https://example.test/worksFor";
        let root = create_relation_with_iri(
            &f.state.pool,
            f.kb,
            "employment",
            "Employment",
            "",
            IMPORTED,
            false,
            false,
            &[],
            &[],
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("missing imported relation"))?;
        let mut declared = Vec::new();
        let mut parent = root;
        for key in ["manages", "directs", "leads"] {
            let ax = RelationAxioms {
                sub_property_of: Some(parent),
                ..Default::default()
            };
            let id = create_relation_type(
                &f.state.pool,
                f.kb,
                key,
                key,
                "state",
                ax,
                "",
                "relation",
                &[],
                &[],
                None,
                None,
            )
            .await?;
            declared.push((id, key, parent));
            parent = id;
        }
        let inverse = create_relation_type(
            &f.state.pool,
            f.kb,
            "employs",
            "Employs",
            "state",
            RelationAxioms {
                inverse_of: Some(root),
                ..Default::default()
            },
            "",
            "relation",
            &[],
            &[],
            None,
            None,
        )
        .await?;
        // Store exactly the reciprocal declaration too; export must not manufacture it.
        update_relation_type(
            &f.state.pool,
            f.kb,
            root,
            "Employment",
            "state",
            RelationAxioms {
                inverse_of: Some(inverse),
                ..Default::default()
            },
            "",
            None,
            None,
            None,
            None,
        )
        .await?;
        let other = create_relation_type(
            &f.state.pool,
            f.other_kb,
            "employment",
            "Employment",
            "state",
            Default::default(),
            "",
            "relation",
            &[],
            &[],
            None,
            None,
        )
        .await?;
        anyhow::ensure!(
            create_relation_type(
                &f.state.pool,
                f.kb,
                "bad_cross_base",
                "Bad",
                "state",
                RelationAxioms {
                    inverse_of: Some(other),
                    ..Default::default()
                },
                "",
                "relation",
                &[],
                &[],
                None,
                None
            )
            .await
            .is_err(),
            "cross-base input must be refused by the real write path"
        );
        let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
        let jwt = crate::auth::issue_token(&f.state, auth.user_id)?;
        let app = crate::api::router(f.state.clone(), &Default::default());
        let names = crate::rdf::Names::new(f.kb, None).map_err(anyhow::Error::msg)?;
        let relation_iri = |key: &str| format!("<urn:utopia:kb:{}:relation:{key}>", f.kb);
        let inverse_term = "<http://www.w3.org/2002/07/owl#inverseOf>".to_string();
        let sub_term = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>".to_string();
        let expected: std::collections::HashSet<_> = [
            (
                relation_iri("employs"),
                inverse_term.clone(),
                format!("<{IMPORTED}>"),
            ),
            (
                format!("<{IMPORTED}>"),
                inverse_term.clone(),
                relation_iri("employs"),
            ),
            (
                relation_iri("manages"),
                sub_term.clone(),
                format!("<{IMPORTED}>"),
            ),
            (
                relation_iri("directs"),
                sub_term.clone(),
                relation_iri("manages"),
            ),
            (
                relation_iri("leads"),
                sub_term.clone(),
                relation_iri("directs"),
            ),
        ]
        .into_iter()
        .collect();
        for renamed in [false, true] {
            if renamed {
                update_relation_type(
                    &f.state.pool,
                    f.kb,
                    declared[0].0,
                    "New label",
                    "state",
                    RelationAxioms {
                        sub_property_of: Some(root),
                        ..Default::default()
                    },
                    "",
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            }
            let snapshot_sql = "SELECT jsonb_build_array(
                (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM relation_types t WHERE kb_id=$1),
                (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM facts t WHERE kb_id=$1),
                (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM derived_facts t WHERE kb_id=$1),
                (SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM jobs t WHERE payload->>'kb_id'=$1::text))";
            let before: Value = sqlx::query_scalar(snapshot_sql)
                .bind(f.kb)
                .fetch_one(&f.state.pool)
                .await?;
            let mut exports = Vec::new();
            for (format, parser_format) in [
                ("turtle", oxrdfio::RdfFormat::Turtle),
                (
                    "jsonld",
                    oxrdfio::RdfFormat::JsonLd {
                        profile: oxrdfio::JsonLdProfileSet::empty(),
                    },
                ),
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(format!("/api/v1/kbs/{}/export?format={format}", f.kb))
                            .header("authorization", format!("Bearer {jwt}"))
                            .body(Body::empty())?,
                    )
                    .await?;
                anyhow::ensure!(response.status() == StatusCode::OK, "export rejected");
                let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
                let quads = oxrdfio::RdfParser::from_format(parser_format)
                    .for_slice(&bytes)
                    .collect::<Result<std::collections::HashSet<_>, _>>()?;
                let links: std::collections::HashSet<_> = quads
                    .iter()
                    .filter(|q| {
                        [inverse_term.as_str(), sub_term.as_str()]
                            .contains(&q.predicate.to_string().as_str())
                    })
                    .map(|q| {
                        (
                            q.subject.to_string(),
                            q.predicate.to_string(),
                            q.object.to_string(),
                        )
                    })
                    .collect();
                anyhow::ensure!(
                    links == expected,
                    "declared property links missing or invented: {links:?}"
                );
                anyhow::ensure!(
                    quads
                        .iter()
                        .any(|q| q.subject == names.fact(f.corrected).into()),
                    "lost existing facts"
                );
                exports.push(quads);
            }
            anyhow::ensure!(exports[0] == exports[1], "formats disagree");
            let after: Value = sqlx::query_scalar(snapshot_sql)
                .bind(f.kb)
                .fetch_one(&f.state.pool)
                .await?;
            anyhow::ensure!(
                before == after,
                "export changed stored declarations/facts/jobs"
            );
        }
        Ok(())
    }
    let result = check(&f).await;
    let cleanup = f.clean().await;
    result.and(cleanup)
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
async fn failed_memory_writes_are_tool_errors() -> anyhow::Result<()> {
    let Some(mut f) = Fixture::new().await? else {
        return Ok(());
    };
    let denied = f
        .request(
            f.kb,
            "tools/call",
            json!({"name":"remember","arguments":{"text":"denied"}}),
        )
        .await
        .map_err(|e| e.0)?
        .0;
    assert_eq!(denied["error"]["code"], -32601);
    let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
    sqlx::query("UPDATE kb_members SET role='editor' WHERE kb_id=$1 AND user_id=$2")
        .bind(f.kb)
        .bind(auth.user_id)
        .execute(&f.state.pool)
        .await?;
    f.token = utopia_store::tokens::issue(
        &f.state.pool,
        auth.user_id,
        "memory error test",
        "write",
        Some(&[f.kb]),
        None,
    )
    .await?
    .1;
    let success = f.call("remember", json!({"text":"recorded"})).await?;
    assert_eq!(success["isError"], false);
    assert!(success["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("Recorded the sentence"));
    sqlx::query("DELETE FROM jobs WHERE kind='memory_ingest' AND payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$1)")
        .bind(f.kb).execute(&f.state.pool).await?;
    // As in failed_reads_do_not_become_successful_empty_results, close only this
    // fixture's pool. append_episode fails before writing or enqueueing anything.
    f.state.pool.close().await;
    let ctx = ToolCtx {
        state: &f.state,
        kb_id: f.kb,
        workspace_id: f.ws,
        mounted_sources: &[],
        can_write: true,
        actor: Some(auth.user_id),
        via_token: None,
        question: None,
    };
    let failed = tool_result(
        tools::dispatch(
            &ctx,
            &mut ToolSink::default(),
            "remember",
            &json!({"text":"not recorded"}),
        )
        .await,
    );
    f.state.pool = sqlx::PgPool::connect(&utopia_store::test_db::url().unwrap()).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chunks WHERE kb_id=$1 AND text LIKE '%not recorded%'",
    )
    .bind(f.kb)
    .fetch_one(&f.state.pool)
    .await?;
    assert_eq!(count, 0);
    f.clean().await?;
    assert!(failed["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("Failed to record:"));
    assert_eq!(failed["isError"], true, "{failed}");
    Ok(())
}

#[tokio::test]
async fn memory_text_empty_after_nul_removal_is_a_tool_error() -> anyhow::Result<()> {
    let Some(mut f) = Fixture::new().await? else {
        return Ok(());
    };
    let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
    sqlx::query("UPDATE kb_members SET role='editor' WHERE kb_id=$1 AND user_id=$2")
        .bind(f.kb)
        .bind(auth.user_id)
        .execute(&f.state.pool)
        .await?;
    f.token = utopia_store::tokens::issue(
        &f.state.pool,
        auth.user_id,
        "empty memory test",
        "write",
        Some(&[f.kb]),
        None,
    )
    .await?
    .1;
    let result = f.call("remember", json!({"text":"\u{0} \u{0}"})).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM documents WHERE kb_id=$1 AND external_key='memory:log'",
    )
    .bind(f.kb)
    .fetch_one(&f.state.pool)
    .await?;
    assert_eq!(count, 0);
    f.clean().await?;
    assert_eq!(
        result["content"][0]["text"],
        "remember requires non-empty text."
    );
    assert_eq!(result["isError"], true);
    Ok(())
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

#[tokio::test]
async fn rule_descriptions_preserve_condition_groups() -> anyhow::Result<()> {
    use utopia_store::business_rules::{self, ConditionInput};
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let (ty, attr): (Uuid, Uuid) = sqlx::query_as(
        "SELECT subject_type_id, conclude_predicate_id FROM attribute_rules WHERE kb_id=$1",
    )
    .bind(f.kb)
    .fetch_one(&f.state.pool)
    .await?;
    for (name, groups, expected) in [
        (
            "single",
            [0, 0, 0],
            "weight gt 1 AND weight lt 9 AND weight gte 7",
        ),
        (
            "mixed",
            [0, 0, 1],
            "(weight gt 1 AND weight lt 9) OR weight gte 7",
        ),
        (
            "sparse",
            [2, 2, 9],
            "(weight gt 1 AND weight lt 9) OR weight gte 7",
        ),
        (
            "singletons",
            [2, 9, 12],
            "weight gt 1 OR weight lt 9 OR weight gte 7",
        ),
    ] {
        let cs: Vec<_> = groups
            .into_iter()
            .zip([("gt", 1), ("lt", 9), ("gte", 7)])
            .map(|(group, (op, n))| ConditionInput {
                group,
                predicate_id: attr,
                op: op.into(),
                operand: Some(json!(n)),
            })
            .collect();
        business_rules::create(
            &f.state.pool,
            f.kb,
            name,
            "",
            ty,
            "attribute",
            None,
            Some(attr),
            Some(json!({"value":8})),
            None,
            &cs,
        )
        .await?;
        let before = business_rules::list(&f.state.pool, f.kb).await?;
        let facts_before: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY id),'[]') FROM derived_facts d WHERE kb_id=$1"
        ).bind(f.kb).fetch_one(&f.state.pool).await?;
        let response = f.call("list_rules", json!({})).await?;
        assert_eq!(response["isError"], false);
        let text = response["content"][0]["text"].as_str().unwrap();
        let line = text
            .lines()
            .find(|line| line.starts_with(&format!("{name} [")))
            .unwrap();
        assert!(line.contains(&format!("where {expected} ⇒")), "{line}");
        assert_eq!(business_rules::list(&f.state.pool, f.kb).await?, before);
        let facts_after: Value = sqlx::query_scalar(
            "SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY id),'[]') FROM derived_facts d WHERE kb_id=$1"
        ).bind(f.kb).fetch_one(&f.state.pool).await?;
        assert_eq!(facts_after, facts_before);
    }
    f.clean().await
}

#[tokio::test]
async fn rule_matches_keep_materialized_intervals_and_count_rows() -> anyhow::Result<()> {
    use utopia_store::business_rules::{self, ConditionInput};
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let ty: Uuid = sqlx::query_scalar("SELECT type_id FROM entities WHERE id=$1")
        .bind(f.subject)
        .fetch_one(&f.state.pool)
        .await?;
    let (reading, result, is_a, marked) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    for (id, key, datatype, builtin) in [
        (reading, "reading", "number", false),
        (result, "result", "number", false),
        (is_a, "is_a", "text", true),
    ] {
        sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,kind,datatype,builtin) VALUES ($1,$2,$3,$3,'attribute',$4,$5)")
            .bind(id).bind(f.kb).bind(key).bind(datatype).bind(builtin).execute(&f.state.pool).await?;
    }
    sqlx::query("INSERT INTO entity_types(id,kb_id,key,label) VALUES ($1,$2,'marked','Marked')")
        .bind(marked)
        .bind(f.kb)
        .execute(&f.state.pool)
        .await?;
    let conditions = [ConditionInput {
        group: 0,
        predicate_id: reading,
        op: "gt".into(),
        operand: Some(json!(0)),
    }];
    let typing = business_rules::create(
        &f.state.pool,
        f.kb,
        "historical typing",
        "",
        ty,
        "typing",
        Some(marked),
        None,
        None,
        None,
        &conditions,
    )
    .await?;
    let attribute = business_rules::create(
        &f.state.pool,
        f.kb,
        "historical attribute",
        "",
        ty,
        "attribute",
        None,
        Some(result),
        Some(json!(8)),
        None,
        &conditions,
    )
    .await?;
    // Source readings use deliberately disjoint, fixed historical intervals.
    // The derived rows and their precision are produced by the real materializer.
    for (from, to, fp, tp) in [
        (
            "2020-01-01T00:00:00Z",
            "2021-03-01T00:00:00Z",
            "year",
            "month",
        ),
        (
            "2023-06-01T00:00:00Z",
            "2024-07-15T00:00:00Z",
            "month",
            "day",
        ),
    ] {
        sqlx::query("INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_value,valid_from,valid_to,valid_from_precision,valid_to_precision) VALUES ($1,$2,$3,$4,'{\"value\":10}',$5,$6,$7,$8)")
            .bind(Uuid::now_v7()).bind(f.kb).bind(f.subject).bind(reading)
            .bind(from.parse::<chrono::DateTime<chrono::Utc>>()?).bind(to.parse::<chrono::DateTime<chrono::Utc>>()?)
            .bind(fp).bind(tp).execute(&f.state.pool).await?;
    }
    utopia_store::reasoning::materialize(&f.state.pool, f.kb).await?;
    let before: Value = sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY id),'[]') FROM derived_facts d WHERE kb_id=$1")
        .bind(f.kb).fetch_one(&f.state.pool).await?;
    let rules_before = business_rules::list(&f.state.pool, f.kb).await?;
    let jobs_before: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY id),'[]') FROM jobs j WHERE payload->>'kb_id'=$1 OR payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$2)")
        .bind(f.kb.to_string()).bind(f.kb).fetch_one(&f.state.pool).await?;
    for (rule, conclusion) in [(typing, "Marked"), (attribute, "8")] {
        let (rows, total) = business_rules::matches(&f.state.pool, f.kb, rule, 50, 0).await?;
        assert_eq!(total, 2, "real materialization must retain both intervals");
        assert!(rows.iter().all(|r| uuid(&r["entity_id"]) == f.subject));
        let response = f.call("rule_matches", json!({"rule_id":rule})).await?;
        assert_eq!(response["isError"], false);
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("validity: 2020 → 2021-03"), "{text}");
        assert!(text.contains("validity: 2023-06 → 2024-07-15"), "{text}");
        assert_eq!(
            text.matches(&format!("Alice ⇒ {conclusion} (because reading = 10)"))
                .count(),
            2,
            "{text}"
        );
        let page = f
            .call("rule_matches", json!({"rule_id":rule,"limit":1}))
            .await?;
        assert!(page["content"][0]["text"]
            .as_str()
            .unwrap()
            .ends_with("(showing 1 of 2 matches)"));
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
        let card = tools::rule_matches(&ctx, &json!({"rule_id":rule})).await;
        assert_eq!(card.step["detail"], "2 matches");
    }
    let after: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(d) ORDER BY id),'[]') FROM derived_facts d WHERE kb_id=$1")
        .bind(f.kb).fetch_one(&f.state.pool).await?;
    assert_eq!(before, after);
    assert_eq!(
        rules_before,
        business_rules::list(&f.state.pool, f.kb).await?
    );
    let jobs_after: Value=sqlx::query_scalar("SELECT coalesce(jsonb_agg(to_jsonb(j) ORDER BY id),'[]') FROM jobs j WHERE payload->>'kb_id'=$1 OR payload->>'document_id' IN (SELECT id::text FROM documents WHERE kb_id=$2)")
        .bind(f.kb.to_string()).bind(f.kb).fetch_one(&f.state.pool).await?;
    assert_eq!(jobs_before, jobs_after);
    // Legacy/anchor-derived rows may have no stated precision or boundary.
    let row = uuid(
        &business_rules::matches(&f.state.pool, f.kb, typing, 50, 0)
            .await?
            .0[0]["derived_id"],
    );
    for (from, expected) in [
        (
            Some("2020-01-01T12:34:56.123456Z"),
            "2020-01-01T12:34:56.123456Z → unknown end",
        ),
        (None, "unknown start → unknown end"),
    ] {
        let from = from
            .map(str::parse::<chrono::DateTime<chrono::Utc>>)
            .transpose()?;
        sqlx::query("UPDATE derived_facts SET valid_from=$2,valid_to=NULL,valid_from_precision=NULL,valid_to_precision=NULL WHERE id=$1")
            .bind(row).bind(from).execute(&f.state.pool).await?;
        let response = f.call("rule_matches", json!({"rule_id":typing})).await?;
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(expected), "{text}");
        assert!(!text.contains("→ now"));
    }
    sqlx::query("UPDATE derived_facts SET invalidated_at=now() WHERE attribute_rule_id=$1 AND valid_from IS NULL")
        .bind(typing).execute(&f.state.pool).await?;
    let response = f.call("rule_matches", json!({"rule_id":typing})).await?;
    let text = response["content"][0]["text"].as_str().unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.contains("2023-06 → 2024-07-15"));
    assert_eq!(
        business_rules::matches(&f.state.pool, f.other_kb, typing, 50, 0)
            .await?
            .1,
        0
    );
    f.clean().await
}

// Reuse the authenticated ledger fixture so RDF exercises the same stored records
// as structured MCP reads, including evidence and retracted history.
#[tokio::test]
async fn rdf_export_preserves_unbound_literal_objects() -> anyhow::Result<()> {
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use oxrdf::{vocab::rdf, Literal, Term};
    use tower::ServiceExt;

    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    async fn check(f: &Fixture) -> anyhow::Result<()> {
        let value = json!({"value": "待复检"});
        let (statement, _) = utopia_store::graph::insert_open_statement(
            &f.state.pool,
            f.kb,
            f.subject,
            "状态",
            utopia_store::graph::FactObject::Value(&value),
            Some("2026-01-01T00:00:00Z".parse()?),
            0.9,
        )
        .await?;
        sqlx::query("UPDATE facts SET recorded_at='2026-02-01' WHERE id=$1")
            .bind(statement)
            .execute(&f.state.pool)
            .await?;
        sqlx::query(
            "INSERT INTO fact_evidence(fact_id,chunk_id,document_id,doc_version,quote)
                     VALUES ($1,$2,$3,1,'设备 A 待复检')",
        )
        .bind(statement)
        .bind(f.chunk)
        .bind(f.document)
        .execute(&f.state.pool)
        .await?;
        let auth = utopia_store::tokens::authenticate(&f.state.pool, &f.token).await?;
        let jwt = crate::auth::issue_token(&f.state, auth.user_id)?;
        let app = crate::api::router(f.state.clone(), &Default::default());
        let names = crate::rdf::Names::new(f.kb, None).map_err(anyhow::Error::msg)?;
        let stmt = names.fact(statement);
        let mut formats = Vec::new();
        for retracted in [false, true] {
            if retracted {
                sqlx::query("UPDATE facts SET invalidated_at='2026-03-01' WHERE id=$1")
                    .bind(statement)
                    .execute(&f.state.pool)
                    .await?;
            }
            // Snapshot every KB-scoped business table, including queues and adoption
            // records. Request audit is deliberately excluded from this read-only check.
            let tables: Vec<String> = sqlx::query_scalar(
                "SELECT table_name FROM information_schema.columns
                 WHERE table_schema='public' AND column_name='kb_id'
                   AND table_name <> 'audit_events' ORDER BY table_name",
            )
            .fetch_all(&f.state.pool)
            .await?;
            let snapshot = async {
                let mut rows = Vec::new();
                for table in &tables {
                    let sql = format!("SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM \"{}\" t WHERE kb_id=$1", table.replace('"', "\"\""));
                    rows.push(
                        sqlx::query_scalar::<_, Value>(&sql)
                            .bind(f.kb)
                            .fetch_one(&f.state.pool)
                            .await?,
                    );
                }
                Ok::<_, anyhow::Error>(rows)
            };
            let before = snapshot.await?;
            let extra_sql = "SELECT jsonb_build_array(
                (SELECT COALESCE(jsonb_agg(to_jsonb(e) ORDER BY to_jsonb(e)::text), '[]')
                 FROM fact_evidence e JOIN facts f ON f.id=e.fact_id WHERE f.kb_id=$1),
                (SELECT COALESCE(jsonb_agg(to_jsonb(j) ORDER BY j.id), '[]') FROM jobs j
                 WHERE payload->>'kb_id'=$1::text OR payload->>'document_id' IN
                     (SELECT id::text FROM documents WHERE kb_id=$1)))";
            let extra_before: Value = sqlx::query_scalar(extra_sql)
                .bind(f.kb)
                .fetch_one(&f.state.pool)
                .await?;
            for format in ["turtle", "jsonld"] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(format!("/api/v1/kbs/{}/export?format={format}", f.kb))
                            .header("authorization", format!("Bearer {jwt}"))
                            .body(Body::empty())?,
                    )
                    .await?;
                anyhow::ensure!(response.status() == StatusCode::OK, "export rejected");
                let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
                let format = if format == "turtle" {
                    oxrdfio::RdfFormat::Turtle
                } else {
                    oxrdfio::RdfFormat::JsonLd {
                        profile: oxrdfio::JsonLdProfileSet::empty(),
                    }
                };
                let quads = oxrdfio::RdfParser::from_format(format)
                    .for_slice(&bytes)
                    .collect::<Result<std::collections::HashSet<_>, _>>()?;
                anyhow::ensure!(
                    quads.iter().any(|q| q.subject == stmt.clone().into()
                        && q.predicate == rdf::OBJECT
                        && q.object == Term::Literal(Literal::new_simple_literal("待复检"))),
                    "unbound statement lost its rdf:object in authenticated export"
                );
                anyhow::ensure!(
                    !quads
                        .iter()
                        .any(|q| q.subject == stmt.clone().into() && q.predicate == rdf::PREDICATE),
                    "invented a bound predicate"
                );
                anyhow::ensure!(
                    quads.iter().any(|q| q.subject == stmt.clone().into()
                        && q.predicate.as_str() == "http://www.w3.org/ns/prov#wasDerivedFrom"
                        && q.object == names.document(f.document).into()),
                    "lost evidence source"
                );
                formats.push(quads);
            }
            anyhow::ensure!(
                formats[formats.len() - 1] == formats[formats.len() - 2],
                "formats disagree"
            );
            let extra_after: Value = sqlx::query_scalar(extra_sql)
                .bind(f.kb)
                .fetch_one(&f.state.pool)
                .await?;
            anyhow::ensure!(
                extra_before == extra_after,
                "export changed evidence or jobs"
            );
            for (table, expected) in tables.iter().zip(before) {
                let sql = format!("SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]'::jsonb) FROM \"{}\" t WHERE kb_id=$1", table.replace('"', "\"\""));
                let actual: Value = sqlx::query_scalar(&sql)
                    .bind(f.kb)
                    .fetch_one(&f.state.pool)
                    .await?;
                anyhow::ensure!(actual == expected, "export changed {table}");
            }
        }
        Ok(())
    }
    let result = check(&f).await;
    let cleanup = f.clean().await;
    result.and(cleanup)
}
