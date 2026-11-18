//! A merge keeps distinct open phrases and the sources of deduplicated typed facts.

use sqlx::PgPool;
use utopia_store::resolution;
use uuid::Uuid;

#[tokio::test]
async fn merging_preserves_open_phrases_and_the_surviving_typed_rows_sources() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let [org, ws, kb, class, target, source, place, property, binding, withdrawn, ships, receives, based, kept, dropped] =
        std::array::from_fn(|_| Uuid::now_v7());
    let run = async {
        // Locally generated UUIDs only. The keeper has a corrected end, but its
        // original source was withdrawn; the duplicate has the only live source.
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','merge-statements');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','merge-statements');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','merge-statements');
             INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{class}','{kb}','thing','Thing');
             INSERT INTO entities(id,kb_id,type_id,canonical_name) VALUES
                  ('{target}','{kb}','{class}','Harbor Bakery'),
                  ('{source}','{kb}','{class}','Harbor'),
                  ('{place}','{kb}','{class}','Port Ellen');
             INSERT INTO relation_types (id,kb_id,key,label,temporal)
                  VALUES ('{property}','{kb}','headquartered_in','headquartered in','state');
             INSERT INTO phrase_bindings
                  (id,kb_id,phrase,subject_type_id,object_type_id,object_is_value,
                   relation_type_id,direction,status)
                  VALUES ('{binding}','{kb}','is based in','{class}','{class}',false,
                          '{property}','forward','bound');
             INSERT INTO facts (id,kb_id,subject_id,object_id,layer,phrase,confidence) VALUES
                  ('{withdrawn}','{kb}','{target}','{place}','open','is based in',0.9),
                  ('{ships}','{kb}','{target}','{place}','open','ships to',0.9),
                  ('{receives}','{kb}','{target}','{place}','open','receives from',0.9),
                  ('{based}','{kb}','{source}','{place}','open','is based in',0.9);
             UPDATE facts SET invalidated_at = now() WHERE id = '{withdrawn}';
             INSERT INTO facts
                  (id,kb_id,subject_id,predicate_id,object_id,confidence,from_statement_id,
                   recorded_at,valid_to,valid_to_precision,corrected_ends) VALUES
                  ('{kept}','{kb}','{target}','{property}','{place}',0.9,'{withdrawn}',
                   '2024-01-01','2025-01-01','year','end'),
                  ('{dropped}','{kb}','{source}','{property}','{place}',0.9,'{based}',
                   '2024-02-01',NULL,NULL,NULL);
             INSERT INTO typed_fact_sources (fact_id,statement_id) VALUES
                  ('{kept}','{withdrawn}'), ('{dropped}','{based}');"
        ))
        .execute(&pool)
        .await?;

        let merge = resolution::merge_entities(&pool, kb, source, target, None, "test").await?;
        let live: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM facts WHERE kb_id = $1 AND subject_id = $2
               AND layer = 'open' AND invalidated_at IS NULL ORDER BY id",
        )
        .bind(kb)
        .bind(target)
        .fetch_all(&pool)
        .await?;
        let mut expected = vec![ships, receives, based];
        expected.sort();
        assert_eq!(live, expected, "different phrases, including those already on the target, survive");
        let invalidated: Vec<Uuid> = sqlx::query_scalar(
            "SELECT invalidated_facts FROM entity_merges WHERE id = $1",
        )
        .bind(merge)
        .fetch_one(&pool)
        .await?;
        assert_eq!(invalidated, vec![dropped]);
        let has_source: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM typed_fact_sources WHERE fact_id = $1 AND statement_id = $2)",
        )
        .bind(kept)
        .bind(based)
        .fetch_one(&pool)
        .await?;
        assert!(has_source, "the surviving typed row inherits the duplicate's live source");

        assert_eq!(
            utopia_store::materialize::materialize(&pool, kb).await?,
            utopia_store::materialize::Outcome::default(),
            "materialization must neither retire the keeper nor rederive its uncorrected interval"
        );
        let unchanged: bool = sqlx::query_scalar(
            "SELECT invalidated_at IS NULL AND valid_to = '2025-01-01'::timestamptz
                    AND valid_to_precision = 'year' AND corrected_ends = 'end'
               FROM facts WHERE id = $1",
        )
        .bind(kept)
        .fetch_one(&pool)
        .await?;
        assert!(unchanged);
        anyhow::Ok(())
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
