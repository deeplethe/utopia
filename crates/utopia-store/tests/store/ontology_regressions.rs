use sqlx::PgPool;
use utopia_store::{graph, ontology_regressions as regressions, phrase_bindings};
use uuid::Uuid;

struct Fixture {
    pool: PgPool,
    org: Uuid,
    kb: Uuid,
    other_kb: Uuid,
    actor: Uuid,
    other_actor: Uuid,
    statement: Uuid,
    value_statement: Uuid,
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
        let (org, workspace, kb, other_kb, actor, other_actor) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'regression-cases')")
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'regression-cases')")
            .bind(workspace)
            .bind(org)
            .execute(&pool)
            .await?;
        for base in [kb, other_kb] {
            sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'regression-cases')").bind(base).bind(workspace).execute(&pool).await?;
        }
        for user in [actor, other_actor] {
            sqlx::query("INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES($1,$2,$3,'fixture','Reviewer')")
                .bind(user).bind(org).bind(format!("{user}@example.test")).execute(&pool).await?;
        }
        let (subject, object) = (Uuid::now_v7(), Uuid::now_v7());
        for (id, name) in [(subject, "A"), (object, "B")] {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)")
                .bind(id)
                .bind(kb)
                .bind(name)
                .execute(&pool)
                .await?;
        }
        let (property, attribute, foreign_property) =
            (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        for (id, base, key, kind, datatype) in [
            (property, kb, "acquired", "relation", None),
            (attribute, kb, "amount", "attribute", Some("number")),
            (foreign_property, other_kb, "acquired", "relation", None),
        ] {
            sqlx::query("INSERT INTO relation_types(id,kb_id,key,label,kind,datatype) VALUES($1,$2,$3,$3,$4,$5)")
                .bind(id).bind(base).bind(key).bind(kind).bind(datatype).execute(&pool).await?;
        }
        let statement = graph::insert_open_statement(
            &pool,
            kb,
            subject,
            " ACQUIRED ",
            graph::FactObject::Entity(object),
            None,
            1.0,
        )
        .await?
        .0;
        let value = serde_json::json!({"type":"number","value":42});
        let value_statement = graph::insert_open_statement(
            &pool,
            kb,
            subject,
            "has amount",
            graph::FactObject::Value(&value),
            None,
            1.0,
        )
        .await?
        .0;
        Ok(Some(Self {
            pool,
            org,
            kb,
            other_kb,
            actor,
            other_actor,
            statement,
            value_statement,
            property,
            attribute,
            foreign_property,
        }))
    }
    fn input(&self) -> regressions::NewCase<'_> {
        regressions::NewCase {
            statement_id: self.statement,
            expected_property_id: self.property,
            expected_direction: "forward",
            created_by: self.actor,
            origin: "person",
        }
    }
    async fn signature(&self) -> anyhow::Result<phrase_bindings::PhraseSignature> {
        Ok(phrase_bindings::signatures(&self.pool, self.kb)
            .await?
            .into_iter()
            .find(|s| s.phrase == "acquired")
            .unwrap())
    }
    async fn finish(self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        result
    }
}

fn decision<'a>(
    property: Option<Uuid>,
    direction: Option<&'a str>,
    status: &'a str,
    actor: &'a str,
) -> phrase_bindings::Decision<'a> {
    phrase_bindings::Decision {
        relation_type_id: property,
        direction,
        status,
        decided_by: actor,
        votes: &serde_json::Value::Null,
        basis: Some("fixture-basis"),
        marks: None,
        marks_asked: false,
    }
}

