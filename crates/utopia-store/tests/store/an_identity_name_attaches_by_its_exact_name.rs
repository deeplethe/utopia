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
