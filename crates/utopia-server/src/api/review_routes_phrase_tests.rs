//! 人定一条短语签名：判定和它的重算任务一次提交，请求答 202 和 job id，不编数字（0051）。
//!
//! 三件事：路由答 202、job 排着、绑定写成人判的；`GET /kbs/{id}/jobs/{job_id}` 在本库能读、
//! 换个库答 404；Viewer 能读状态但不能判。没有 `UTOPIA_DATABASE_URL` 时跳过。
use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use utopia_store::phrase_bindings;
use uuid::Uuid;

struct Fx {
    pool: sqlx::PgPool,
    app: axum::Router,
    org: Uuid,
    kb: Uuid,
    other_kb: Uuid,
    editor: String,
    viewer: String,
    binding: Uuid,
    _dir: tempfile::TempDir,
}

impl Fx {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, other_kb, editor, viewer, subject, object, property, statement) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // Only locally generated UUIDs are interpolated into fixture SQL.
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','phrase-route-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','phrase-route-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{editor}','{org}','{editor}@phrase.test','editor','unused'),
                 ('{viewer}','{org}','{viewer}@phrase.test','viewer','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES
                 ('{kb}','{ws}','phrases'), ('{other_kb}','{ws}','other');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES
                 ('{kb}','{editor}','editor'), ('{kb}','{viewer}','viewer'),
                 ('{other_kb}','{editor}','editor');
             INSERT INTO entities(id,kb_id,canonical_name) VALUES
                 ('{subject}','{kb}','Acme'), ('{object}','{kb}','London');
             INSERT INTO relation_types(id,kb_id,key,label,temporal) VALUES
                 ('{property}','{kb}','based_in','based in','state');
             INSERT INTO facts(id,kb_id,subject_id,object_id,layer,phrase) VALUES
                 ('{statement}','{kb}','{subject}','{object}','open','based in');"
        ))
        .execute(&pool)
        .await?;
        // 队列里的一条：代理判成 undecided，人来定
        let sig = phrase_bindings::signatures(&pool, kb).await?.remove(0);
        phrase_bindings::decide(
            &pool,
            kb,
            &sig,
            phrase_bindings::Decision {
                relation_type_id: None,
                direction: None,
                status: "undecided",
                votes: &json!({}),
                decided_by: "agent",
            },
        )
        .await?;
        let binding: Uuid = sqlx::query_scalar("SELECT id FROM phrase_bindings WHERE kb_id=$1")
            .bind(kb)
            .fetch_one(&pool)
            .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
        let editor_token = crate::auth::issue_token(&state, editor)?;
        let viewer_token = crate::auth::issue_token(&state, viewer)?;
        let app = super::super::router(state, &cfg);
        Ok(Some(Self {
            pool,
            app,
            org,
            kb,
            other_kb,
            editor: editor_token,
            viewer: viewer_token,
            binding,
            _dir: dir,
        }))
    }

    async fn call(
        &self,
        token: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("Authorization", format!("Bearer {token}"));
        let request = match body {
            Some(b) => request
                .header("Content-Type", "application/json")
                .body(Body::from(b.to_string()))?,
            None => request.body(Body::empty())?,
        };
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)?
        };
        Ok((status, value))
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM jobs WHERE payload->>'kb_id'=$1")
            .bind(self.kb.to_string())
            .execute(&self.pool)
            .await?;
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_phrase_decision_is_accepted_with_its_job() -> anyhow::Result<()> {
    let Some(f) = Fx::new().await? else {
        return Ok(());
    };
    let run = async {
        let path = format!(
            "/api/v1/kbs/{}/review/alignment/phrases/{}",
            f.kb, f.binding
        );
        let (status, body) = f
            .call(
                &f.editor,
                "POST",
                &path,
                Some(json!({ "property": "based_in", "direction": "forward" })),
            )
            .await?;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["status"], json!("accepted"));
        assert!(body.get("typed").is_none(), "no invented counts: {body}");
        let job_id = body["job_id"].as_i64().expect("job id");

        // job 排着，载荷是这个库；绑定是人判的、bound
        let (kind, job_status, payload): (String, String, Value) =
            sqlx::query_as("SELECT kind, status, payload FROM jobs WHERE id=$1")
                .bind(job_id)
                .fetch_one(&f.pool)
                .await?;
        assert_eq!(kind, phrase_bindings::MATERIALIZE_KIND);
        assert_eq!(job_status, "queued");
        assert_eq!(payload["kb_id"], json!(f.kb));
        let b = phrase_bindings::bindings(&f.pool, f.kb).await?.remove(0);
        assert_eq!(
            (b.status.as_str(), b.decided_by.as_str()),
            ("bound", "person")
        );
        // 这次请求没有重算：投影要等 job
        assert_eq!(utopia_store::materialize::count(&f.pool, f.kb).await?, 0);

        // 状态读：本库 200，换库 404，Viewer 也能读
        let (status, body) = f
            .call(
                &f.editor,
                "GET",
                &format!("/api/v1/kbs/{}/jobs/{job_id}", f.kb),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["job"]["status"], json!("queued"));
        assert_eq!(
            body["job"]["kind"],
            json!(phrase_bindings::MATERIALIZE_KIND)
        );
        let (status, _) = f
            .call(
                &f.editor,
                "GET",
                &format!("/api/v1/kbs/{}/jobs/{job_id}", f.other_kb),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = f
            .call(
                &f.viewer,
                "GET",
                &format!("/api/v1/kbs/{}/jobs/{job_id}", f.kb),
                None,
            )
            .await?;
        assert_eq!(status, StatusCode::OK);

        // Viewer 不能判
        let (status, _) = f
            .call(
                &f.viewer,
                "POST",
                &path,
                Some(json!({ "property": null, "direction": "forward" })),
            )
            .await?;
        assert_eq!(status, StatusCode::FORBIDDEN);

        // 让 job 该做的事在这里做一遍：读当前绑定，算出一条类型化行
        let outcome = utopia_store::materialize::try_materialize(&f.pool, f.kb)
            .await?
            .expect("nobody holds the lock");
        assert_eq!(outcome.added, 1);
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    run
}
