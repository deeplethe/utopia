//! 0043 on the typed graph. These checks exercise the real ledger and its
//! temporal rewrites, including the failure cases that held PR #699 back.

use chrono::{TimeZone, Utc};
use serde_json::Value;
use sqlx::{postgres::PgPoolOptions, PgPool};
use utopia_store::{conflict_governance as agent, governance, temporal};
use uuid::Uuid;

struct Fx {
    pool: PgPool,
    org: Uuid,
    kb: Uuid,
    user: Uuid,
    subject: Uuid,
    property: Uuid,
    old: Uuid,
    new: Uuid,
    conflict: Uuid,
}

impl Fx {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        // One connection catches accidentally reacquiring the pool while a
        // graph transaction is held (the execution gate and audit use it too).
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await?;
        let [org, ws, kb, user, subject, property, a, b, old, new, conflict] =
            std::array::from_fn(|_| Uuid::now_v7());
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','conflict-governance');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','conflict-governance');
             INSERT INTO knowledge_bases(id,workspace_id,name,governance) VALUES ('{kb}','{ws}','conflicts',true);
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                ('{user}','{org}','{user}@conflict.test','Reviewer','unused');
             INSERT INTO entities(id,kb_id,canonical_name) VALUES
                ('{subject}','{kb}','Building'), ('{a}','{kb}','Tenant A'), ('{b}','{kb}','Tenant B');
             INSERT INTO relation_types(id,kb_id,key,label,temporal,functional) VALUES
                ('{property}','{kb}','tenant','tenant','state',true);"
        )).execute(&pool).await?;
        for (id, object, text, date) in [
            (
                old,
                a,
                "Tenant A occupies the building from January 1, 2020.",
                "2020-01-01",
            ),
            (
                new,
                b,
                "Tenant B became the tenant on June 1, 2021.",
                "2021-06-01",
            ),
        ] {
            let [doc, chunk, statement] = std::array::from_fn(|_| Uuid::now_v7());
            sqlx::raw_sql(&format!(
                "INSERT INTO documents(id,kb_id,filename,sha256,doc_time,doc_time_source)
                    VALUES ('{doc}','{kb}','{doc}.txt','{doc}','{date}','content');
                 INSERT INTO chunks(id,kb_id,document_id,seq,text)
                    VALUES ('{chunk}','{kb}','{doc}',0,'{text}');
                 INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase,attested_from)
                    VALUES ('{statement}','{kb}','{subject}','{object}','open','tenant','{date}');
                 INSERT INTO fact_evidence(fact_id,chunk_id,document_id,quote,quote_start,quote_end)
                    VALUES ('{statement}','{chunk}','{doc}','{text}',0,{length});
                 INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,confidence,
                                   valid_from,valid_from_precision,layer,from_statement_id,attested_from)
                    VALUES ('{id}','{kb}','{subject}','{property}','{object}',0.9,
                            '2020-01-01','day','typed','{statement}','{date}');
                 INSERT INTO typed_fact_sources(fact_id,statement_id) VALUES ('{id}','{statement}');",
                length = text.chars().count(),
            )).execute(&pool).await?;
        }
        sqlx::query(
            "INSERT INTO fact_conflicts(id,kb_id,old_fact_id,new_fact_id,reason)
            VALUES ($1,$2,$3,$4,'simultaneous')",
        )
        .bind(conflict)
        .bind(kb)
        .bind(old)
        .bind(new)
        .execute(&pool)
        .await?;
        Ok(Some(Self {
            pool,
            org,
            kb,
            user,
            subject,
            property,
            old,
            new,
            conflict,
        }))
    }

    async fn close(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
            .bind(self.kb)
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        self.pool.close().await;
        Ok(())
    }

    async fn item(&self) -> anyhow::Result<agent::Case> {
        Ok(agent::load(&self.pool, self.kb, self.conflict)
            .await?
            .expect("open conflict"))
    }

    async fn decide(
        &self,
        action: &str,
        confidence: f32,
        params: agent::Parameters,
    ) -> anyhow::Result<agent::Recorded> {
        Ok(agent::record(
            &self.pool,
            self.kb,
            &self.item().await?,
            agent::Decision {
                run_id: Uuid::now_v7(),
                action,
                confidence,
                why: "the source states this",
                params,
                precedents: &[],
            },
        )
        .await?
        .expect("recorded decision"))
    }

    async fn live(&self) -> anyhow::Result<Vec<Value>> {
        Ok(sqlx::query_scalar(
            "SELECT jsonb_build_array(subject_id,predicate_id,object_id,
                valid_from,valid_from_precision,valid_to,valid_to_precision,confidence)
            FROM facts WHERE kb_id = $1 AND layer = 'typed' AND invalidated_at IS NULL
            ORDER BY subject_id, predicate_id, object_id, valid_from NULLS FIRST",
        )
        .bind(self.kb)
        .fetch_all(&self.pool)
        .await?)
    }
}

