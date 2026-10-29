use super::*;
use serde_json::{json, Value};
use wiremock::{matchers::method, matchers::path, Mock, MockServer, Request, ResponseTemplate};

async fn decide_pair(
    governance: bool,
    same: bool,
    identity: bool,
    proposal: &str,
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let server = MockServer::start().await;
    let verdict = if same { "same" } else { "different" };
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(move |request: &Request| {
            let body: Value = request.body_json().unwrap();
            let answer = json!({"verdict": verdict, "confidence": 0.99, "why": "source checked"});
            let message = if body["tools"].is_array() {
                json!({"role":"assistant","content":null,"tool_calls":[{
                    "id":"decide-1","type":"function","function":{
                        "name":"decide","arguments":answer.to_string()
                    }
                }]})
            } else {
                json!({"role":"assistant","content":json!({"verdicts":[{
                    "i":0,"verdict":verdict,"confidence":0.99,"why":"source checked"
                }]}).to_string()})
            };
            ResponseTemplate::new(200).set_body_json(json!({
                "choices":[{"message":message,"finish_reason":"stop"}]
            }))
        })
        .mount(&server)
        .await;

    let [org, ws, kb, source, doc, chunk, prose, prose_chunk, left, right]: [Uuid; 10] =
        std::array::from_fn(|_| Uuid::now_v7());
    let result = async {
        let similarity = utopia_core::review_reasons::similarity_proposed(Some(proposal));
        let right_name = if similarity { "Account" } else { "InvoiceExporter" };
        let config = if identity {
            json!({"names":"identity"})
        } else {
            json!({})
        };
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','identity-merge');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','identity-merge');
             INSERT INTO knowledge_bases(id,workspace_id,name,governance)
                 VALUES ('{kb}','{ws}','identity-merge',{governance});
             INSERT INTO sources(id,kb_id,kind,name,config)
                 VALUES ('{source}','{kb}','statements','identities',$${config}$$::jsonb);
             INSERT INTO documents(id,kb_id,filename,sha256,source_id) VALUES
                 ('{doc}','{kb}','records.json','{doc}','{source}'),
                 ('{prose}','{kb}','notes.md','{prose}',NULL);
             INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
                 ('{chunk}','{kb}','{doc}',0,'billing/export'),
                 ('{prose_chunk}','{kb}','{prose}',0,'billing/export');
             INSERT INTO entities(id,kb_id,canonical_name) VALUES
                 ('{left}','{kb}','InvoiceExporter'),('{right}','{kb}','{right_name}');"
        ))
        .execute(&pool)
        .await?;

        // 别名同样算数，证据里散文和标识混在一起也算：名字带标识的是哪一侧都得拦下
        let named = if governance { left } else { right };
        for evidence in [prose_chunk, chunk] {
            utopia_store::names::record(
                &pool,
                kb,
                named,
                "billing/export",
                Some(utopia_store::names::NameSource {
                    chunk_id: evidence,
                    quote: "billing/export",
                }),
                None,
            )
            .await?;
        }
        utopia_store::resolution::create_review(
            &pool,
            kb,
            left,
            right,
            0.9,
            proposal,
            utopia_store::resolution::ReviewStage::Adjudicating,
        )
        .await?;
        utopia_store::settings::upsert(
            &pool,
            ws,
            Some(&server.uri()),
            None,
            Some("identity-merge"),
            None,
            None,
            None,
            None,
        )
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        if governance {
            crate::governance::govern(&state, kb).await?;
        } else {
            adjudicate_entities(&state, kb).await?;
        }
        let (review, status, stage, reason): (Uuid, String, String, String) = sqlx::query_as(
            "SELECT id, status, stage, reason FROM resolution_reviews WHERE kb_id=$1",
        )
        .bind(kb)
        .fetch_one(&pool)
        .await?;
        let held = identity && same;
        assert_eq!(
            status,
            if held {
                "pending"
            } else if same {
                "merged"
            } else {
                "kept"
            }
        );
        if held {
            assert_eq!(stage, "human");
            assert_eq!(reason, "escalate_identity");
        }
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM entities WHERE kb_id=$1 AND merged_into IS NULL",
        )
        .bind(kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(live, if same && !identity { 1 } else { 2 });
        let second_look = same && similarity;
        let calls = server.received_requests().await.unwrap().len();
        assert_eq!(calls, if second_look { 2 } else { 1 });
        if governance || second_look {
            let (action, status, merge_id, reason): (String, String, Option<Uuid>, Option<String>) =
                sqlx::query_as("SELECT action, status, merge_id, reason FROM agent_decisions WHERE kb_id=$1")
                    .bind(kb)
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(action, if same { "merge" } else { "keep" });
            assert_eq!(status, if held { "proposed" } else { "applied" });
            if held {
                assert!(merge_id.is_none());
                if governance {
                    assert_eq!(
                        reason.as_deref(),
                        Some("held for a person: a source treats one side's names as identifiers; source checked")
                    );
                }
            }
        }
        if held && !governance {
            // 缓存的模型判决重放时也得再查一次来源，不能直接沿用
            sqlx::query("UPDATE resolution_reviews SET stage='adjudicating' WHERE id=$1")
                .bind(review)
                .execute(&pool)
                .await?;
            adjudicate_entities(&state, kb).await?;
            let replay: (String, String, String) =
                sqlx::query_as("SELECT status, stage, reason FROM resolution_reviews WHERE id=$1")
                    .bind(review)
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(
                replay,
                ("pending".into(), "human".into(), "escalate_identity".into())
            );
            assert_eq!(server.received_requests().await.unwrap().len(), calls);

            // 关掉声明后，缓存的判决直接放行，不再调用模型
            sqlx::query("UPDATE sources SET config='{}'::jsonb WHERE id=$1")
                .bind(source)
                .execute(&pool)
                .await?;
            sqlx::query("UPDATE resolution_reviews SET stage='adjudicating' WHERE id=$1")
                .bind(review)
                .execute(&pool)
                .await?;
            adjudicate_entities(&state, kb).await?;
            let status: String =
                sqlx::query_scalar("SELECT status FROM resolution_reviews WHERE id=$1")
                    .bind(review)
                    .fetch_one(&pool)
                    .await?;
            assert_eq!(status, "merged");
            let live: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM entities WHERE kb_id=$1 AND merged_into IS NULL",
            )
            .bind(kb)
            .fetch_one(&pool)
            .await?;
            assert_eq!(live, 1);
            assert_eq!(server.received_requests().await.unwrap().len(), calls);
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    pool.close().await;
    result
}

#[tokio::test]
async fn a_same_verdict_on_an_identity_entity_waits_for_a_person() -> anyhow::Result<()> {
    for governance in [false, true] {
        for proposal in ["ambiguous_name|0.7", "name_vector|0.9"] {
            decide_pair(governance, true, true, proposal).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn an_identity_source_still_allows_a_kept_verdict() -> anyhow::Result<()> {
    for governance in [false, true] {
        decide_pair(governance, false, true, "name_vector|0.9").await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_source_without_identity_names_still_allows_a_merge() -> anyhow::Result<()> {
    for governance in [false, true] {
        decide_pair(governance, true, false, "name_vector|0.9").await?;
    }
    Ok(())
}
