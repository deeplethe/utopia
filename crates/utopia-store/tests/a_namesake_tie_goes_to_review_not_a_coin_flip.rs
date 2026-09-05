//! 同名并列的灰区不该靠候选顺序掷硬币（#270，续 #221/#296）。
//!
//! 一个库里已有两个「张伟」，画像向量一模一样（都从同一个 chunk 里长出来，
//! #221 之后这是常态）。后来一条没有任何 handle 的「张伟」mention 进来，对两个
//! 候选打出同一个分数。旧的 `resolve_mention` 在最高分 ≥ `SIM_ATTACH` 时直接归并到
//! **先遇到的那个**，且不入任何审核对——落到谁头上全看候选从库里回来的顺序。
//!
//! 分不开就别硬分：新建实体，对并列的两个候选各入一条**人工**审核对，绝不静默归并。
//! 连库才测得到——「先遇到的那个」是一条 `ORDER BY` 缺省下的物理顺序，`cargo check`
//! 一个字看不见。没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败，自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_store::resolution::ReviewStage;
use uuid::Uuid;

struct Fx {
    org: Uuid,
    kb: Uuid,
    person: Uuid,
    zhang_a: Uuid,
    zhang_b: Uuid,
}

/// 两个同名的「张伟」，画像向量完全相同；雇主不同，但那条线索此刻没人用得上。
async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (person, organization) = (Uuid::now_v7(), Uuid::now_v7());
    let works_for = Uuid::now_v7();
    let (platform, finance) = (Uuid::now_v7(), Uuid::now_v7());
    let (zhang_a, zhang_b) = (Uuid::now_v7(), Uuid::now_v7());

    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'namesake-tie-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'namesake-tie-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'namesake-tie-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    for (id, key, label) in [
        (person, "person", "Person"),
        (organization, "organization", "Organization"),
    ] {
        sqlx::query("INSERT INTO entity_types (id, kb_id, key, label) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(kb)
            .bind(key)
            .bind(label)
            .execute(pool)
            .await?;
    }
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal)
         VALUES ($1, $2, 'works_for', 'works for', 'state')",
    )
    .bind(works_for)
    .bind(kb)
    .execute(pool)
    .await?;

    // 两个雇主，两个同名的人。故意一个大写一个小写：召回用 SQL `lower()`，
    // 「Zhang Wei」和「zhang wei」本就是一对同名候选，并列判定也必须忽略大小写。
    for (id, type_id, name) in [
        (platform, organization, "Platform Engineering"),
        (finance, organization, "Finance"),
        (zhang_a, person, "Zhang Wei"),
        (zhang_b, person, "zhang wei"),
    ] {
        sqlx::query(
            "INSERT INTO entities (id, kb_id, type_id, canonical_name) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(kb)
        .bind(type_id)
        .bind(name)
        .execute(pool)
        .await?;
    }
    // 画像向量一模一样：两人都是同一个 chunk 里播下的种子，质心相同
    for id in [zhang_a, zhang_b] {
        sqlx::query("UPDATE entities SET profile_embedding = '[1,0,0]'::vector, profile_n = 1 WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await?;
    }
    // 区分他们的事实就摆在这里——department/雇主——只是画像比对看不见它
    for (subject, object) in [(zhang_a, platform), (zhang_b, finance)] {
        sqlx::query(
            "INSERT INTO facts (id, kb_id, subject_id, predicate_id, object_id, confidence)
             VALUES ($1, $2, $3, $4, $5, 0.9)",
        )
        .bind(Uuid::now_v7())
        .bind(kb)
        .bind(subject)
        .bind(works_for)
        .bind(object)
        .execute(pool)
        .await?;
    }

    Ok(Fx {
        org,
        kb,
        person,
        zhang_a,
        zhang_b,
    })
}

#[tokio::test]
async fn a_namesake_tie_creates_an_entity_and_two_reviews() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;

    let run = async {
        // 与两个候选画像都一致的上下文：对 A、对 B 打出的余弦相同
        let ctx: Vec<f32> = vec![1.0, 0.0, 0.0];
        let r = utopia_store::resolution::resolve_mention(
            &pool,
            f.kb,
            Some(f.person),
            "Zhang Wei",
            Some(&ctx),
            &[],
        )
        .await?;

        // 分不开就不归并：新建了第三个实体，没有 attach 到 A 或 B
        assert!(
            r.created,
            "同名并列不该静默归并到先遇到的那个——该新建实体（#270）"
        );
        assert_ne!(r.entity_id, f.zhang_a, "attach 到了 A：候选顺序掷出的硬币");
        assert_ne!(r.entity_id, f.zhang_b, "attach 到了 B：候选顺序掷出的硬币");

        // 对并列的两个候选各入一条审核对
        let mut reviewed: Vec<Uuid> = r.reviews.iter().map(|rv| rv.other_id).collect();
        reviewed.sort();
        let mut want = vec![f.zhang_a, f.zhang_b];
        want.sort();
        assert_eq!(reviewed, want, "两个同名候选都该进审核，一个都不能少");

        // 同名并列只有人分得开，绝不能让批量裁决器把两条几乎相同的画像自动并掉
        assert!(
            r.reviews.iter().all(|rv| rv.stage == ReviewStage::Human),
            "同名并列的审核对必须是人工阶段（Human）"
        );

        // 落库后确实是两条待裁的人工审核对，都挂在新建的实体上
        for rv in &r.reviews {
            utopia_store::resolution::create_review(
                &pool,
                f.kb,
                r.entity_id,
                rv.other_id,
                rv.score,
                &rv.reason,
                rv.stage,
            )
            .await?;
        }
        let pending: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT CASE WHEN left_id = $2 THEN right_id ELSE left_id END, stage
             FROM resolution_reviews
             WHERE kb_id = $1 AND status = 'pending' AND (left_id = $2 OR right_id = $2)",
        )
        .bind(f.kb)
        .bind(r.entity_id)
        .fetch_all(&pool)
        .await?;
        assert_eq!(pending.len(), 2, "库里该有两条待裁的审核对");
        assert!(
            pending.iter().all(|(_, stage)| stage == "human"),
            "落库的审核对都该是 human 阶段"
        );
        let mut others: Vec<Uuid> = pending.iter().map(|(id, _)| *id).collect();
        others.sort();
        assert_eq!(others, want, "两条审核对分别指向 A 和 B");

        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await?;
    run
}
