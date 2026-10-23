//! 0071 decisions 2 and 3: an identity mention attaches to the exact-name holder whatever the
//! profile says, and is created otherwise. Skipped without `UTOPIA_DATABASE_URL`.

use sqlx::PgPool;
use utopia_store::resolution::resolve_identity_name;
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
