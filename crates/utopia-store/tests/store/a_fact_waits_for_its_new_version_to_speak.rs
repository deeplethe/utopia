//! 新版本写下了、还没被读：它引用的事实不算过期，也不进审核队列。
//! 新版本被读过而没再提这条事实，才算它不再被陈述。
//! 没人会去读的新版本（抽取关着、跳过或失败）不让事实等：等下去就是永远。

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use utopia_store::{graph, review};
use uuid::Uuid;

/// 新版本写下的那一刻：旧分块在这时被取代
const REPUSHED: &str = "2025-06-01T12:34:56Z";

struct Seeded {
    kb: Uuid,
    org: Uuid,
    ada: Uuid,
    fresh: Uuid,
    said: Uuid,
}

async fn seed(pool: &PgPool, status: &str, graph_status: &str) -> anyhow::Result<Seeded> {
    let [org, ws, kb, class, ada, port, based_in, binding] =
        std::array::from_fn(|_| Uuid::now_v7());
    let [source, doc, old, fresh, said] = std::array::from_fn(|_| Uuid::now_v7());
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','fact-waits');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','fact-waits');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','fact-waits');
         INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{class}','{kb}','thing','Thing');
         INSERT INTO entities(id,kb_id,type_id,canonical_name,created_at) VALUES
              ('{ada}','{kb}','{class}','Ada','2025-01-01'),
              ('{port}','{kb}','{class}','Port Ellen','2025-01-01');
         INSERT INTO relation_types (id,kb_id,key,label,temporal)
              VALUES ('{based_in}','{kb}','based_in','based in','state');
         INSERT INTO phrase_bindings
              (id,kb_id,phrase,subject_type_id,object_type_id,object_is_value,
               relation_type_id,direction,status)
              VALUES ('{binding}','{kb}','is based in','{class}','{class}',false,
                      '{based_in}','forward','bound');
         INSERT INTO sources(id,kb_id,kind,name) VALUES ('{source}','{kb}','statements','mirror');
         INSERT INTO documents(id,kb_id,filename,sha256,source_id,status,graph_status,created_at)
              VALUES ('{doc}','{kb}','a.md','sha-a','{source}','{status}','{graph_status}','2025-01-01');
         INSERT INTO chunks(id,kb_id,document_id,seq,text,doc_version,created_at,superseded_at,extracted_at) VALUES
              ('{old}','{kb}','{doc}',0,'v1','1','2025-01-01','{REPUSHED}','2025-01-02'),
              ('{fresh}','{kb}','{doc}',1,'v2','2','{REPUSHED}',NULL,NULL);
         INSERT INTO facts (id,kb_id,subject_id,object_id,layer,phrase,confidence,
                            valid_from,valid_from_precision,recorded_at)
              VALUES ('{said}','{kb}','{ada}','{port}','open','is based in',0.9,'2020-01-01','day',
                      '2025-01-02');
         INSERT INTO fact_evidence (fact_id,chunk_id,document_id,quote)
              VALUES ('{said}','{old}','{doc}','q');"
    ))
    .execute(pool)
    .await?;
    Ok(Seeded {
        kb,
        org,
        ada,
        fresh,
        said,
    })
}

async fn is_stale(pool: &PgPool, seeded: &Seeded) -> anyhow::Result<bool> {
    is_stale_as_of(pool, seeded, None).await
}

async fn is_stale_as_of(
    pool: &PgPool,
    seeded: &Seeded,
    as_of: Option<DateTime<Utc>>,
) -> anyhow::Result<bool> {
    let (_, facts) = graph::entity_detail(pool, seeded.kb, seeded.ada, None, as_of).await?;
    let fact = facts
        .iter()
        .find(|fact| fact.id == seeded.said)
        .expect("the fact is on Ada");
    Ok(fact.stale)
}

async fn is_listed_stale(pool: &PgPool, seeded: &Seeded) -> anyhow::Result<bool> {
    let listed = graph::stale_facts(pool, seeded.kb, 100, 0).await?;
    Ok(listed.iter().any(|item| item.id == seeded.said))
}

async fn is_counted_unconfirmed(pool: &PgPool, seeded: &Seeded) -> anyhow::Result<bool> {
    Ok(review::counts(pool, seeded.kb).await?.unconfirmed == 1)
}

