//! 审核阶段是执行边界：human 项必须展示给人，但绝不能被自动裁决器捞走。

use sqlx::PgPool;
use utopia_store::resolution::{self, ReviewStage};
use uuid::Uuid;

#[tokio::test]
async fn human_review_is_visible_but_never_pending_adjudication() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb, left, right, third) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'review-stage-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'review-stage-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'review-stage-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    for (id, name) in [
        (left, "Zhang Wei"),
        (right, "Zhang Wei"),
        (third, "Zhang W."),
    ] {
        sqlx::query("INSERT INTO entities (id, kb_id, canonical_name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(kb)
            .bind(name)
            .execute(&pool)
            .await?;
    }

    let run = async {
        resolution::create_review(&pool, kb, left, right, 1.0, "namesake", ReviewStage::Human)
            .await?;
        resolution::create_review(
            &pool,
            kb,
            left,
            third,
            0.42,
            "ambiguous_name|0.42",
            ReviewStage::Adjudicating,
        )
        .await?;

        let visible = resolution::list_reviews(&pool, kb, 10, 0).await?;
        assert_eq!(visible.len(), 2);
        assert!(visible.iter().any(|r| r.stage == "human"));
        let pending = resolution::pending_adjudications(&pool, kb, 10).await?;
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].stage, "adjudicating");

        // pending pair 的唯一索引不含 stage。后出现的 namesake 必须把已有行升级为
        // human；反向的普通请求不得再把它降回自动裁决通道。
        resolution::create_review(&pool, kb, left, third, 1.0, "namesake", ReviewStage::Human)
            .await?;
        resolution::create_review(
            &pool,
            kb,
            left,
            third,
            0.42,
            "ambiguous_name|0.42",
            ReviewStage::Adjudicating,
        )
        .await?;
        assert!(resolution::pending_adjudications(&pool, kb, 10)
            .await?
            .is_empty());
        let visible = resolution::list_reviews(&pool, kb, 10, 0).await?;
        let upgraded = visible.iter().find(|r| {
            (r.left.id == left && r.right.id == third) || (r.left.id == third && r.right.id == left)
        });
        assert_eq!(upgraded.map(|r| r.stage.as_str()), Some("human"));
        assert_eq!(upgraded.and_then(|r| r.reason.as_deref()), Some("namesake"));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await;
    run
}