fn date() -> agent::Parameters {
    agent::Parameters {
        date: Some(Utc.with_ymd_and_hms(2021, 6, 1, 0, 0, 0).unwrap()),
        precision: Some("day".into()),
    }
}

#[tokio::test]
async fn a_retimed_typed_fact_and_its_predecessor_return_and_are_not_judged_again(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        let before = f.live().await?;
        let item = f.item().await?;
        assert_eq!(
            item.new_evidence.len(),
            1,
            "typed evidence is read through source statements"
        );
        assert!(item.new_evidence[0]
            .quote
            .as_deref()
            .unwrap()
            .contains("June 1, 2021"));
        let d = f.decide("retime_new", 0.95, date()).await?;
        assert!(d.applied);
        let live = f.live().await?;
        assert_ne!(live, before);
        assert_eq!(live.len(), 2);
        let view = governance::get(&f.pool, f.kb, d.id).await?;
        assert!(view.summary.as_deref().unwrap().contains("Tenant B"));
        assert_eq!(view.status, "applied");
        agent::answer(
            &f.pool,
            f.kb,
            d.id,
            "revert",
            Default::default(),
            f.user,
            Some("keep the earlier reading"),
        )
        .await?;
        assert_eq!(
            f.live().await?,
            before,
            "undo restores both intervals, not just the target row"
        );
        assert!(
            agent::queue(&f.pool, f.kb, 20).await?.is_empty(),
            "the rewritten conflict's lineage remembers the revert"
        );
        let events = agent::precedents(&f.pool, f.kb).await?;
        assert_eq!(
            events.len(),
            1,
            "the agent's action is not its own precedent"
        );
        assert_eq!(events[0]["action"], "conflict.revert");
        assert_eq!(events[0]["detail"]["new_object"], "Tenant B");
        assert_eq!(
            governance::get(&f.pool, f.kb, d.id).await?.status,
            "reverted"
        );
        anyhow::Ok(())
    }
    .await;
    f.close().await?;
    run
}

#[tokio::test]
async fn keep_close_and_reject_have_real_inverses() -> anyhow::Result<()> {
    for action in ["keep_both", "close_old", "reject_new"] {
        let Some(f) = Fx::new().await? else {
            return Ok(());
        };
        let run = async {
            if action == "close_old" {
                sqlx::query(
                    "UPDATE facts SET valid_from = NULL, valid_from_precision = NULL WHERE id = $1",
                )
                .bind(f.new)
                .execute(&f.pool)
                .await?;
                sqlx::query("UPDATE fact_conflicts SET reason = 'no_time' WHERE id = $1")
                    .bind(f.conflict)
                    .execute(&f.pool)
                    .await?;
                temporal::reconcile_moved_facts(&f.pool, f.kb, &[f.old, f.new]).await?;
                assert_eq!(f.item().await?.old.valid_to_precision.as_deref(), Some("unknown"),
                    "the engine has ended the predecessor at an unknown date; the agent supplies the evidenced date");
            }
            let before = f.live().await?;
            let d = f
                .decide(
                    action,
                    0.95,
                    if action == "close_old" {
                        date()
                    } else {
                        Default::default()
                    },
                )
                .await?;
            assert!(d.applied, "{action}");
            let pending: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM fact_conflicts WHERE kb_id = $1 AND status = 'open'",
            )
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
            assert_eq!(pending, 0);
            agent::answer(
                &f.pool,
                f.kb,
                d.id,
                "revert",
                Default::default(),
                f.user,
                None,
            )
            .await?;
            assert_eq!(f.live().await?, before, "{action}");
            let pending: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM fact_conflicts WHERE kb_id = $1 AND status = 'open'",
            )
            .bind(f.kb)
            .fetch_one(&f.pool)
            .await?;
            assert_eq!(pending, 1, "{action}");
            assert!(agent::queue(&f.pool, f.kb, 20).await?.is_empty());
            anyhow::Ok(())
        }
        .await;
        f.close().await?;
        run?;
    }
    Ok(())
}

