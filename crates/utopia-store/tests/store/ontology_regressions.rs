use sqlx::PgPool;
use utopia_store::{graph, ontology_regressions as regressions, phrase_bindings};
use uuid::Uuid;

#[tokio::test]
async fn committed_decisions_report_matches_and_human_bindings() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
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
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'regression-cases')")
        .bind(org)
        .execute(&pool)
        .await?;
    let result = async {
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'regression-cases')")
            .bind(workspace).bind(org).execute(&pool).await?;
        sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'regression-cases')")
            .bind(kb).bind(workspace).execute(&pool).await?;
        sqlx::query("INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES($1,$2,$3,'fixture','Reviewer')")
            .bind(actor).bind(org).bind(format!("{actor}@example.test")).execute(&pool).await?;
        for (id, name) in [(subject, "A"), (object, "B")] {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)")
                .bind(id).bind(kb).bind(name).execute(&pool).await?;
        }
        sqlx::query("INSERT INTO relation_types(id,kb_id,key,label) VALUES($1,$2,'acquired','acquired')")
            .bind(property).bind(kb).execute(&pool).await?;
        let statement_id = graph::insert_open_statement(
            &pool, kb, subject, "acquired", graph::FactObject::Entity(object), None, 1.0,
        ).await?.0;
        let case_id = regressions::add(&pool, kb, regressions::NewCase {
            statement_id, expected_property_id: property, expected_direction: "forward",
            created_by: actor, origin: "person",
        }).await?;
        let signature = phrase_bindings::signatures(&pool, kb).await?.remove(0);
        for (direction, decided_by, passed, human_bound) in [
            ("forward", "agent", true, false),
            ("reverse", "agent", false, false),
            ("reverse", "person", true, true),
        ] {
            phrase_bindings::decide(&pool, kb, &signature, phrase_bindings::Decision {
                relation_type_id: Some(property), direction: Some(direction), status: "bound",
                decided_by, votes: &serde_json::Value::Null, basis: None,
                marks: None, marks_asked: false,
            }).await?;
            let case = regressions::get(&pool, kb, case_id).await?.unwrap();
            let report = case.last_result.unwrap();
            anyhow::ensure!(report["passed"] == passed);
            anyhow::ensure!(report["human_bound"] == human_bound);
            anyhow::ensure!(report["actual_direction"] == direction);
            anyhow::ensure!(case.last_checked_at.is_some());
        }
        Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