#[tokio::test]
async fn repeated_and_concurrent_confirmation_preserves_first_actor_and_origin(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let id = regressions::add(&f.pool, f.kb, f.input()).await?;
        let mut duplicate = f.input();
        duplicate.created_by = f.other_actor;
        duplicate.origin = "adoption";
        anyhow::ensure!(regressions::add(&f.pool, f.kb, duplicate).await? == id);
        let (a, b) = tokio::join!(
            regressions::add(&f.pool, f.kb, f.input()),
            regressions::add(&f.pool, f.kb, f.input())
        );
        anyhow::ensure!(a? == id && b? == id);
        let case = regressions::get(&f.pool, f.kb, id).await?.unwrap();
        anyhow::ensure!(case.created_by == Some(f.actor) && case.origin == "person");
        anyhow::ensure!(regressions::list(&f.pool, f.kb).await?.len() == 1);
        anyhow::ensure!(
            case.last_checked_at.is_none() && case.last_result.is_none(),
            "No binding is not a fabricated check"
        );
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn the_database_and_api_keep_every_reference_in_the_same_kb() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let id = regressions::add(&f.pool, f.kb, f.input()).await?;
        anyhow::ensure!(regressions::get(&f.pool, f.other_kb, id).await?.is_none());
        let mut cross = f.input();
        cross.expected_property_id = f.foreign_property;
        anyhow::ensure!(regressions::add(&f.pool, f.kb, cross).await.is_err());
        anyhow::ensure!(regressions::add(&f.pool, f.other_kb, f.input())
            .await
            .is_err());
        anyhow::ensure!(sqlx::query(
            "UPDATE ontology_regression_cases SET expected_property_id=$2 WHERE id=$1"
        )
        .bind(id)
        .bind(f.foreign_property)
        .execute(&f.pool)
        .await
        .is_err());
        anyhow::ensure!(
            sqlx::query("UPDATE ontology_regression_cases SET kb_id=$2 WHERE id=$1")
                .bind(id)
                .bind(f.other_kb)
                .execute(&f.pool)
                .await
                .is_err()
        );
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn any_live_open_statement_is_allowed_and_value_expectations_fit_the_object(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        // No quote or evidence is required: the human is choosing an existing statement.
        regressions::add(&f.pool, f.kb, f.input()).await?;
        let value = regressions::NewCase {
            statement_id: f.value_statement,
            expected_property_id: f.attribute,
            expected_direction: "forward",
            created_by: f.actor,
            origin: "person",
        };
        regressions::add(&f.pool, f.kb, value).await?;
        let mut wrong = f.input();
        wrong.expected_property_id = f.attribute;
        anyhow::ensure!(regressions::add(&f.pool, f.kb, wrong).await.is_err());
        let reverse = regressions::NewCase {
            statement_id: f.value_statement,
            expected_property_id: f.attribute,
            expected_direction: "reverse",
            created_by: f.actor,
            origin: "person",
        };
        anyhow::ensure!(regressions::add(&f.pool, f.kb, reverse).await.is_err());
        sqlx::query("UPDATE facts SET invalidated_at=now() WHERE id=$1")
            .bind(f.statement)
            .execute(&f.pool)
            .await?;
        anyhow::ensure!(regressions::add(&f.pool, f.kb, f.input()).await.is_err());
        anyhow::ensure!(
            regressions::list(&f.pool, f.kb).await?.len() == 1,
            "Invalidated sources are not offered as live regressions"
        );
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn production_decisions_compare_property_and_direction_without_touching_facts(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let id = regressions::add(&f.pool, f.kb, f.input()).await?;
        let mut sig = f.signature().await?;
        sig.phrase = "  ACQUIRED\t ".to_owned();
        let alternative = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO relation_types(id,kb_id,key,label) VALUES($1,$2,'supplies','supplies')",
        )
        .bind(alternative)
        .bind(f.kb)
        .execute(&f.pool)
        .await?;
        for (property, direction, status, passed) in [
            (Some(f.property), Some("forward"), "bound", true),
            (Some(f.property), Some("reverse"), "bound", false),
            (Some(alternative), Some("forward"), "bound", false),
            (None, None, "none", false),
            (None, None, "undecided", false),
        ] {
            phrase_bindings::decide(
                &f.pool,
                f.kb,
                &sig,
                decision(property, direction, status, "agent"),
            )
            .await?;
            let case = regressions::get(&f.pool, f.kb, id).await?.unwrap();
            let report = case.last_result.unwrap();
            anyhow::ensure!(
                report["passed"] == passed
                    && report["human_bound"] == false
                    && report["status"] == status
            );
            let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "SELECT decided_at FROM phrase_bindings WHERE kb_id=$1 AND phrase='acquired'",
            )
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
            anyhow::ensure!(case.last_checked_at.is_some_and(|checked| checked >= at));
            anyhow::ensure!(
                serde_json::from_value::<chrono::DateTime<chrono::Utc>>(
                    report["decided_at"].clone()
                )? == at
            );
        }
        let facts: i64 = sqlx::query_scalar("SELECT count(*) FROM facts WHERE kb_id=$1")
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
        anyhow::ensure!(
            facts == 2,
            "Comparing outcomes does not materialize or overwrite graph facts"
        );
        let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(f.kb.to_string())
            .fetch_one(&f.pool)
            .await?;
        anyhow::ensure!(
            jobs == 0,
            "Capturing an outcome must not start another alignment job"
        );
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn a_human_binding_is_protected_and_keeps_its_decision_time() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let sig = f.signature().await?;
        phrase_bindings::decide(
            &f.pool,
            f.kb,
            &sig,
            decision(Some(f.property), Some("reverse"), "bound", "person"),
        )
        .await?;
        let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT decided_at FROM phrase_bindings WHERE kb_id=$1 AND phrase='acquired'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        let id = regressions::add(&f.pool, f.kb, f.input()).await?;
        let case = regressions::get(&f.pool, f.kb, id).await?.unwrap();
        let report = case.last_result.unwrap();
        anyhow::ensure!(
            report["passed"] == true
                && report["human_bound"] == true
                && report["actual_direction"] == "reverse"
        );
        anyhow::ensure!(
            case.last_checked_at == Some(at),
            "A cached human choice is not a new check"
        );
        sqlx::query("UPDATE phrase_bindings SET decided_at=now()-interval '1 day' WHERE kb_id=$1")
            .bind(f.kb).execute(&f.pool).await?;
        anyhow::ensure!(
            !phrase_bindings::decide(
                &f.pool,
                f.kb,
                &sig,
                decision(Some(f.property), Some("forward"), "bound", "agent")
            )
            .await?
        );
        regressions::record_human(&f.pool, f.kb).await?;
        let checked = regressions::get(&f.pool, f.kb, id).await?.unwrap();
        anyhow::ensure!(checked.last_checked_at.is_some_and(|checked| checked > at));
        let decision_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT decided_at FROM phrase_bindings WHERE kb_id=$1 AND phrase='acquired'")
            .bind(f.kb).fetch_one(&f.pool).await?;
        anyhow::ensure!(serde_json::from_value::<chrono::DateTime<chrono::Utc>>(checked.last_result.unwrap()["decided_at"].clone())? == decision_at);
        phrase_bindings::decide(&f.pool, f.kb, &sig, decision(None, None, "none", "person")).await?;
        regressions::record_human(&f.pool, f.kb).await?;
        let rejected = regressions::get(&f.pool, f.kb, id).await?.unwrap().last_result.unwrap();
        anyhow::ensure!(rejected["passed"] == false && rejected["human_bound"] == false,
            "A person's explicit rejection is protected but is not a manual binding success");
        // The helper must not report cached agent bindings as newly checked.
        let agent_case = regressions::add(
            &f.pool,
            f.kb,
            regressions::NewCase {
                statement_id: f.value_statement,
                expected_property_id: f.attribute,
                expected_direction: "forward",
                created_by: f.actor,
                origin: "person",
            },
        )
        .await?;
        let value_sig=phrase_bindings::signatures(&f.pool,f.kb).await?.into_iter()
            .find(|s|s.phrase=="has amount").unwrap();
        phrase_bindings::decide(&f.pool,f.kb,&value_sig,
            decision(Some(f.attribute),Some("forward"),"bound","agent")).await?;
        let value_report = regressions::get(&f.pool, f.kb, agent_case).await?.unwrap().last_result.unwrap();
        anyhow::ensure!(value_report["passed"] == true && value_report["actual_property_id"] == f.attribute.to_string());
        // A case imported without a report must wait for a real agent decision, even if
        // an older cached agent binding exists when the human-only capture runs.
        sqlx::query("UPDATE ontology_regression_cases SET last_checked_at=NULL,last_result=NULL WHERE id=$1")
            .bind(agent_case).execute(&f.pool).await?;
        regressions::record_human(&f.pool, f.kb).await?;
        anyhow::ensure!(regressions::get(&f.pool, f.kb, agent_case)
            .await?
            .unwrap()
            .last_result
            .is_none());
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn adoption_and_decision_rollbacks_do_not_leave_partial_cases_or_reports(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let mut tx = f.pool.begin().await?;
        let mut adoption = f.input();
        adoption.origin = "adoption";
        regressions::add_on(&mut tx, f.kb, adoption).await?;
        tx.rollback().await?;
        anyhow::ensure!(regressions::list(&f.pool, f.kb).await?.is_empty());
        let id = regressions::add(&f.pool, f.kb, f.input()).await?;
        let sig = f.signature().await?;
        let mut tx = f.pool.begin().await?;
        phrase_bindings::decide_on(
            &mut tx,
            f.kb,
            &sig,
            decision(Some(f.property), Some("reverse"), "bound", "agent"),
        )
        .await?;
        let report: serde_json::Value =
            sqlx::query_scalar("SELECT last_result FROM ontology_regression_cases WHERE id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?;
        anyhow::ensure!(report["passed"] == false);
        tx.rollback().await?;
        anyhow::ensure!(regressions::get(&f.pool, f.kb, id)
            .await?
            .unwrap()
            .last_result
            .is_none());
        sqlx::query("DELETE FROM facts WHERE id=$1")
            .bind(f.statement)
            .execute(&f.pool)
            .await?;
        anyhow::ensure!(regressions::get(&f.pool, f.kb, id).await?.is_none());
        let value_id = regressions::add(
            &f.pool,
            f.kb,
            regressions::NewCase {
                statement_id: f.value_statement,
                expected_property_id: f.attribute,
                expected_direction: "forward",
                created_by: f.actor,
                origin: "person",
            },
        )
        .await?;
        sqlx::query("DELETE FROM relation_types WHERE id=$1")
            .bind(f.attribute)
            .execute(&f.pool)
            .await?;
        anyhow::ensure!(regressions::get(&f.pool, f.kb, value_id).await?.is_none());
        Ok(())
    }
    .await;
    f.finish(result).await
}

#[tokio::test]
async fn comparisons_do_not_cross_types_value_signatures_or_knowledge_bases() -> anyhow::Result<()>
{
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let id = regressions::add(&f.pool, f.kb, f.input()).await?;
        let sig = f.signature().await?;
        let class = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO entity_types(id,kb_id,key,label) VALUES($1,$2,'company','Company')",
        )
        .bind(class)
        .bind(f.kb)
        .execute(&f.pool)
        .await?;
        let mut typed = sig.clone();
        typed.subject_type_id = Some(class);
        phrase_bindings::decide(
            &f.pool,
            f.kb,
            &typed,
            decision(Some(f.property), Some("forward"), "bound", "agent"),
        )
        .await?;
        anyhow::ensure!(
            regressions::get(&f.pool, f.kb, id)
                .await?
                .unwrap()
                .last_result
                .is_none(),
            "A typed decision cannot check an untyped statement"
        );

        sqlx::query("UPDATE facts SET phrase='acquired' WHERE id=$1")
            .bind(f.value_statement)
            .execute(&f.pool)
            .await?;
        let value_id = regressions::add(
            &f.pool,
            f.kb,
            regressions::NewCase {
                statement_id: f.value_statement,
                expected_property_id: f.attribute,
                expected_direction: "forward",
                created_by: f.actor,
                origin: "person",
            },
        )
        .await?;
        let mut literal = sig.clone();
        literal.object_is_value = true;
        // decide_on deliberately drops object classes for literals; reports must use
        // that stored signature even when callers supply a meaningless object class.
        literal.object_type_id = Some(class);
        phrase_bindings::decide(
            &f.pool,
            f.kb,
            &literal,
            decision(Some(f.attribute), Some("forward"), "bound", "agent"),
        )
        .await?;
        anyhow::ensure!(
            regressions::get(&f.pool, f.kb, value_id)
                .await?
                .unwrap()
                .last_result
                .unwrap()["passed"]
                == true
        );
        anyhow::ensure!(
            regressions::get(&f.pool, f.kb, id)
                .await?
                .unwrap()
                .last_result
                .is_none(),
            "NULL object types must still distinguish an entity from a literal"
        );

        phrase_bindings::decide(
            &f.pool,
            f.other_kb,
            &sig,
            decision(Some(f.foreign_property), Some("forward"), "bound", "agent"),
        )
        .await?;
        anyhow::ensure!(
            regressions::get(&f.pool, f.kb, id)
                .await?
                .unwrap()
                .last_result
                .is_none(),
            "The same phrase in another knowledge base cannot check this case"
        );

        phrase_bindings::decide(
            &f.pool,
            f.kb,
            &sig,
            decision(Some(f.property), Some("reverse"), "bound", "agent"),
        )
        .await?;
        anyhow::ensure!(
            regressions::get(&f.pool, f.kb, id)
                .await?
                .unwrap()
                .last_result
                .unwrap()["passed"]
                == false
        );
        anyhow::ensure!(
            regressions::get(&f.pool, f.kb, value_id)
                .await?
                .unwrap()
                .last_result
                .unwrap()["passed"]
                == true
        );
        Ok(())
    }
    .await;
    f.finish(result).await
}
