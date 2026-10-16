//! 个人令牌读得了图视图（0014 决定 7）：总览与邻域。人的角色是上限，`kb_ids` 再往下压
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    pool: sqlx::PgPool,
    app: axum::Router,
    org: Uuid,
    /// 库 viewer
    viewer: Uuid,
    kb: Uuid,
    entity: Uuid,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, viewer, kb, entity) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','token-graph-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','token-graph-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{viewer}','{org}','{viewer}@token-graph.test','Viewer','unused');
             INSERT INTO memberships(user_id,workspace_id,role) VALUES ('{viewer}','{ws}','viewer');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','mirror');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{viewer}','viewer');
             INSERT INTO entities(id,kb_id,canonical_name) VALUES ('{entity}','{kb}','Robot');"
        ))
        .execute(&pool)
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        let app = super::router(state, &cfg);
        Ok(Some(Self {
            pool,
            app,
            org,
            viewer,
            kb,
            entity,
            _dir: dir,
        }))
    }

    async fn token(&self, kb_ids: Option<&[Uuid]>) -> String {
        utopia_store::tokens::issue(&self.pool, self.viewer, "ci", "read", kb_ids, None)
            .await
            .expect("issuing a token")
            .1
    }

    /// 两条图读各回什么状态码
    async fn graph_statuses(&self, token: &str) -> anyhow::Result<[StatusCode; 2]> {
        let (kb, entity) = (self.kb, self.entity);
        let mut statuses = [StatusCode::OK; 2];
        for (status, path) in statuses.iter_mut().zip([
            format!("/api/v1/kbs/{kb}/graph/overview"),
            format!("/api/v1/kbs/{kb}/graph/neighborhood?entity={entity}"),
        ]) {
            let request = Request::get(path)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())?;
            *status = self.app.clone().oneshot(request).await?.status();
        }
        Ok(statuses)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_read_token_of_a_base_viewer_reads_the_overview_and_a_neighborhood() -> anyhow::Result<()>
{
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.token(None).await;
        let statuses = f.graph_statuses(&token).await?;
        anyhow::ensure!(statuses == [StatusCode::OK; 2], "{statuses:?}");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_token_listing_another_base_gets_404_on_both_graph_reads() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.token(Some(&[Uuid::now_v7()])).await;
        let statuses = f.graph_statuses(&token).await?;
        anyhow::ensure!(statuses == [StatusCode::NOT_FOUND; 2], "{statuses:?}");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}
