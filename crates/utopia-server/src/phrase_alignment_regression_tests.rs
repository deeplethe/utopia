//! Human examples observe ordinary phrase alignment, including failures and protected decisions.
use super::*;

struct RegressionFixture {
    pool: sqlx::PgPool,
    state: AppState,
    model: wiremock::MockServer,
    org: Uuid,
    kb: Uuid,
    case: Uuid,
    property: Uuid,
    _dir: tempfile::TempDir,
}

impl RegressionFixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, workspace, kb, actor, subject, object, property) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'alignment-regression')")
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'alignment-regression')")
            .bind(workspace)
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'alignment-regression')")
            .bind(kb).bind(workspace).execute(&pool).await?;
        sqlx::query("INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES($1,$2,$3,'fixture','Reviewer')")
            .bind(actor).bind(org).bind(format!("{actor}@example.test")).execute(&pool).await?;
        for (id, name) in [(subject, "Acme"), (object, "Harbor")] {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)")
                .bind(id)
                .bind(kb)
                .bind(name)
                .execute(&pool)
                .await?;
        }
        sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,kind,temporal,description) VALUES($1,$2,'acquired','acquired','relation','event','The subject acquired the object.')")
            .bind(property).bind(kb).execute(&pool).await?;
        let statement = utopia_store::graph::insert_open_statement(
            &pool,
            kb,
            subject,
            " acquired ",
            utopia_store::graph::FactObject::Entity(object),
            None,
            1.0,
        )
        .await?
        .0;
        let case = utopia_store::ontology_regressions::add(
            &pool,
            kb,
            utopia_store::ontology_regressions::NewCase {
                statement_id: statement,
                expected_property_id: property,
                expected_direction: "forward",
                created_by: actor,
                origin: "person",
            },
        )
        .await?;
        let model = wiremock::MockServer::start().await;
        utopia_store::settings::upsert(
            &pool,
            workspace,
            Some(&model.uri()),
            None,
            Some("scripted-alignment"),
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
        let search = std::sync::Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        Ok(Some(Self {
            pool,
            state,
            model,
            org,
            kb,
            case,
            property,
            _dir: dir,
        }))
    }

    async fn reply(&self, text: &str, count: u64) {
        let frame = serde_json::json!({"choices":[{"delta":{"content":text}}]});
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(format!("data: {frame}\n\ndata: [DONE]\n\n")),
            )
            .expect(count)
            .mount(&self.model)
            .await;
    }

    async fn case(&self) -> anyhow::Result<utopia_store::ontology_regressions::Case> {
        Ok(
            utopia_store::ontology_regressions::get(&self.pool, self.kb, self.case)
                .await?
                .expect("the confirmed case exists"),
        )
    }

    async fn edit_property(&self) -> anyhow::Result<()> {
        // Production freshness uses updated_at, so exercise the same edited-definition input.
        sqlx::query("UPDATE relation_types SET description='The object acquired the subject.',updated_at=clock_timestamp() WHERE id=$1")
            .bind(self.property).execute(&self.pool).await?;
        Ok(())
    }

    async fn finish(self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        result
    }
}

#[tokio::test]
async fn regression_observes_the_production_redecision_after_a_property_edit() -> anyhow::Result<()>
{
    let Some(f) = RegressionFixture::new().await? else {
        return Ok(());
    };
    let result = async {
        f.reply(r#"{"b":[[0,"acquired","forward"]]}"#, 2).await;
        align_phrases_reasking(&f.state, f.kb, 0).await?;
        let before = f.case().await?;
        let report = before.last_result.as_ref().unwrap();
        assert_eq!(report["passed"], true);
        assert_eq!(report["human_bound"], false);
        assert_eq!(f.model.received_requests().await.unwrap().len(), 2);
        f.model.reset().await;
        f.edit_property().await?;
        f.reply(r#"{"b":[[0,"acquired","reverse"]]}"#, 2).await;
        align_phrases_reasking(&f.state, f.kb, 0).await?;
        let after = f.case().await?;
        let report = after.last_result.unwrap();
        assert_eq!(report["passed"], false);
        assert_eq!(report["actual_property_id"], f.property.to_string());
        assert_eq!(report["actual_direction"], "reverse");
        assert!(after.last_checked_at > before.last_checked_at);
        let requests = f.model.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests {
            let body: serde_json::Value = request.body_json()?;
            assert!(body["messages"]
                .to_string()
                .contains("The object acquired the subject."));
        }
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn an_unreadable_model_response_does_not_refresh_a_cached_regression_result(
) -> anyhow::Result<()> {
    let Some(f) = RegressionFixture::new().await? else {
        return Ok(());
    };
    let result = async {
        f.reply(r#"{"b":[[0,"acquired","forward"]]}"#, 2).await;
        align_phrases_reasking(&f.state, f.kb, 0).await?;
        let before = f.case().await?;
        f.model.reset().await;
        f.edit_property().await?;
        f.reply("not a readable alignment response", 1).await;
        // Stop at the production retry budget; the failed call itself still follows the ordinary path.
        align_phrases_reasking(&f.state, f.kb, MAX_REASK).await?;
        let after = f.case().await?;
        assert_eq!(after.last_checked_at, before.last_checked_at);
        assert_eq!(after.last_result, before.last_result);
        assert_eq!(f.model.received_requests().await.unwrap().len(), 1);
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn a_failed_model_call_does_not_refresh_a_cached_regression_result() -> anyhow::Result<()> {
    let Some(f) = RegressionFixture::new().await? else {
        return Ok(());
    };
    let result = async {
        f.reply(r#"{"b":[[0,"acquired","forward"]]}"#, 2).await;
        align_phrases_reasking(&f.state, f.kb, 0).await?;
        let before = f.case().await?;
        f.model.reset().await;
        f.edit_property().await?;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/chat/completions"))
            // A non-retryable endpoint error exercises transport failure without timing assumptions.
            .respond_with(wiremock::ResponseTemplate::new(401))
            .expect(1)
            .mount(&f.model)
            .await;
        align_phrases_reasking(&f.state, f.kb, MAX_REASK).await?;
        let after = f.case().await?;
        assert_eq!(after.last_checked_at, before.last_checked_at);
        assert_eq!(after.last_result, before.last_result);
        assert_eq!(f.model.received_requests().await.unwrap().len(), 1);
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn a_protected_human_binding_is_explicitly_reported_without_a_model_call(
) -> anyhow::Result<()> {
    let Some(f) = RegressionFixture::new().await? else {
        return Ok(());
    };
    let result=async {
        let signature=phrase_bindings::signatures(&f.pool,f.kb).await?.remove(0);
        phrase_bindings::decide(&f.pool,f.kb,&signature,Decision {
            relation_type_id:Some(f.property),direction:Some("reverse"),status:"bound",
            votes:&serde_json::json!({"reason":"human review"}),decided_by:"person",
            basis:None,marks:None,marks_asked:false,
        }).await?;
        // Remove the report so this also proves the aligner captures a skipped human decision.
        sqlx::query("UPDATE ontology_regression_cases SET last_checked_at=NULL,last_result=NULL WHERE id=$1")
            .bind(f.case).execute(&f.pool).await?;
        f.edit_property().await?;
        align_phrases_reasking(&f.state,f.kb,0).await?;
        let report=f.case().await?.last_result.unwrap();
        assert_eq!(report["passed"],true);
        assert_eq!(report["human_bound"],true);
        assert_eq!(report["actual_direction"],"reverse");
        assert_eq!(f.model.received_requests().await.unwrap().len(),0);
        Ok(())
    }.await;
    f.finish(result).await
}
