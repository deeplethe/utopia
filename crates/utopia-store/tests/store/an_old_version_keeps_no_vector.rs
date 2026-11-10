//! An old version keeps no vector: a chunk superseded longer ago than the cutoff
//! loses its vector, and its row too when no evidence, pending fact, time mention or
//! statement qualifier refers to it. A chunk superseded more recently, a live one, and
//! one evidence still points at, keep what they have. So does every chunk of a deleted
//! document, which a restore makes live again.

use chrono::{Duration, Utc};
use pgvector::Vector;
use sqlx::PgPool;
use utopia_store::retention::{prune_superseded, Pruned};
use uuid::Uuid;

#[derive(Debug, PartialEq, sqlx::FromRow)]
struct Row {
    id: Uuid,
    has_vector: bool,
}

async fn rows(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<Row>> {
    Ok(sqlx::query_as(
        "SELECT id, embedding IS NOT NULL AS has_vector FROM chunks WHERE kb_id = $1 ORDER BY seq",
    )
    .bind(kb)
    .fetch_all(pool)
    .await?)
}

#[tokio::test]
async fn a_chunk_superseded_ten_days_ago_is_gone_and_a_cited_or_qualified_one_keeps_only_its_text(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let [org, ws, kb, class, ada, bea, fact, doc, live, recent, old, cited, qualified] =
        std::array::from_fn(|_| Uuid::now_v7());
    let now = Utc::now();
    let run = async {
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','prune');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','prune');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','prune');
             INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{class}','{kb}','thing','Thing');
             INSERT INTO entities(id,kb_id,type_id,canonical_name) VALUES
                  ('{ada}','{kb}','{class}','Ada'), ('{bea}','{kb}','{class}','Bea');
             INSERT INTO facts (id,kb_id,subject_id,object_id,layer,phrase,confidence)
                  VALUES ('{fact}','{kb}','{ada}','{bea}','open','knows',0.9);
             INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','a.md','sha-{doc}');"
        ))
        .execute(&pool)
        .await?;
        for (id, seq, superseded_at) in [
            (live, 0, None),
            (recent, 1, Some(now - Duration::days(2))),
            (old, 2, Some(now - Duration::days(10))),
            (cited, 3, Some(now - Duration::days(10))),
            (qualified, 4, Some(now - Duration::days(10))),
        ] {
            sqlx::query(
                "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding, superseded_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(id)
            .bind(kb)
            .bind(doc)
            .bind(seq)
            .bind(format!("chunk {seq}"))
            .bind(Vector::from(vec![0.1, 0.2, 0.3]))
            .bind(superseded_at)
            .execute(&pool)
            .await?;
        }
        sqlx::query("INSERT INTO fact_evidence (fact_id, chunk_id, document_id, quote) VALUES ($1, $2, $3, 'q')")
            .bind(fact)
            .bind(cited)
            .bind(doc)
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO statement_qualifiers (fact_id, role, value, chunk_id) VALUES ($1, 'since', '\"2020\"', $2)")
            .bind(fact)
            .bind(qualified)
            .execute(&pool)
            .await?;

        let pruned = prune_superseded(&pool, kb, Duration::days(7)).await?;
        assert_eq!(
            pruned,
            Pruned {
                vectors_cleared: 3,
                chunks_deleted: 1
            }
        );
        assert_eq!(
            rows(&pool, kb).await?,
            vec![
                Row {
                    id: live,
                    has_vector: true
                },
                Row {
                    id: recent,
                    has_vector: true
                },
                Row {
                    id: cited,
                    has_vector: false
                },
                Row {
                    id: qualified,
                    has_vector: false
                },
            ]
        );
        let evidence: (i64,) =
            sqlx::query_as("SELECT count(*) FROM fact_evidence WHERE fact_id = $1")
                .bind(fact)
                .fetch_one(&pool)
                .await?;
        assert_eq!(evidence.0, 1, "the evidence survives with its chunk");

        assert_eq!(
            prune_superseded(&pool, kb, Duration::days(7)).await?,
            Pruned::default(),
            "a second pass finds nothing"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn a_document_restored_after_ten_days_in_the_trash_still_has_its_vector() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let [org, ws, kb, doc, chunk] = std::array::from_fn(|_| Uuid::now_v7());
    let run = async {
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','prune-trash');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','prune-trash');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','prune-trash');
             INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','a.md','sha-{doc}');"
        ))
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO chunks (id, kb_id, document_id, seq, text, embedding)
             VALUES ($1, $2, $3, 0, 'chunk 0', $4)",
        )
        .bind(chunk)
        .bind(kb)
        .bind(doc)
        .bind(Vector::from(vec![0.1, 0.2, 0.3]))
        .execute(&pool)
        .await?;
        utopia_store::documents::delete(&pool, kb, doc, None).await?;
        sqlx::query("UPDATE chunks SET superseded_at = $2 WHERE id = $1")
            .bind(chunk)
            .bind(Utc::now() - Duration::days(10))
            .execute(&pool)
            .await?;

        assert_eq!(
            prune_superseded(&pool, kb, Duration::days(7)).await?,
            Pruned::default(),
            "a deleted document's chunks are left whole"
        );
        utopia_store::documents::restore(&pool, kb, doc).await?;
        assert_eq!(
            rows(&pool, kb).await?,
            vec![Row {
                id: chunk,
                has_vector: true
            }]
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
