//! 个人令牌读得到本体（#1262，0014 决定 7）：本体与某个类下的实例两条读。
//! 人的角色仍是上限，`kb_ids` 排除掉的库照文档原件（0052）回 404。
use crate::state::AppState;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    pool: sqlx::PgPool,
    app: axum::Router,
    org: Uuid,
    /// 两个库的 viewer
    viewer: Uuid,
    kb: Uuid,
    other_kb: Uuid,
    /// `kb` 里的一个类
    entity_type: Uuid,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let [org, ws, viewer, kb, other_kb, entity_type] = [(); 6].map(|_| Uuid::now_v7());
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','token-ontology-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','token-ontology-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{viewer}','{org}','{viewer}@token-ontology.test','Viewer','unused');
             INSERT INTO memberships(user_id,workspace_id,role) VALUES ('{viewer}','{ws}','viewer');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES
                 ('{kb}','{ws}','mirror'), ('{other_kb}','{ws}','other');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES
                 ('{kb}','{viewer}','viewer'), ('{other_kb}','{viewer}','viewer');
             INSERT INTO entity_types(id,kb_id,key,label,color,shape) VALUES
                 ('{entity_type}','{kb}','robot','Robot','#888888','circle');"
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
            other_kb,
            entity_type,
            _dir: dir,
        }))
    }

    async fn read_token(&self, kb_ids: Option<&[Uuid]>) -> anyhow::Result<String> {
        Ok(
            utopia_store::tokens::issue(&self.pool, self.viewer, "ci", "read", kb_ids, None)
                .await?
                .1,
        )
    }

    /// 本体与 `kb` 里那个类的实例，两条读各回一次
    async fn read_ontology(&self, token: &str) -> anyhow::Result<[(StatusCode, Value); 2]> {
        let (kb, entity_type) = (self.kb, self.entity_type);
        Ok([
            self.get(&format!("/kbs/{kb}/ontology"), token).await?,
            self.get(
                &format!("/kbs/{kb}/ontology/entity-types/{entity_type}/entities"),
                token,
            )
            .await?,
        ])
    }

    async fn get(&self, path: &str, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        let request = Request::get(format!("/api/v1{path}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())?;
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
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
async fn a_read_token_of_a_base_viewer_reads_its_ontology_and_a_class_instances(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(None).await?;
        let [(status, ontology), (instances_status, instances)] = f.read_ontology(&token).await?;
        anyhow::ensure!(
            status == StatusCode::OK && ontology["entity_types"][0]["key"] == "robot",
            "ontology: {status} {ontology}"
        );
        anyhow::ensure!(
            instances_status == StatusCode::OK && instances["total"] == 0,
            "instances: {instances_status} {instances}"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_token_listing_another_base_gets_404_on_both_ontology_reads() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(Some(&[f.other_kb])).await?;
        for (status, body) in f.read_ontology(&token).await? {
            anyhow::ensure!(status == StatusCode::NOT_FOUND, "{status} {body}");
        }
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}
