//! 个人令牌读图（0014 决定 7）：搜实体、实体详情、认知历史、事实证据、图总览、邻域、本体、
//! 某类的实例这些读路由收任何一枚活着的令牌。角色照旧是上限，`kb_ids` 排除了的库答 404；改实体仍只认会话。
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
    kb: Uuid,
    /// 令牌够不着的另一个库
    other_kb: Uuid,
    reader: Uuid,
    person: Uuid,
    entity: Uuid,
    fact: Uuid,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let id = Uuid::now_v7;
        let (org, ws, reader, kb, other_kb) = (id(), id(), id(), id(), id());
        let (person, knows, entity, peer, fact) = (id(), id(), id(), id(), id());
        // 插进去的只有本地生成的 UUID 和写死的日期
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','token-read-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','token-read-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{reader}','{org}','{reader}@token-read.test','Reader','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES
                 ('{kb}','{ws}','mirror'), ('{other_kb}','{ws}','other');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES
                 ('{kb}','{reader}','viewer'), ('{other_kb}','{reader}','viewer');
             INSERT INTO entity_types(id,kb_id,key,label) VALUES ('{person}','{kb}','person','Person');
             INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES
                 ('{knows}','{kb}','knows','knows','state');
             INSERT INTO entities(id,kb_id,type_id,canonical_name,created_at) VALUES
                 ('{entity}','{kb}','{person}','Zhang San','2026-01-01'),
                 ('{peer}','{kb}','{person}','Li Si','2026-01-01');
             INSERT INTO facts(id,kb_id,subject_id,predicate_id,object_id,recorded_at,confidence)
                 VALUES ('{fact}','{kb}','{entity}','{knows}','{peer}','2026-01-01',0.9);"
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
            kb,
            other_kb,
            reader,
            person,
            entity,
            fact,
            _dir: dir,
        }))
    }

    async fn read_token(&self, kb: Uuid) -> String {
        utopia_store::tokens::issue(&self.pool, self.reader, "ci", "read", Some(&[kb]), None)
            .await
            .expect("issuing a token")
            .1
    }

    fn read_paths(&self) -> [String; 8] {
        let (kb, person, entity, fact) = (self.kb, self.person, self.entity, self.fact);
        [
            format!("/kbs/{kb}/entities?q=Zhang"),
            format!("/kbs/{kb}/entities/{entity}"),
            format!("/kbs/{kb}/entities/{entity}/history"),
            format!("/kbs/{kb}/facts/{fact}/evidence"),
            format!("/kbs/{kb}/graph/overview"),
            format!("/kbs/{kb}/graph/neighborhood?entity={entity}"),
            format!("/kbs/{kb}/ontology"),
            format!("/kbs/{kb}/ontology/entity-types/{person}/entities"),
        ]
    }

    async fn get(&self, path: &str, token: &str) -> anyhow::Result<StatusCode> {
        let request = Request::builder()
            .uri(format!("/api/v1{path}"))
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())?;
        Ok(self.app.clone().oneshot(request).await?.status())
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
async fn a_read_token_of_a_base_viewer_gets_200_on_every_graph_read() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(f.kb).await;
        for path in f.read_paths() {
            let status = f.get(&path, &token).await?;
            anyhow::ensure!(status == StatusCode::OK, "{path}: {status}");
        }
        Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_token_limited_to_another_base_gets_404_on_every_graph_read() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(f.other_kb).await;
        for path in f.read_paths() {
            let status = f.get(&path, &token).await?;
            anyhow::ensure!(status == StatusCode::NOT_FOUND, "{path}: {status}");
        }
        Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}
