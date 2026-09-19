//! Negative decisions depend on existing definitions as well as new candidates (#773).
use sqlx::PgPool;
use utopia_store::graph::{self, FactObject};
use utopia_store::{ontology, phrase_bindings as pb, type_bindings as tb};
use uuid::Uuid;

const WORD: &str = "candidate wording";

#[derive(Clone, Copy, Debug)]
enum BindingKind {
    KindWord,
    Phrase,
}

impl BindingKind {
    fn definitions(self) -> &'static str {
        match self {
            Self::KindWord => "entity_types",
            Self::Phrase => "relation_types",
        }
    }

    fn bindings(self) -> &'static str {
        match self {
            Self::KindWord => "type_bindings",
            Self::Phrase => "phrase_bindings",
        }
    }

    async fn stale(self, pool: &PgPool, kb: Uuid) -> anyhow::Result<bool> {
        Ok(match self {
            Self::KindWord => !tb::stale(pool, kb).await?.is_empty(),
            Self::Phrase => !pb::stale(pool, kb).await?.is_empty(),
        })
    }

    async fn decide(
        self,
        pool: &PgPool,
        kb: Uuid,
        chosen: Option<Uuid>,
        status: &str,
        actor: &str,
    ) -> anyhow::Result<bool> {
        let votes = serde_json::json!({});
        Ok(match self {
            Self::KindWord => {
                tb::decide(pool, kb, WORD, &[], chosen, status, &votes, actor).await?
            }
            Self::Phrase => {
                let sig = pb::signatures(pool, kb)
                    .await?
                    .into_iter()
                    .find(|s| s.phrase == WORD)
                    .expect("the open statement produces a signature");
                pb::decide(
                    pool,
                    kb,
                    &sig,
                    pb::Decision {
                        relation_type_id: chosen,
                        direction: chosen.map(|_| "forward"),
                        status,
                        votes: &votes,
                        decided_by: actor,
                    },
                )
                .await?
            }
        })
    }
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Edit,
    Add,
}