#[tokio::test]
async fn rejecting_one_successor_can_restore_every_conflict_it_removed() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        let third = Uuid::now_v7();
        let tenant = Uuid::now_v7();
        sqlx::query("INSERT INTO entities(id,kb_id,canonical_name) VALUES ($1,$2,'Tenant C')")
            .bind(tenant).bind(f.kb).execute(&f.pool).await?;
        sqlx::query("INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,valid_from,valid_from_precision)
            VALUES ($1,$2,$3,$4,$5,'2020-01-01','day')")
            .bind(third).bind(f.kb).bind(f.subject).bind(f.property).bind(tenant).execute(&f.pool).await?;
        temporal::reconcile_moved_facts(&f.pool, f.kb, &[f.old, f.new, third]).await?;
        let before: i64 = sqlx::query_scalar("SELECT count(*) FROM fact_conflicts WHERE kb_id = $1 AND status = 'open'")
            .bind(f.kb).fetch_one(&f.pool).await?;
        assert!(before >= 2);
        let d = f.decide("reject_new", 0.95, Default::default()).await?;
        assert!(d.applied);
        agent::answer(&f.pool, f.kb, d.id, "revert", Default::default(), f.user, None).await?;
        let after: i64 = sqlx::query_scalar("SELECT count(*) FROM fact_conflicts WHERE kb_id = $1 AND status = 'open'")
            .bind(f.kb).fetch_one(&f.pool).await?;
        assert_eq!(after, before, "undo must restore siblings as well as the target");
        for id in agent::queue(&f.pool, f.kb, 20).await? {
            let case = agent::load(&f.pool, f.kb, id).await?.unwrap();
            assert!(case.old.id != f.new && case.new.id != f.new,
                "a sibling conflict must not repeat the rejection the person just reverted");
        }
        anyhow::Ok(())
    }.await;
    f.close().await?;
    run
}

#[tokio::test]
async fn a_late_model_result_cannot_overrule_a_person() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        let item = f.item().await?;
        agent::resolve(
            &f.pool,
            f.kb,
            f.conflict,
            "keep_both",
            Default::default(),
            f.user,
            Some("joint tenants"),
        )
        .await?;
        let got = agent::record(
            &f.pool,
            f.kb,
            &item,
            agent::Decision {
                run_id: Uuid::now_v7(),
                action: "reject_new",
                confidence: 0.99,
                why: "late reply",
                params: Default::default(),
                precedents: &[],
            },
        )
        .await?;
        assert!(got.is_none());
        assert_eq!(f.live().await?.len(), 2);
        assert_eq!(
            agent::precedents(&f.pool, f.kb).await?[0]["detail"]["why"],
            "joint tenants"
        );
        anyhow::Ok(())
    }
    .await;
    f.close().await?;
    run
}

#[tokio::test]
async fn a_later_rewrite_makes_revert_fail_without_counting_a_revert() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        let d = f.decide("retime_new", 0.95, date()).await?;
        let corrected: Uuid = sqlx::query_scalar(
            "SELECT id FROM facts WHERE supersedes = $1 AND invalidated_at IS NULL",
        )
        .bind(f.new)
        .fetch_one(&f.pool)
        .await?;
        temporal::correct_interval(
            &f.pool,
            corrected,
            utopia_store::graph::Validity {
                from: Some(Utc.with_ymd_and_hms(2022, 7, 1, 0, 0, 0).unwrap()),
                from_precision: Some("day"),
                from_grade: None,
                to: None,
                to_precision: None,
                attested_at: None,
            },
        )
        .await?;
        let before = f.live().await?;
        assert!(agent::answer(
            &f.pool,
            f.kb,
            d.id,
            "revert",
            Default::default(),
            f.user,
            None
        )
        .await
        .is_err());
        assert_eq!(f.live().await?, before);
        assert_eq!(
            governance::get(&f.pool, f.kb, d.id).await?.status,
            "applied"
        );
        let reverted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM agent_decisions WHERE kb_id = $1 AND status = 'reverted'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        assert_eq!(reverted, 0);
        anyhow::Ok(())
    }
    .await;
    f.close().await?;
    run
}

