//! 否决一条开放陈述会排一次类型化重算（#1366）：陈述全没了的类型化行退掉，
//! 还有别的活陈述撑着的行留着。

use sqlx::PgPool;
use utopia_store::{graph, materialize, phrase_bindings};
use uuid::Uuid;

#[tokio::test]
async fn rejecting_a_statement_queues_the_recompute_and_retires_only_unsupported_rows(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let [org, ws, kb, class, alone, shared, place, property, binding, only, first, second] =
        std::array::from_fn(|_| Uuid::now_v7());
    let run = async {
        // Hill Mill 的类型化行由两条陈述撑着，Harbor Bakery 的只有一条
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','reject-typed');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','reject-typed');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','reject-typed');
             INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{class}','{kb}','thing','Thing');
             INSERT INTO entities(id,kb_id,type_id,canonical_name) VALUES
                  ('{alone}','{kb}','{class}','Harbor Bakery'),
                  ('{shared}','{kb}','{class}','Hill Mill'),
                  ('{place}','{kb}','{class}','Port Ellen');
             INSERT INTO relation_types (id,kb_id,key,label,temporal)
                  VALUES ('{property}','{kb}','based_in','based in','state');
             INSERT INTO phrase_bindings
                  (id,kb_id,phrase,subject_type_id,object_type_id,object_is_value,
                   relation_type_id,direction,status)
                  VALUES ('{binding}','{kb}','is based in','{class}','{class}',false,
                          '{property}','forward','bound');
             INSERT INTO facts (id,kb_id,subject_id,object_id,layer,phrase,confidence) VALUES
                  ('{only}','{kb}','{alone}','{place}','open','is based in',0.9),
                  ('{first}','{kb}','{shared}','{place}','open','is based in',0.9),
                  ('{second}','{kb}','{shared}','{place}','open','is based in',0.8);"
        ))
        .execute(&pool)
        .await?;
        materialize::materialize(&pool, kb).await?;
        let row = |subject: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Uuid>(
                    "SELECT id FROM facts WHERE kb_id = $1 AND subject_id = $2 AND layer = 'typed'
                       AND predicate_id = $3 AND invalidated_at IS NULL",
                )
                .bind(kb)
                .bind(subject)
                .bind(property)
                .fetch_optional(&pool)
                .await
            }
        };
        let (Some(lone), Some(both)) = (row(alone).await?, row(shared).await?) else {
            anyhow::bail!("the bound statements were not typed");
        };
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
            .bind(kb.to_string())
            .execute(&pool)
            .await?;

        graph::reject_fact(&pool, kb, only).await?;
        graph::reject_fact(&pool, kb, first).await?;
        let queued: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM jobs WHERE kind = $1 AND payload->>'kb_id' = $2 AND status = 'queued'",
        )
        .bind(phrase_bindings::MATERIALIZE_KIND)
        .bind(kb.to_string())
        .fetch_one(&pool)
        .await?;
        assert_eq!(queued, 1, "a rejected statement queues one recompute, and a second reject does not queue another");

        materialize::materialize(&pool, kb).await?;
        assert_eq!(row(alone).await?, None, "the row whose only statement was rejected retires");
        assert_eq!(row(shared).await?, Some(both), "the row another live statement supports stays");
        assert_ne!(lone, both);
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id' = $1")
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
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
