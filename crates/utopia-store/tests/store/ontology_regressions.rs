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
            statement_id, expected_property_id: Some(property), expected_direction: Some("forward"),
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

struct Fx {
    pool: PgPool,
    org: Uuid,
    kb: Uuid,
    actor: Uuid,
    subject: Uuid,
    object: Uuid,
    property: Uuid,
}
impl Fx {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
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
        sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'regression-cases')")
            .bind(workspace)
            .bind(org)
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'regression-cases')",
        )
        .bind(kb)
        .bind(workspace)
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES($1,$2,$3,'fixture','Reviewer')")
            .bind(actor).bind(org).bind(format!("{actor}@example.test")).execute(&pool).await?;
        for (id, name) in [(subject, "A"), (object, "B")] {
            sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES($1,$2,$3)")
                .bind(id)
                .bind(kb)
                .bind(name)
                .execute(&pool)
                .await?;
        }
        sqlx::query(
            "INSERT INTO relation_types(id,kb_id,key,label) VALUES($1,$2,'acquired','acquired')",
        )
        .bind(property)
        .bind(kb)
        .execute(&pool)
        .await?;
        Ok(Some(Self {
            pool,
            org,
            kb,
            actor,
            subject,
            object,
            property,
        }))
    }
}

#[tokio::test]
async fn negative_cases_fail_on_human_bindings_and_pass_only_explicit_none() -> anyhow::Result<()> {
    let Some(Fx {
        pool,
        org,
        kb,
        actor,
        subject,
        object,
        property,
    }) = Fx::new().await?
    else {
        return Ok(());
    };
    let result = async {
        let statement_id = graph::insert_open_statement(
            &pool, kb, subject, "acquired", graph::FactObject::Entity(object), None, 1.0,
        ).await?.0;
        let case_id = regressions::add(&pool, kb, regressions::NewCase {
            statement_id, expected_property_id: Some(property), expected_direction: Some("forward"),
            created_by: actor, origin: "person",
        }).await?;
        // Negative expectations compare the same committed ledger, including human choices.
        let negative_statement = graph::insert_open_statement(
            &pool, kb, subject, "reported alongside", graph::FactObject::Entity(object), None, 1.0,
        ).await?.0;
        let negative = || regressions::NewCase { statement_id: negative_statement,
            expected_property_id: None, expected_direction: None, created_by: actor, origin: "person" };
        let negative_id = regressions::add(&pool, kb, negative()).await?;
        let unchecked = regressions::get(&pool, kb, negative_id).await?.unwrap();
        assert_eq!(unchecked.expected_binding, "none");
        assert!(unchecked.last_result.is_none() && unchecked.last_checked_at.is_none());
        let sig = phrase_bindings::signatures(&pool, kb).await?.into_iter()
            .find(|s| s.phrase == "reported alongside").unwrap();
        for (status, by, passed, conflict) in [
            ("undecided", "agent", false, false), ("bound", "agent", false, false),
            ("none", "agent", true, false), ("bound", "person", false, true),
            ("none", "person", true, false),
        ] {
            let bound = status == "bound";
            phrase_bindings::decide(&pool, kb, &sig, phrase_bindings::Decision {
                relation_type_id: bound.then_some(property), direction: bound.then_some("forward"),
                status, decided_by: by, votes: &serde_json::Value::Null, basis: None,
                marks: None, marks_asked: false,
            }).await?;
            regressions::record_human(&pool, kb).await?;
            let result = regressions::get(&pool, kb, negative_id).await?.unwrap().last_result.unwrap();
            assert_eq!(result["passed"], passed);
            assert_eq!(result["human_bound"], false);
            assert_eq!(result["conflict"], if conflict { serde_json::json!("human_binding") } else { serde_json::Value::Null });
            assert_eq!(result["actual_property_id"], serde_json::json!(bound.then_some(property)));
            let stored: (String, Option<Uuid>, String) = sqlx::query_as(
                "SELECT status,relation_type_id,decided_by FROM phrase_bindings WHERE kb_id=$1 AND phrase=$2"
            ).bind(kb).bind(&sig.phrase).fetch_one(&pool).await?;
            assert_eq!(stored, (status.into(), bound.then_some(property), by.into()), "comparison never rewrites a binding");
        }
        // A new example of an already decided signature reports the original decision time.
        let later_statement = graph::insert_open_statement(
            &pool, kb, object, "reported alongside", graph::FactObject::Entity(subject), None, 1.0,
        ).await?.0;
        let later = regressions::add(&pool, kb, regressions::NewCase { statement_id: later_statement, ..negative() }).await?;
        let original_time: bool = sqlx::query_scalar("SELECT c.last_checked_at=b.decided_at AND (c.last_result->>'decided_at')::timestamptz=b.decided_at
            FROM ontology_regression_cases c JOIN phrase_bindings b ON b.kb_id=c.kb_id AND b.phrase='reported alongside' WHERE c.id=$1")
            .bind(later).fetch_one(&pool).await?;
        assert!(original_time);
        let mut tx = pool.begin().await?;
        utopia_store::export::provenance_integrity(&mut tx, kb).await?;
        tx.rollback().await?;
        sqlx::query("DELETE FROM relation_types WHERE id=$1").bind(property).execute(&pool).await?;
        assert!(regressions::get(&pool, kb, case_id).await?.is_none());
        assert!(regressions::get(&pool, kb, negative_id).await?.is_some());
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}

#[tokio::test]
async fn confirmations_deduplicate_without_vetoing_opposite_expectations() -> anyhow::Result<()> {
    let Some(Fx {
        pool,
        org,
        kb,
        actor,
        subject,
        object,
        property,
    }) = Fx::new().await?
    else {
        return Ok(());
    };
    let result = async {

        let statement = graph::insert_open_statement(&pool, kb, subject, "alongside",
            graph::FactObject::Entity(object), None, 1.0).await?.0;
        let other = Uuid::now_v7();
        sqlx::query("INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES($1,$2,$3,'fixture','Other')")
            .bind(other).bind(org).bind(format!("{other}@example.test")).execute(&pool).await?;
        let case = |statement_id, negative: bool, created_by, origin| regressions::NewCase {
            statement_id, expected_property_id: (!negative).then_some(property),
            expected_direction: (!negative).then_some("forward"), created_by, origin,
        };
        let first = regressions::add(&pool, kb, case(statement, true, actor, "person")).await?;
        let (a,b) = tokio::join!(regressions::add(&pool,kb,case(statement,true,other,"person")),
                                regressions::add(&pool,kb,case(statement,true,actor,"person")));
        assert_eq!((a?, b?), (first, first));
        let saved = regressions::get(&pool,kb,first).await?.unwrap();
        assert_eq!(saved.created_by,Some(actor)); assert_eq!(saved.origin,"person");
        let fresh = graph::insert_open_statement(&pool,kb,subject,"first confirmations",
            graph::FactObject::Entity(object),None,1.0).await?.0;
        let (a,b) = tokio::join!(regressions::add(&pool,kb,case(fresh,true,actor,"person")),
            regressions::add(&pool,kb,case(fresh,true,other,"person")));
        let a = a?; assert_eq!(a,b?);
        let winner = regressions::get(&pool,kb,a).await?.unwrap();
        assert!((winner.created_by==Some(actor) && winner.origin=="person") ||
                (winner.created_by==Some(other) && winner.origin=="person"));
        // Cases observe decisions: opposite expectations can coexist in either order
        // and under concurrent confirmation, without rewriting either person's case.
        for (i, first_negative) in [Some(true), Some(false), None].into_iter().enumerate() {
            let statement = graph::insert_open_statement(&pool,kb,subject,&format!("case {i}"),
                graph::FactObject::Entity(object),None,1.0).await?.0;
            if let Some(negative) = first_negative {
                regressions::add(&pool,kb,case(statement,negative,actor,"person")).await?;
                regressions::add(&pool,kb,case(statement,!negative,other,"person")).await?;
            } else {
                let (a,b) = tokio::join!(regressions::add(&pool,kb,case(statement,true,actor,"person")),
                    regressions::add(&pool,kb,case(statement,false,other,"person")));
                assert_ne!(a?, b?);
            }
            let count: i64 = sqlx::query_scalar("SELECT count(*) FROM ontology_regression_cases WHERE statement_id=$1")
                .bind(statement).fetch_one(&pool).await?;
            assert_eq!(count,2);
        }
        // Storage itself rejects half targets and duplicate NULL targets, not just the route.
        for (target,direction,code) in [(Some(property),None,"23514"),(None,Some("forward"),"23514"),(None,None,"23505")] {
            let err = sqlx::query("INSERT INTO ontology_regression_cases(id,kb_id,statement_id,expected_property_id,expected_direction,origin) VALUES($1,$2,$3,$4,$5,'person')")
                .bind(Uuid::now_v7()).bind(kb).bind(statement).bind(target).bind(direction).execute(&pool).await.unwrap_err();
            assert_eq!(err.as_database_error().and_then(|e|e.code()).as_deref(),Some(code));
        }
        anyhow::Ok(())
    }.await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
