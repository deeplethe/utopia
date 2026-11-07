//! 0071 decisions 2 to 6: an identity mention attaches to the exact-name holder whatever the
//! profile says, and is created otherwise; its names get no vectors. Skipped without
//! `UTOPIA_DATABASE_URL`.

use serde_json::json;
use sqlx::PgPool;
use utopia_store::names::{record, NameSource};
use utopia_store::{name_vectors, resolution::resolve_identity_name, sources};
use uuid::Uuid;

/// Orthogonal to every holder's profile, so the profile never supports an attach.
const UNRELATED: [f32; 3] = [0.0, 1.0, 0.0];

/// A kb with a project and an organization class, a body to run in it, then teardown.
async fn in_kb<F, Fut>(body: F) -> anyhow::Result<()>
where
    F: FnOnce(PgPool, Uuid, Uuid, Uuid) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let ids: [Uuid; 5] = std::array::from_fn(|_| Uuid::now_v7());
    let [org, ws, kb, project, organization] = ids;
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations (id, name) VALUES ('{org}', 'identity');
         INSERT INTO workspaces (id, org_id, name) VALUES ('{ws}', '{org}', 'identity');
         INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ('{kb}', '{ws}', 'identity');
         INSERT INTO entity_types (id, kb_id, key, label) VALUES
             ('{project}', '{kb}', 'project', 'Project'),
             ('{organization}', '{kb}', 'organization', 'Organization');"
    ))
    .execute(&pool)
    .await?;
    let run = body(pool.clone(), kb, project, organization).await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}

/// A live entity whose profile points along the first axis.
async fn holder(
    pool: &PgPool,
    kb: Uuid,
    type_id: Option<Uuid>,
    name: &str,
) -> anyhow::Result<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entities (id, kb_id, type_id, canonical_name, profile_embedding, profile_n)
         VALUES ($1, $2, $3, $4, '[1,0,0]'::vector, 1)",
    )
    .bind(id)
    .bind(kb)
    .bind(type_id)
    .bind(name)
    .execute(pool)
    .await?;
    utopia_store::names::record(pool, kb, id, name, None, None).await?;
    Ok(id)
}

