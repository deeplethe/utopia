//! 个人令牌读得到一份文档的抽取结果，也导得出整个库（#1262，0014 决定 7）。
//! 库的 viewer 就够；令牌的 `kb_ids` 排除掉的库一律 404（0052）。
use crate::state::AppState;
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    pool: sqlx::PgPool,
    app: axum::Router,
    org: Uuid,
    /// 库的 viewer
    viewer: Uuid,
    kb: Uuid,
    doc: Uuid,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, viewer, kb, doc) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','token-export-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','token-export-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{viewer}','{org}','{viewer}@token-export.test','Viewer','unused');
             INSERT INTO memberships(user_id,workspace_id,role) VALUES ('{viewer}','{ws}','viewer');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','mirror');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{viewer}','viewer');
             INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','spec.md','x');"
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
            doc,
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

    async fn extractions(&self, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        self.get(&format!("/documents/{}/extractions", self.doc), token)
            .await
    }

    async fn export(&self, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        self.get(&format!("/kbs/{}/export?format=turtle", self.kb), token)
            .await
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
async fn a_read_token_of_a_base_viewer_reads_extractions_and_exports_with_its_id_audited(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(None).await?;
        let token_id = utopia_store::tokens::authenticate(&f.pool, &token)
            .await?
            .token_id;

        let (status, body) = f.extractions(&token).await?;
        anyhow::ensure!(
            status == StatusCode::OK && body["facts"].is_array(),
            "{status} {body}"
        );
        let (status, body) = f.export(&token).await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {body}");

        let (actor, detail): (Option<Uuid>, Value) = sqlx::query_as(
            "SELECT actor_id, detail FROM audit_events WHERE kb_id = $1 AND action = 'kb.exported'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        anyhow::ensure!(
            actor == Some(f.viewer) && detail == json!({ "format": "ttl", "token_id": token_id }),
            "{actor:?} {detail}"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_token_whose_kb_ids_name_another_base_gets_404_on_extractions_and_export(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(Some(&[Uuid::now_v7()])).await?;
        for (route, (status, body)) in [
            ("extractions", f.extractions(&token).await?),
            ("export", f.export(&token).await?),
        ] {
            anyhow::ensure!(status == StatusCode::NOT_FOUND, "{route}: {status} {body}");
        }
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}