async fn drop_seeded(pool: &PgPool, seeded: &Seeded) -> anyhow::Result<()> {
    let Seeded { kb, org, .. } = seeded;
    sqlx::raw_sql(&format!(
        "DELETE FROM jobs WHERE payload->>'kb_id' = '{kb}';
         DELETE FROM knowledge_bases WHERE id = '{kb}';
         DELETE FROM organizations WHERE id = '{org}';"
    ))
    .execute(pool)
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_fact_is_not_stale_until_the_new_version_has_been_read() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let seeded = seed(&pool, "ready", "queued").await?;
    let run = async {
        assert!(
            !is_stale(&pool, &seeded).await?,
            "a new version was written and not yet read: nothing has been dropped yet"
        );
        assert!(
            !is_listed_stale(&pool, &seeded).await?,
            "the review queue must not list a fact whose new version is unread"
        );
        assert!(
            !is_counted_unconfirmed(&pool, &seeded).await?,
            "the review badge must not count a fact whose new version is unread"
        );

        let read_at: DateTime<Utc> = Utc::now();
        sqlx::query("UPDATE chunks SET extracted_at = $2 WHERE id = $1")
            .bind(seeded.fresh)
            .bind(read_at)
            .execute(&pool)
            .await?;
        assert!(
            is_stale(&pool, &seeded).await?,
            "the new version spoke and did not restate the fact"
        );
        assert!(
            is_listed_stale(&pool, &seeded).await?,
            "the review queue lists it once the version has spoken"
        );
        assert!(
            is_counted_unconfirmed(&pool, &seeded).await?,
            "the review badge counts it once the version has spoken"
        );
        anyhow::Ok(())
    }
    .await;
    drop_seeded(&pool, &seeded).await?;
    run
}

#[tokio::test]
async fn a_fact_does_not_wait_for_a_version_nobody_will_read() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    for (status, graph_status) in [
        ("ready", "none"),
        ("ready", "skipped"),
        ("ready", "failed"),
        ("failed", "none"),
    ] {
        let seeded = seed(&pool, status, graph_status).await?;
        let run = async {
            assert!(
                is_stale(&pool, &seeded).await?,
                "{status}/{graph_status}: no extraction is coming, so the unread version holds nothing back"
            );
            assert!(
                is_listed_stale(&pool, &seeded).await?,
                "{status}/{graph_status}: the review queue lists it"
            );
            anyhow::Ok(())
        }
        .await;
        drop_seeded(&pool, &seeded).await?;
        run?;
    }
    Ok(())
}

#[tokio::test]
async fn a_fact_waits_while_its_new_version_is_still_being_ingested() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let seeded = seed(&pool, "pending", "none").await?;
    let run = async {
        assert!(
            !is_stale(&pool, &seeded).await?,
            "a re-push resets graph_status to none until ingest queues the extraction"
        );
        assert!(
            !is_listed_stale(&pool, &seeded).await?,
            "the review queue lists nothing before the version is queued"
        );
        anyhow::Ok(())
    }
    .await;
    drop_seeded(&pool, &seeded).await?;
    run
}

#[tokio::test]
async fn a_replay_to_the_day_before_the_new_version_was_read_shows_the_fact_still_waiting(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let seeded = seed(&pool, "ready", "done").await?;
    let run = async {
        sqlx::query("UPDATE chunks SET extracted_at = '2025-06-03T00:00:00Z' WHERE id = $1")
            .bind(seeded.fresh)
            .execute(&pool)
            .await?;
        let stale_on = |day: &'static str| {
            let (pool, seeded) = (&pool, &seeded);
            async move { is_stale_as_of(pool, seeded, Some(day.parse()?)).await }
        };
        assert_eq!(
            (
                stale_on("2025-05-31T00:00:00Z").await?,
                stale_on("2025-06-02T00:00:00Z").await?,
                stale_on("2025-06-04T00:00:00Z").await?,
            ),
            (false, false, true),
            "before the re-push, between the re-push and its reading, after the reading"
        );
        anyhow::Ok(())
    }
    .await;
    drop_seeded(&pool, &seeded).await?;
    run
}