#[tokio::test]
async fn one_exact_holder_is_attached_and_otherwise_the_name_is_created() -> anyhow::Result<()> {
    in_kb(|pool, kb, project, _| async move {
        let only = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        let (twin, twin_too) = (
            holder(&pool, kb, Some(project), "acme/b.ts").await?,
            holder(&pool, kb, Some(project), "acme/b.ts").await?,
        );
        let resolve = |name: &'static str| {
            resolve_identity_name(&pool, kb, Some(project), name, Some(&UNRELATED), None, &[])
        };
        let attached = resolve("ACME/a.ts").await?;
        assert_eq!((attached.entity_id, attached.created), (only, false));
        assert!(resolve("acme/c.ts").await?.created);
        let split = resolve("acme/b.ts").await?;
        assert!(split.created && ![twin, twin_too].contains(&split.entity_id));
        // known only by a known_as fact, as after a merge
        let renamed = holder(&pool, kb, Some(project), "acme/old.ts").await?;
        utopia_store::names::record(&pool, kb, renamed, "acme/e.ts", None, None).await?;
        assert_eq!(resolve("acme/e.ts").await?.entity_id, renamed);
        // untyped mention, same profile as a typed holder: still no concept-fallback attach
        let typed = holder(&pool, kb, Some(project), "acme/d.ts").await?;
        let same = [1.0, 0.0, 0.0];
        let untyped =
            resolve_identity_name(&pool, kb, None, "acme/d.ts", Some(&same), None, &[]).await?;
        assert!(untyped.created && untyped.entity_id != typed);
        // an untyped holder takes the declared class
        let untyped_holder = holder(&pool, kb, None, "acme/f.ts").await?;
        resolve("acme/f.ts").await?;
        let class: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id = $1")
            .bind(untyped_holder)
            .fetch_one(&pool)
            .await?;
        assert_eq!(class, Some(project));
        // a class a person removed stays removed
        let cleared = holder(&pool, kb, None, "acme/g.ts").await?;
        sqlx::query("UPDATE entities SET type_source = 'human' WHERE id = $1")
            .bind(cleared)
            .execute(&pool)
            .await?;
        resolve("acme/g.ts").await?;
        let class: Option<Uuid> = sqlx::query_scalar("SELECT type_id FROM entities WHERE id = $1")
            .bind(cleared)
            .fetch_one(&pool)
            .await?;
        assert_eq!(class, None);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_name_under_another_class_is_created_with_a_drift_review_unless_disjoint(
) -> anyhow::Result<()> {
    in_kb(|pool, kb, project, organization| async move {
        holder(&pool, kb, Some(organization), "acme/a.ts").await?;
        holder(&pool, kb, Some(organization), "acme/b.ts").await?;
        let reviews = |name: &'static str| async {
            let r = resolve_identity_name(&pool, kb, Some(project), name, None, None, &[]).await?;
            anyhow::Ok((
                r.created,
                r.reviews.into_iter().map(|v| v.reason).collect::<Vec<_>>(),
            ))
        };
        assert_eq!(
            reviews("acme/a.ts").await?,
            (true, vec!["type_drift|project vs organization".to_string()])
        );
        for (a, b) in [(project, organization), (organization, project)] {
            sqlx::query("INSERT INTO entity_type_disjoint (kb_id, a_id, b_id) VALUES ($1, $2, $3)")
                .bind(kb)
                .bind(a)
                .bind(b)
                .execute(&pool)
                .await?;
        }
        assert_eq!(reviews("acme/b.ts").await?, (true, vec![]));
        Ok(())
    })
    .await
}

/// A statements source with this config and one chunk in it; `None` is an upload's chunk.
async fn chunk(
    pool: &PgPool,
    kb: Uuid,
    config: Option<serde_json::Value>,
) -> anyhow::Result<(Uuid, Uuid)> {
    let (source, document, chunk) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    if let Some(config) = &config {
        sqlx::query("INSERT INTO sources (id, kb_id, kind, name, config) VALUES ($1, $2, 'statements', 's', $3)")
            .bind(source)
            .bind(kb)
            .bind(config)
            .execute(pool)
            .await?;
    }
    sqlx::query("INSERT INTO documents (id, kb_id, filename, sha256, source_id) VALUES ($1, $2, 'd', $1::text, $3)")
        .bind(document)
        .bind(kb)
        .bind(config.map(|_| source))
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO chunks (id, kb_id, document_id, seq, text) VALUES ($1, $2, $3, 0, 'acme')",
    )
    .bind(chunk)
    .bind(kb)
    .bind(document)
    .execute(pool)
    .await?;
    Ok((source, chunk))
}

/// Names `entity` once from each chunk and returns the one name fact.
async fn named(
    pool: &PgPool,
    kb: Uuid,
    entity: Uuid,
    name: &str,
    chunks: &[Uuid],
) -> anyhow::Result<Uuid> {
    let mut fact = None;
    for &chunk_id in chunks {
        fact = record(
            pool,
            kb,
            entity,
            name,
            Some(NameSource {
                chunk_id,
                quote: name,
            }),
            None,
        )
        .await?;
    }
    fact.ok_or_else(|| anyhow::anyhow!("no name fact"))
}

async fn vectors(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(
        sqlx::query_scalar("SELECT fact_id FROM name_vectors WHERE kb_id = $1")
            .bind(kb)
            .fetch_all(pool)
            .await?,
    )
}

async fn pending_entities(pool: &PgPool, kb: Uuid) -> anyhow::Result<Vec<Uuid>> {
    Ok(name_vectors::pending(pool, kb, 100)
        .await?
        .into_iter()
        .map(|(_, entity, _)| entity)
        .collect())
}