#[tokio::test]
async fn invalid_confidence_and_unsupported_dates_are_proposals() -> anyhow::Result<()> {
    for bad_confidence in [true, false] {
        let Some(f) = Fx::new().await? else {
            return Ok(());
        };
        let run = async {
            let before = f.live().await?;
            let mut params = date();
            params.date = Some(Utc.with_ymd_and_hms(2099, 6, 1, 0, 0, 0).unwrap());
            let d = f
                .decide(
                    if bad_confidence {
                        "reject_new"
                    } else {
                        "retime_new"
                    },
                    if bad_confidence { 60.0 } else { 0.99 },
                    params,
                )
                .await?;
            assert!(!d.applied);
            assert_eq!(f.live().await?, before);
            let row = governance::get(&f.pool, f.kb, d.id).await?;
            assert_eq!(row.status, "proposed");
            if bad_confidence {
                assert_eq!(row.action, "unsure");
                assert_eq!(row.confidence, 0.0)
            }
            anyhow::Ok(())
        }
        .await;
        f.close().await?;
        run?;
    }
    Ok(())
}

#[tokio::test]
async fn the_ordinary_card_answers_the_proposal_and_records_the_persons_why() -> anyhow::Result<()>
{
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        let d = f.decide("reject_new", 0.4, Default::default()).await?;
        assert!(!d.applied);
        agent::resolve(
            &f.pool,
            f.kb,
            f.conflict,
            "keep_both",
            Default::default(),
            f.user,
            Some("different units of space"),
        )
        .await?;
        let row = governance::get(&f.pool, f.kb, d.id).await?;
        assert_eq!(row.status, "overridden");
        assert_eq!(row.decided_by_name.as_deref(), Some("Reviewer"));
        let p = agent::precedents(&f.pool, f.kb).await?;
        assert_eq!(p[0]["action"], "conflict.keep_both");
        assert_eq!(p[0]["detail"]["old_subject"], "Building");
        assert_eq!(p[0]["detail"]["why"], "different units of space");
        anyhow::Ok(())
    }
    .await;
    f.close().await?;
    run
}

