//! Shared entities keep their own aliases, degree and top-four limit when a
//! review page is assembled in batches. Exercise the SQL against a real DB.

use chrono::{Duration, TimeZone, Utc};
use sqlx::PgPool;
use utopia_core::models::ReviewItem;
use utopia_store::resolution::{self, TypeFilter};
use uuid::Uuid;

fn ids(items: &[ReviewItem]) -> Vec<Uuid> {
    items.iter().map(|item| item.id).collect()
}

#[tokio::test]
async fn shared_sides_keep_their_summaries_and_the_page_keeps_its_order() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, other_kb) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'review-page-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'review-page-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;

    let run = async {
        for base in [kb, other_kb] {
            sqlx::query(
                "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'review-page-test')",
            )
            .bind(base)
            .bind(ws)
            .execute(&pool)
            .await?;
        }
        let (person, company, relation) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        for (id, key, label) in [(person, "person", "Person"), (company, "company", "Company")] {
            sqlx::query(
                "INSERT INTO entity_types (id, kb_id, key, label, color) VALUES ($1, $2, $3, $4, '#123456')",
            )
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(&pool)
            .await?;
        }
        sqlx::query(
            "INSERT INTO relation_types (id, kb_id, key, label) VALUES ($1, $2, 'knows', 'Knows')",
        )
        .bind(relation)
        .bind(kb)
        .execute(&pool)
        .await?;
        let [a, b, c, d, neighbor, other_a, other_b] = std::array::from_fn(|_| Uuid::now_v7());
        for (id, base, ty, name, suffix) in [
            (a, kb, Some(person), "Alpha", Some("North")),
            (b, kb, Some(person), "Beta", None),
            (c, kb, Some(company), "Gamma", None),
            (d, kb, None, "Untyped", None),
            (neighbor, kb, None, "Neighbor", None),
            (other_a, other_kb, None, "Alpha", None),
            (other_b, other_kb, None, "Beta", None),
        ] {
            sqlx::query(
                "INSERT INTO entities (id, kb_id, type_id, canonical_name, disambiguator)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(base)
            .bind(ty)
            .bind(name)
            .bind(suffix)
            .execute(&pool)
            .await?;
        }

        let at = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();
        let known_as = utopia_store::names::ensure_known_as(&pool, kb).await?;
        for (entity, canonical) in [(a, "alpha"), (b, "BETA")] {
            // Two entities share the same aliases; deduplication must stay per entity.
            for (i, name) in ["Common", "Alternate", "common", canonical, "Retired"]
                .into_iter()
                .enumerate()
            {
                sqlx::query(
                    "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_value,
                                        recorded_at, invalidated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)",
                )
                .bind(Uuid::now_v7())
                .bind(kb)
                .bind(entity)
                .bind(known_as)
                .bind(serde_json::json!({"value": name}))
                .bind(at + Duration::seconds(i as i64))
                .bind((name == "Retired").then_some(at + Duration::days(1)))
                .execute(&pool)
                .await?;
            }

            // Six live facts per entity, including a self-loop. A seventh, higher
            // confidence fact is invalidated. Equal confidence uses recorded_at.
            for (i, confidence) in [0.9f32, 0.9, 0.8, 0.7, 0.6, 0.5, 1.0]
                .into_iter()
                .enumerate()
            {
                let subject = if i == 0 { neighbor } else { entity };
                let object = match i {
                    0 | 5 => Some(entity),
                    3 => None,
                    _ => Some(neighbor),
                };
                sqlx::query(
                    "INSERT INTO facts (id, kb_id, subject_id, predicate_id, phrase, object_id,
                                        object_value, confidence, recorded_at, invalidated_at,
                                        valid_from, valid_from_precision, valid_to, valid_to_precision)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10,
                             CASE WHEN $11 < 2 THEN '2020-01-01'::timestamptz END,
                             CASE WHEN $11 < 2 THEN 'month' END,
                             CASE WHEN $11 = 0 THEN '2021-02-01'::timestamptz END,
                             CASE WHEN $11 = 0 THEN 'month' END)",
                )
                .bind(Uuid::now_v7())
                .bind(kb)
                .bind(subject)
                .bind((i != 2).then_some(relation))
                .bind((i == 2).then_some("works alongside"))
                .bind(object)
                .bind((i == 3).then(|| serde_json::json!({"value": "text"})))
                .bind(confidence)
                .bind(at + Duration::seconds(10 - i as i64))
                .bind((i == 6).then_some(at + Duration::days(1)))
                .bind(i as i32)
                .execute(&pool)
                .await?;
            }
        }

        let mut reviews: [Uuid; 6] = std::array::from_fn(|_| Uuid::now_v7());
        reviews.sort_unstable();
        // The newest two share a timestamp, so the ID tie-breaker matters.
        for (i, (base, left, right, stage, status, seconds)) in [
            (kb, a, b, "human", "pending", 3),
            (kb, c, a, "adjudicating", "pending", 3),
            (kb, b, c, "human", "pending", 2),
            (kb, a, d, "adjudicating", "pending", 1),
            (kb, b, d, "human", "kept", 4),
            (other_kb, other_a, other_b, "human", "pending", 5),
        ]
        .into_iter()
        .enumerate()
        {
            sqlx::query(
                "INSERT INTO resolution_reviews (id, kb_id, left_id, right_id, stage, status,
                                                 score, reason, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, 0.8, 'namesake', $7)",
            )
            .bind(reviews[i])
            .bind(base)
            .bind(left)
            .bind(right)
            .bind(stage)
            .bind(status)
            .bind(at + Duration::seconds(seconds))
            .execute(&pool)
            .await?;
        }
        let merge_proposal = Uuid::now_v7();
        let keep_proposal = Uuid::now_v7();
        for (id, review, action, status) in [
            (merge_proposal, reviews[0], "merge", "proposed"),
            (keep_proposal, reviews[2], "keep", "proposed"),
            (Uuid::now_v7(), reviews[0], "keep", "superseded"),
            (Uuid::now_v7(), reviews[1], "keep", "accepted"),
        ] {
            sqlx::query(
                "INSERT INTO agent_decisions (id, kb_id, run_id, target_kind, target_id,
                                              action, confidence, reason, status)
                 VALUES ($1, $2, $3, 'review', $4, $5, 0.75, 'evidence', $6)",
            )
            .bind(id)
            .bind(kb)
            .bind(Uuid::now_v7())
            .bind(review)
            .bind(action)
            .bind(status)
            .execute(&pool)
            .await?;
        }

        let all = resolution::list_reviews(&pool, kb, TypeFilter::Any, 10, 0).await?;
        assert_eq!(ids(&all), vec![reviews[1], reviews[0], reviews[2], reviews[3]]);
        let expected = vec![
            "also known as: Common, Alternate",
            "Knows ← Neighbor (2020-01 → 2021-02)",
            "Knows → Neighbor (2020-01 → now)",
            "works alongside → Neighbor",
            "Knows → text",
        ];
        for item in &all {
            assert_eq!(item.score, 0.8);
            assert_eq!(item.reason.as_deref(), Some("namesake"));
            for side in [&item.left, &item.right] {
                if side.id == a || side.id == b {
                    assert_eq!(side.top_facts, expected, "four facts per entity, plus aliases");
                    assert_eq!(side.degree, 6, "names and invalidated facts do not count; a self-loop counts once");
                    assert_eq!(side.type_label.as_deref(), Some("Person"));
                    assert_eq!(side.color, "#123456");
                } else {
                    assert!(side.top_facts.is_empty());
                    assert_eq!(side.degree, 0);
                }
            }
        }
        assert_eq!(all[0].left.id, c);
        assert_eq!(all[0].left.type_label.as_deref(), Some("Company"));
        assert_eq!(all[0].right.name, "Alpha");
        assert_eq!(all[0].right.disambiguator.as_deref(), Some("North"));
        assert_eq!(all[1].right.name, "Beta");
        assert_eq!(all[1].right.disambiguator, None);
        assert_eq!(all[3].right.id, d);
        assert_eq!(all[3].right.type_label, None);
        assert_eq!(all[3].right.color, "#94a3b8");
        for (item, proposal_id, action) in [
            (&all[1], merge_proposal, "merge"),
            (&all[2], keep_proposal, "keep"),
        ] {
            let proposal = item.proposal.as_ref().expect("open proposal");
            assert_eq!(proposal.id, proposal_id);
            assert_eq!(proposal.action, action);
            assert_eq!(proposal.confidence, 0.75);
            assert_eq!(proposal.reason.as_deref(), Some("evidence"));
        }
        assert!(all[0].proposal.is_none());
        assert!(all[3].proposal.is_none());
        for entity in [a, b] {
            assert_eq!(resolution::entity_fact_lines(&pool, kb, entity, 4).await?, expected);
            assert_eq!(
                resolution::entity_fact_lines(&pool, kb, entity, 1).await?,
                expected[..2],
                "the alias heading does not consume the fact limit"
            );
        }

        assert_eq!(
            ids(&resolution::list_reviews(&pool, kb, TypeFilter::Any, 2, 1).await?),
            vec![reviews[0], reviews[2]]
        );
        assert_eq!(
            ids(&resolution::list_reviews(&pool, kb, TypeFilter::Same, 10, 0).await?),
            vec![reviews[0]]
        );
        assert_eq!(
            ids(&resolution::list_reviews(&pool, kb, TypeFilter::Conflict, 10, 0).await?),
            vec![reviews[1], reviews[2]]
        );
        let pending = resolution::pending_adjudications(&pool, kb, 10).await?;
        assert_eq!(ids(&pending), vec![reviews[3], reviews[1]]);
        assert!(pending.iter().all(|item| item.stage == "adjudicating"));
        assert_eq!(pending[1].right.top_facts, expected);
        assert!(resolution::list_reviews(&pool, kb, TypeFilter::Any, 10, 4).await?.is_empty());
        assert!(resolution::list_reviews(&pool, kb, TypeFilter::Any, 0, 0).await?.is_empty());
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = ANY($1)")
        .bind([kb, other_kb])
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