#[tokio::test]
async fn a_name_with_identity_evidence_gets_no_vector_even_when_prose_names_it_too(
) -> anyhow::Result<()> {
    in_kb(|pool, kb, project, _| async move {
        let (_, identity) = chunk(&pool, kb, Some(json!({"names": "identity"}))).await?;
        let (_, prose) = chunk(&pool, kb, None).await?;
        let both = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        let both_name = named(&pool, kb, both, "acme/a.ts", &[identity, prose]).await?;
        let prose_only = holder(&pool, kb, Some(project), "acme/b.ts").await?;
        named(&pool, kb, prose_only, "acme/b.ts", &[prose]).await?;
        assert_eq!(pending_entities(&pool, kb).await?, vec![prose_only]);
        // a backfill that read it as pending before the identity evidence came writes nothing
        name_vectors::set(&pool, kb, &[(both_name, both, vec![1.0, 0.0, 0.0])]).await?;
        assert_eq!(vectors(&pool, kb).await?, Vec::<Uuid>::new());
        Ok(())
    })
    .await
}

#[tokio::test]
async fn turning_the_flag_on_drops_the_sources_name_vectors_and_off_makes_them_pending(
) -> anyhow::Result<()> {
    in_kb(|pool, kb, project, _| async move {
        let (source, pushed) = chunk(&pool, kb, Some(json!({}))).await?;
        let (_, prose) = chunk(&pool, kb, None).await?;
        let file = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        let file_name = named(&pool, kb, file, "acme/a.ts", &[pushed]).await?;
        let other = holder(&pool, kb, Some(project), "acme/b.ts").await?;
        let other_name = named(&pool, kb, other, "acme/b.ts", &[prose]).await?;
        let vector = vec![1.0, 0.0, 0.0];
        name_vectors::set(
            &pool,
            kb,
            &[
                (file_name, file, vector.clone()),
                (other_name, other, vector),
            ],
        )
        .await?;
        let flag = |config: serde_json::Value| {
            let pool = pool.clone();
            async move { sources::update(&pool, source, None, Some(&config), None, None).await }
        };

        flag(json!({"names": "identity"})).await?;
        assert_eq!(vectors(&pool, kb).await?, vec![other_name]);
        flag(json!({})).await?;
        assert!(pending_entities(&pool, kb).await?.contains(&file));
        Ok(())
    })
    .await
}

/// `entity` 现行的名字事实。
async fn live_name(pool: &PgPool, entity: Uuid) -> anyhow::Result<Uuid> {
    Ok(sqlx::query_scalar(
        "SELECT f.id FROM facts f JOIN relation_types r ON r.id = f.predicate_id
          WHERE f.subject_id = $1 AND r.key = $2 AND f.invalidated_at IS NULL",
    )
    .bind(entity)
    .bind(utopia_store::names::KNOWN_AS)
    .fetch_one(pool)
    .await?)
}

/// #1426：人把身份来源的实体并进一个早就从散文里记下同名的实体。存活的名字留住身份证据，
/// 它仍是身份实体；撤回合并只收回抄过去的那些。
#[tokio::test]
async fn a_merge_keeps_the_identity_evidence_of_the_name_it_dedupes() -> anyhow::Result<()> {
    use utopia_store::execution_gate::{hold_for_merge, Hold};
    use utopia_store::resolution::{merge_entities, revert_merge};
    in_kb(|pool, kb, project, _| async move {
        let (_, identity) = chunk(&pool, kb, Some(json!({"names": "identity"}))).await?;
        let (_, prose) = chunk(&pool, kb, None).await?;
        let keeper = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        let kept_name = named(&pool, kb, keeper, "acme/a.ts", &[prose]).await?;
        name_vectors::set(&pool, kb, &[(kept_name, keeper, vec![1.0, 0.0, 0.0])]).await?;
        let pushed = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        named(&pool, kb, pushed, "acme/a.ts", &[identity]).await?;
        let bystander = holder(&pool, kb, Some(project), "acme/z.ts").await?;
        let evidence = |fact: Uuid| {
            let pool = pool.clone();
            async move {
                let mut chunks: Vec<Uuid> =
                    sqlx::query_scalar("SELECT chunk_id FROM fact_evidence WHERE fact_id = $1")
                        .bind(fact)
                        .fetch_all(&pool)
                        .await?;
                chunks.sort();
                anyhow::Ok(chunks)
            }
        };
        let mut both = vec![identity, prose];
        both.sort();

        let merge = merge_entities(&pool, kb, pushed, keeper, None, "test").await?;
        assert_eq!(live_name(&pool, keeper).await?, kept_name);
        assert_eq!(evidence(kept_name).await?, both);
        assert_eq!(
            hold_for_merge(&pool, kb, keeper, bystander).await?,
            Some(Hold::Identity)
        );
        assert_eq!(vectors(&pool, kb).await?, Vec::<Uuid>::new());

        revert_merge(&pool, kb, merge).await?;
        assert_eq!(evidence(kept_name).await?, vec![prose]);
        assert_eq!(
            evidence(live_name(&pool, pushed).await?).await?,
            vec![identity]
        );
        assert_ne!(
            hold_for_merge(&pool, kb, keeper, bystander).await?,
            Some(Hold::Identity)
        );
        assert!(pending_entities(&pool, kb).await?.contains(&keeper));
        Ok(())
    })
    .await
}