async fn lifecycle(
    kind: BindingKind,
    status: &str,
    change: Change,
    actor: &str,
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let org = Uuid::now_v7();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'negative-binding-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    let result = exercise(&pool, org, kind, status, change, actor).await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}

async fn exercise(
    pool: &PgPool,
    org: Uuid,
    kind: BindingKind,
    status: &str,
    change: Change,
    actor: &str,
) -> anyhow::Result<()> {
    let (ws, kb, other_kb, definition) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'binding-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    for base in [kb, other_kb] {
        sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'test')")
            .bind(base)
            .bind(ws)
            .execute(pool)
            .await?;
    }
    let table = kind.definitions();
    // Control the ordering without sleeps: creation < decision < the real edit below.
    sqlx::query(&format!(
        "INSERT INTO {table} (id, kb_id, key, label, description, created_at, updated_at)
         VALUES ($1, $2, 'candidate', 'candidate', 'old definition',
                 '2000-01-01', '2000-01-01')"
    ))
    .bind(definition)
    .bind(kb)
    .execute(pool)
    .await?;
    let (subject, object) = (Uuid::now_v7(), Uuid::now_v7());
    for entity in [subject, object] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, canonical_name, specific_type)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(entity)
        .bind(kb)
        .bind(entity.to_string())
        .bind(WORD)
        .execute(pool)
        .await?;
    }
    graph::insert_open_statement(
        pool,
        kb,
        subject,
        WORD,
        FactObject::Entity(object),
        None,
        0.9,
    )
    .await?;
    assert_eq!(tb::signatures(pool, kb).await?[0].kind_word, WORD);
    let chosen = (status == "bound").then_some(definition);
    assert!(kind.decide(pool, kb, chosen, status, actor).await?);
    sqlx::query(&format!(
        "UPDATE {} SET decided_at = '2000-01-02' WHERE kb_id = $1",
        kind.bindings()
    ))
    .bind(kb)
    .execute(pool)
    .await?;
    assert!(!kind.stale(pool, kb).await?, "unchanged inputs stay cached");

    // A newer candidate in another base must not invalidate this base's decisions.
    sqlx::query(&format!(
        "INSERT INTO {table} (id, kb_id, key, label) VALUES ($1, $2, 'other', 'other')"
    ))
    .bind(Uuid::now_v7())
    .bind(other_kb)
    .execute(pool)
    .await?;
    assert!(
        !kind.stale(pool, kb).await?,
        "invalidation is scoped to the KB"
    );

    match change {
        Change::Add => {
            sqlx::query(&format!(
                "INSERT INTO {table} (id, kb_id, key, label) VALUES ($1, $2, 'new', 'new')"
            ))
            .bind(Uuid::now_v7())
            .bind(kb)
            .execute(pool)
            .await?;
        }
        Change::Edit => match kind {
            BindingKind::KindWord => {
                ontology::update_entity_type(
                    pool,
                    kb,
                    definition,
                    "candidate",
                    None,
                    "circle",
                    &[],
                    "expanded definition",
                )
                .await?;
            }
            BindingKind::Phrase => {
                ontology::update_relation_type(
                    pool,
                    kb,
                    definition,
                    "candidate",
                    "state",
                    Default::default(),
                    "expanded definition",
                    None,
                    None,
                    None,
                    None,
                )
                .await?;
            }
        },
    }
    let (created_before, updated_after): (bool, bool) = sqlx::query_as(&format!(
        "SELECT created_at < '2000-01-02'::timestamptz,
                updated_at > '2000-01-02'::timestamptz FROM {table} WHERE id = $1"
    ))
    .bind(definition)
    .fetch_one(pool)
    .await?;
    assert!(created_before);
    assert_eq!(updated_after, matches!(change, Change::Edit));
    let stale = kind.stale(pool, kb).await?;
    eprintln!("DB: {kind:?} {status} {change:?} {actor}: stale={stale}");
    anyhow::ensure!(stale, "{kind:?} {status} should be stale after {change:?}");

    if actor == "person" {
        assert!(
            !kind
                .decide(pool, kb, Some(definition), "bound", "agent")
                .await?,
            "a stale person decision still rejects automatic replacement"
        );
        let (saved_status, saved_actor): (String, String) = sqlx::query_as(&format!(
            "SELECT status, decided_by FROM {} WHERE kb_id = $1",
            kind.bindings()
        ))
        .bind(kb)
        .fetch_one(pool)
        .await?;
        assert_eq!(
            (saved_status.as_str(), saved_actor.as_str()),
            (status, actor)
        );
    } else {
        // A fresh decision, including another negative one, must close the work again.
        assert!(kind.decide(pool, kb, chosen, status, actor).await?);
        assert!(
            !kind.stale(pool, kb).await?,
            "a fresh decision stays cached"
        );
    }
    Ok(())
}

#[tokio::test]
async fn new_candidates_and_bound_edits_still_invalidate() -> anyhow::Result<()> {
    for kind in [BindingKind::KindWord, BindingKind::Phrase] {
        lifecycle(kind, "bound", Change::Edit, "agent").await?;
        for status in ["none", "undecided"] {
            lifecycle(kind, status, Change::Add, "agent").await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn stale_person_decisions_still_reject_agent_writes() -> anyhow::Result<()> {
    for kind in [BindingKind::KindWord, BindingKind::Phrase] {
        for status in ["none", "undecided"] {
            for change in [Change::Add, Change::Edit] {
                lifecycle(kind, status, change, "person").await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn class_edit_reconsiders_none() -> anyhow::Result<()> {
    lifecycle(BindingKind::KindWord, "none", Change::Edit, "agent").await
}

#[tokio::test]
async fn class_edit_reconsiders_undecided() -> anyhow::Result<()> {
    lifecycle(BindingKind::KindWord, "undecided", Change::Edit, "agent").await
}

#[tokio::test]
async fn property_edit_reconsiders_none() -> anyhow::Result<()> {
    lifecycle(BindingKind::Phrase, "none", Change::Edit, "agent").await
}

#[tokio::test]
async fn property_edit_reconsiders_undecided() -> anyhow::Result<()> {
    lifecycle(BindingKind::Phrase, "undecided", Change::Edit, "agent").await
}