#[tokio::test]
async fn changed_fact_or_property_inputs_invalidate_an_inflight_verdict() -> anyhow::Result<()> {
    for change in ["fact", "property", "neighbour"] {
        let Some(f) = Fx::new().await? else {
            return Ok(());
        };
        let run = async {
            let item = f.item().await?;
            match change {
                "property" => {
                    sqlx::query("UPDATE relation_types SET description = 'a revised scope of tenancy' WHERE id = $1")
                        .bind(f.property).execute(&f.pool).await?;
                }
                "neighbour" => {
                    sqlx::query("INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,layer,valid_from,valid_from_precision)
                        VALUES ($1,$2,$3,$4,$5,'typed','2025-01-01','day')")
                        .bind(Uuid::now_v7()).bind(f.kb).bind(f.subject).bind(f.property)
                        .bind(item.new.object_id).execute(&f.pool).await?;
                }
                _ => {
                    sqlx::query("UPDATE facts SET confidence = 1.0 WHERE id = $1")
                        .bind(f.new).execute(&f.pool).await?;
                }
            }
            let got = agent::record(&f.pool, f.kb, &item, agent::Decision {
                run_id: Uuid::now_v7(), action: "reject_new", confidence: 0.99, why: "old inputs",
                params: Default::default(), precedents: &[],
            }).await?;
            assert!(got.is_none(), "changed {change} was part of the model's input");
            assert!(f.item().await?.new.invalidated_at.is_none());
            assert!(governance::list(&f.pool, f.kb, 10, 0).await?.is_empty());
            anyhow::Ok(())
        }.await;
        f.close().await?;
        run?;
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_decision_insert_rolls_the_graph_change_back() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let name = format!("conflict_write_failure_{}", Uuid::now_v7().simple());
    // Only generated identifiers/UUIDs enter this fixture SQL. The trigger
    // affects this KB while unrelated store tests continue in parallel.
    sqlx::raw_sql(&format!(
        "CREATE FUNCTION {name}() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN RAISE EXCEPTION 'injected decision write failure'; END $$;
        CREATE TRIGGER {name} BEFORE INSERT ON agent_decisions FOR EACH ROW
          WHEN (NEW.kb_id = '{}') EXECUTE FUNCTION {name}();",
        f.kb
    ))
    .execute(&f.pool)
    .await?;
    let run = async {
        let before = f.live().await?;
        assert!(f.decide("retime_new", 0.95, date()).await.is_err());
        assert_eq!(f.live().await?, before);
        assert_eq!(f.item().await?.new.id, f.new);
        assert!(governance::list(&f.pool, f.kb, 10, 0).await?.is_empty());
        anyhow::Ok(())
    }
    .await;
    sqlx::raw_sql(&format!(
        "DROP TRIGGER {name} ON agent_decisions; DROP FUNCTION {name}();"
    ))
    .execute(&f.pool)
    .await?;
    f.close().await?;
    run
}

#[tokio::test]
async fn a_memory_statement_cannot_be_changed_by_an_automatic_conflict_decision(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        // The model sees eight passages. Put a memory source outside that
        // excerpt window: provenance protections must still see it.
        for _ in 0..8 {
            let [document, chunk] = std::array::from_fn(|_| Uuid::now_v7());
            sqlx::query("INSERT INTO documents(id,kb_id,filename,sha256) VALUES ($1,$2,'source.txt',$1::text)")
                .bind(document).bind(f.kb).execute(&f.pool).await?;
            sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ($1,$2,$3,0,'Another tenancy statement.')")
                .bind(chunk).bind(f.kb).bind(document).execute(&f.pool).await?;
            sqlx::query("INSERT INTO fact_evidence(fact_id,chunk_id,document_id,quote)
                SELECT from_statement_id,$2,$3,'Another tenancy statement.' FROM facts WHERE id = $1")
                .bind(f.new).bind(chunk).bind(document).execute(&f.pool).await?;
        }
        let document: Uuid = sqlx::query_scalar("SELECT e.document_id FROM fact_evidence e
            JOIN typed_fact_sources s ON s.statement_id = e.fact_id WHERE s.fact_id = $1
            ORDER BY e.document_id DESC LIMIT 1")
            .bind(f.new).fetch_one(&f.pool).await?;
        let source = utopia_store::memory::get_or_create_memory_source(&f.pool, f.kb).await?;
        sqlx::query("UPDATE documents SET source_id = $1 WHERE id = $2")
            .bind(source)
            .bind(document)
            .execute(&f.pool)
            .await?;
        let before = f.live().await?;
        assert_eq!(f.item().await?.new_evidence.len(), 8);
        assert!(f.item().await?.new_evidence.iter().all(|e| !e.memory));
        let d = f.decide("reject_new", 0.99, Default::default()).await?;
        assert!(!d.applied);
        assert_eq!(f.live().await?, before);
        assert!(governance::get(&f.pool, f.kb, d.id)
            .await?
            .reason
            .unwrap()
            .contains("memory statement"));
        anyhow::Ok(())
    }
    .await;
    f.close().await?;
    run
}

#[tokio::test]
async fn changed_proposals_expire_and_return_the_stable_conflict_error() -> anyhow::Result<()> {
    for withdrawn in [false, true] {
        let Some(f) = Fx::new().await? else {
            return Ok(());
        };
        let run = async {
            let d = f.decide("keep_both", 0.4, Default::default()).await?;
            if withdrawn {
                temporal::retract(&f.pool, f.kb, f.new).await?;
            } else {
                sqlx::query("UPDATE relation_types SET description = 'different tenant scope' WHERE id = $1")
                    .bind(f.property).execute(&f.pool).await?;
            }
            let before = f.live().await?;
            let error = agent::answer(&f.pool, f.kb, d.id, "keep_both", Default::default(), f.user, None)
                .await.unwrap_err();
            assert!(matches!(error, utopia_core::AppError::CodedConflict { code: "agent_inputs_changed", .. }));
            assert_eq!(f.live().await?, before);
            assert_eq!(governance::get(&f.pool, f.kb, d.id).await?.status, "superseded");
            assert!(agent::precedents(&f.pool, f.kb).await?.is_empty());
            anyhow::Ok(())
        }.await;
        f.close().await?;
        run?;
    }
    Ok(())
}