/// 存活行原本就有的同一分块证据是它自己的，撤回合并不动。
#[tokio::test]
async fn undoing_a_merge_leaves_the_survivors_own_evidence() -> anyhow::Result<()> {
    use utopia_store::resolution::{merge_entities, revert_merge};
    in_kb(|pool, kb, project, _| async move {
        let (_, prose) = chunk(&pool, kb, None).await?;
        let keeper = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        let kept_name = named(&pool, kb, keeper, "acme/a.ts", &[prose]).await?;
        let other = holder(&pool, kb, Some(project), "acme/a.ts").await?;
        named(&pool, kb, other, "acme/a.ts", &[prose]).await?;
        let merge = merge_entities(&pool, kb, other, keeper, None, "test").await?;
        revert_merge(&pool, kb, merge).await?;
        let kept: Vec<Uuid> =
            sqlx::query_scalar("SELECT chunk_id FROM fact_evidence WHERE fact_id = $1")
                .bind(kept_name)
                .fetch_all(&pool)
                .await?;
        assert_eq!(kept, vec![prose]);
        Ok(())
    })
    .await
}

/// 合并之后存活的名字可能被改写：人改了区间，证据随行到新行上。撤回合并要从现在持有它的
/// 那一行收回，不管存活行原本属于哪一边。
#[tokio::test]
async fn undoing_a_merge_takes_the_copied_evidence_off_a_rewritten_name() -> anyhow::Result<()> {
    use utopia_store::graph::Validity;
    use utopia_store::resolution::{merge_entities, revert_merge};
    in_kb(|pool, kb, project, _| async move {
        let (_, identity) = chunk(&pool, kb, Some(json!({"names": "identity"}))).await?;
        let (_, prose) = chunk(&pool, kb, None).await?;
        // 散文的名字先记，无论往哪边并都是它存活
        for prose_moves in [false, true] {
            let name = format!("acme/{prose_moves}.ts");
            let prose_holder = holder(&pool, kb, Some(project), &name).await?;
            let kept_name = named(&pool, kb, prose_holder, &name, &[prose]).await?;
            let identity_holder = holder(&pool, kb, Some(project), &name).await?;
            named(&pool, kb, identity_holder, &name, &[identity]).await?;
            let (source, target) = if prose_moves {
                (prose_holder, identity_holder)
            } else {
                (identity_holder, prose_holder)
            };
            let merge = merge_entities(&pool, kb, source, target, None, "test").await?;
            utopia_store::temporal::correct_interval(
                &pool,
                kept_name,
                Validity {
                    from: Some(chrono::Utc::now() - chrono::Duration::days(30)),
                    from_precision: Some("day"),
                    ..Default::default()
                },
            )
            .await?
            .expect("the surviving name is live");
            revert_merge(&pool, kb, merge).await?;
            let live = live_name(&pool, prose_holder).await?;
            let chunks: Vec<Uuid> =
                sqlx::query_scalar("SELECT chunk_id FROM fact_evidence WHERE fact_id = $1")
                    .bind(live)
                    .fetch_all(&pool)
                    .await?;
            assert_eq!(chunks, vec![prose], "prose_moves = {prose_moves}");
        }
        Ok(())
    })
    .await
}
