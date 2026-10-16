//! 写权限的个人令牌能把一个库从无到有搭起来（#1243）：建库、建来源、读推送密钥、
//! 导入本体、定类别词。令牌照旧只收窄——人的角色是上限，scope 与 `kb_ids` 再往下压。
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
    ws: Uuid,
    /// 工作区 admin：建得了库
    admin: Uuid,
    /// 工作区 viewer：令牌勾了 write 也建不了
    viewer: Uuid,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, admin, viewer) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','token-setup-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','token-setup-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash) VALUES
                 ('{admin}','{org}','{admin}@token-setup.test','Admin','unused'),
                 ('{viewer}','{org}','{viewer}@token-setup.test','Viewer','unused');
             INSERT INTO memberships(user_id,workspace_id,role) VALUES
                 ('{admin}','{ws}','admin'), ('{viewer}','{ws}','viewer');"
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
            ws,
            admin,
            viewer,
            _dir: dir,
        }))
    }

    async fn token(&self, user: Uuid, scope: &str, kb_ids: Option<&[Uuid]>) -> String {
        utopia_store::tokens::issue(&self.pool, user, "ci", scope, kb_ids, None)
            .await
            .expect("issuing a token")
            .1
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let request = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"))
            .header("authorization", format!("Bearer {token}"));
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?,
            None => request.body(Body::empty())?,
        };
        self.respond(request).await
    }

    /// 本体导入收 multipart。空表单在读上传那一步就会被拒（`no_files`），
    /// 够用来分辨「令牌被挡在门外」与「进了门、请求本身不对」
    async fn import_nothing(&self, kb: Uuid, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        let request = Request::post(format!("/api/v1/kbs/{kb}/ontology/imports"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "multipart/form-data; boundary=x")
            .body(Body::from("--x--\r\n"))?;
        self.respond(request).await
    }

    /// 真导入一份 Turtle：字段要带文件名，读上传只收带文件名的，同网页的导入面板
    async fn import_turtle(
        &self,
        kb: Uuid,
        token: &str,
        ttl: &str,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let body = format!(
            "--x\r\nContent-Disposition: form-data; name=\"file\"; filename=\"o.ttl\"\r\n\r\n\
             {ttl}\r\n--x--\r\n"
        );
        let request = Request::post(format!("/api/v1/kbs/{kb}/ontology/imports"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "multipart/form-data; boundary=x")
            .body(Body::from(body))?;
        self.respond(request).await
    }

    async fn respond(&self, request: Request<Body>) -> anyhow::Result<(StatusCode, Value)> {
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, body))
    }

    async fn create_kb(&self, token: &str, name: &str) -> anyhow::Result<(StatusCode, Value)> {
        let ws = self.ws;
        self.send(
            "POST",
            &format!("/workspaces/{ws}/kbs"),
            token,
            Some(json!({ "name": name })),
        )
        .await
    }

    async fn create_source(&self, kb: Uuid, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        self.create_source_of_kind(kb, token, "statements").await
    }

    async fn create_source_of_kind(
        &self,
        kb: Uuid,
        token: &str,
        kind: &str,
    ) -> anyhow::Result<(StatusCode, Value)> {
        self.send(
            "POST",
            &format!("/kbs/{kb}/sources"),
            token,
            Some(json!({ "kind": kind, "name": "ci" })),
        )
        .await
    }

    async fn push_key(
        &self,
        kb: Uuid,
        source: Uuid,
        token: &str,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let path = format!("/kbs/{kb}/sources/{source}/token");
        self.send("GET", &path, token, None).await
    }

    async fn decide_kind_word(&self, kb: Uuid, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        let path = format!("/kbs/{kb}/review/alignment/kind-words/robot");
        self.send("POST", &path, token, Some(json!({}))).await
    }

    async fn list_kbs(&self, token: &str) -> anyhow::Result<(StatusCode, Value)> {
        let ws = self.ws;
        self.send("GET", &format!("/workspaces/{ws}/kbs"), token, None)
            .await
    }

    /// 直接写库建一个 admin 名下的库：只测令牌边界的用例不必先走一遍建库
    async fn seeded_kb(&self, name: &str) -> anyhow::Result<Uuid> {
        let kb = Uuid::now_v7();
        sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ($1,$2,$3)")
            .bind(kb)
            .bind(self.ws)
            .bind(name)
            .execute(&self.pool)
            .await?;
        sqlx::query("INSERT INTO kb_members(kb_id,user_id,role) VALUES ($1,$2,'admin')")
            .bind(kb)
            .bind(self.admin)
            .execute(&self.pool)
            .await?;
        Ok(kb)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        // 任务按载荷里的 kb_id 挂在库上，没有外键，删组织带不走它们
        sqlx::query(
            "DELETE FROM jobs WHERE payload->>'kb_id' IN
                 (SELECT id::text FROM knowledge_bases WHERE workspace_id = $1)",
        )
        .bind(self.ws)
        .execute(&self.pool)
        .await?;
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[tokio::test]
async fn a_live_write_token_of_a_workspace_admin_sets_up_a_base_and_a_dead_one_does_not(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.token(f.admin, "write", None).await;
        let token_id = utopia_store::tokens::authenticate(&f.pool, &token)
            .await?
            .token_id;

        let (status, kb) = f.create_kb(&token, "lore-mirror").await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {kb}");
        let kb: Uuid = serde_json::from_value(kb["id"].clone())?;

        let (status, created) = f.create_source(kb, &token).await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {created}");
        let source: Uuid = serde_json::from_value(created["source"]["id"].clone())?;

        let (status, key) = f.push_key(kb, source, &token).await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {key}");
        anyhow::ensure!(
            key["ingest_token"] == created["ingest_token"] && key["ingest_token"].is_string(),
            "{key} vs {created}"
        );

        // 进了门、请求本身不对：空上传过了令牌与角色，停在读上传那一步
        let (status, body) = f.import_nothing(kb, &token).await?;
        anyhow::ensure!(body["code"] == "no_files", "{status} {body}");
        let (status, body) = f.decide_kind_word(kb, &token).await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {body}");

        // 审计记的是发令牌的人，与会话一样
        let actors: Vec<(String, Option<Uuid>)> = sqlx::query_as(
            "SELECT action, actor_id FROM audit_events
              WHERE kb_id = $1 AND action IN ('source.created', 'alignment.kind_word_decided')
              ORDER BY action",
        )
        .bind(kb)
        .fetch_all(&f.pool)
        .await?;
        anyhow::ensure!(
            actors
                == [
                    ("alignment.kind_word_decided".to_string(), Some(f.admin)),
                    ("source.created".to_string(), Some(f.admin)),
                ],
            "{actors:?}"
        );
        // 哪一枚令牌代人做的（决定 5 靠它）：记在 detail 里
        let detail: Value = sqlx::query_scalar(
            "SELECT detail FROM audit_events WHERE kb_id = $1 AND action = 'source.created'",
        )
        .bind(kb)
        .fetch_one(&f.pool)
        .await?;
        anyhow::ensure!(detail["token_id"] == json!(token_id), "{detail}");

        // 同一个人、同一条刚走通的路：过期的与撤销的令牌照旧挡在门外
        let (_, expired) = utopia_store::tokens::issue(
            &f.pool,
            f.admin,
            "expired",
            "write",
            None,
            Some(chrono::Utc::now() - chrono::Duration::minutes(1)),
        )
        .await?;
        let (revoked_view, revoked) =
            utopia_store::tokens::issue(&f.pool, f.admin, "revoked", "write", None, None).await?;
        utopia_store::tokens::revoke(&f.pool, f.admin, revoked_view.id).await?;
        for token in [expired, revoked] {
            let (status, body) = f.create_source(kb, &token).await?;
            anyhow::ensure!(status == StatusCode::UNAUTHORIZED, "{status} {body}");
        }
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_write_token_creating_a_url_source_is_refused_with_token_source_kind(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let kb = f.seeded_kb("pull").await?;
        let token = f.token(f.admin, "write", None).await;

        let (status, body) = f.create_source_of_kind(kb, &token, "url").await?;
        anyhow::ensure!(
            status == StatusCode::FORBIDDEN && body["code"] == "token_source_kind",
            "{status} {body}"
        );
        let sources: i64 = sqlx::query_scalar("SELECT count(*) FROM sources WHERE kb_id = $1")
            .bind(kb)
            .fetch_one(&f.pool)
            .await?;
        anyhow::ensure!(sources == 0, "{sources} sources");
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_read_token_lists_bases_and_is_refused_on_every_setup_route() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let kb = f.seeded_kb("existing").await?;
        let source = Uuid::now_v7();
        sqlx::query("INSERT INTO sources(id,kb_id,kind,name) VALUES ($1,$2,'statements','ci')")
            .bind(source)
            .bind(kb)
            .execute(&f.pool)
            .await?;
        let token = f.token(f.admin, "read", None).await;

        let (status, listed) = f.list_kbs(&token).await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {listed}");
        anyhow::ensure!(listed[0]["name"] == "existing", "{listed}");

        for (route, (status, body)) in [
            ("create base", f.create_kb(&token, "nope").await?),
            ("create source", f.create_source(kb, &token).await?),
            ("read push key", f.push_key(kb, source, &token).await?),
            ("import ontology", f.import_nothing(kb, &token).await?),
            ("decide kind word", f.decide_kind_word(kb, &token).await?),
        ] {
            anyhow::ensure!(
                status == StatusCode::FORBIDDEN && body["code"] == "token_read_only",
                "{route}: {status} {body}"
            );
        }
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_write_token_reaches_no_further_than_its_owners_role_and_its_base_list(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        // 工作区 viewer：令牌勾了 write，建库照旧要工作区 admin
        let viewers = f.token(f.viewer, "write", None).await;
        let (status, body) = f.create_kb(&viewers, "nope").await?;
        anyhow::ensure!(status == StatusCode::FORBIDDEN, "viewer: {status} {body}");

        let (a, b) = (f.seeded_kb("a").await?, f.seeded_kb("b").await?);
        let token = f.token(f.admin, "write", Some(&[a])).await;

        let (status, body) = f.create_source(a, &token).await?;
        anyhow::ensure!(status == StatusCode::OK, "base a: {status} {body}");

        let (status, body) = f.create_source(b, &token).await?;
        anyhow::ensure!(
            status == StatusCode::FORBIDDEN && body["code"] == "token_kb_out_of_scope",
            "base b: {status} {body}"
        );
        let (status, body) = f.create_kb(&token, "new").await?;
        anyhow::ensure!(
            status == StatusCode::FORBIDDEN && body["code"] == "token_kb_out_of_scope",
            "new base: {status} {body}"
        );

        let (status, listed) = f.list_kbs(&token).await?;
        let names: Vec<&str> = listed
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|kb| kb["name"].as_str())
            .collect();
        anyhow::ensure!(
            status == StatusCode::OK && names == ["a"],
            "{status} {listed}"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

/// 文档先到、本体后导（建库对话框说本体以后还能再导）：没有类时类别词判成「没有」，
/// 短语签名对不上属性。导进来的类与属性要接住它们就得重排对齐，同编辑器的入口一样先排
/// `embed_ontology` 再排对齐；不排的话要等下一篇文档，最后一篇之后导进来的类没有实例
#[tokio::test]
async fn importing_an_ontology_queues_alignment_behind_the_index_refresh() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let kb = f.seeded_kb("robots").await?;
        let token = f.token(f.admin, "write", None).await;
        let ttl = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                   @prefix ex: <http://example.org/> .\n\
                   ex:Robot a owl:Class ; rdfs:label \"Robot\" .\n";
        let (status, body) = f.import_turtle(kb, &token, ttl).await?;
        anyhow::ensure!(status == StatusCode::OK, "{status} {body}");

        let queued: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM jobs WHERE status = 'queued' AND payload->>'kb_id' = $1
              ORDER BY run_at, id",
        )
        .bind(kb.to_string())
        .fetch_all(&f.pool)
        .await?;
        anyhow::ensure!(
            queued == ["embed_ontology", "align_types"],
            "the alignment is queued behind the index refresh: {queued:?}"
        );
        anyhow::Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}
