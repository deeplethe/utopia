//! 个人令牌读图（0014 决定 7）：搜实体、实体详情、认知历史、事实证据、图总览、邻域、本体、
//! 某类的实例、文档的抽取结果、整库导出和分块检索这些读路由收任何一枚活着的令牌。角色照旧是上限，`kb_ids` 排除了的库答 404；改实体仍只认会话。
use crate::state::AppState;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::json;
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
    doc: Uuid,
    search: Arc<utopia_search::SearchIndex>,
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
        let (person, knows, entity, peer, fact, doc) = (id(), id(), id(), id(), id(), id());
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
                 VALUES ('{fact}','{kb}','{entity}','{knows}','{peer}','2026-01-01',0.9);
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
        let state = AppState::new(pool.clone(), &cfg, search.clone(), "test-only".into());
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
            doc,
            search,
            _dir: dir,
        }))
    }

    async fn read_token(&self, kb: Uuid) -> String {
        utopia_store::tokens::issue(&self.pool, self.reader, "ci", "read", Some(&[kb]), None)
            .await
            .expect("issuing a token")
            .1
    }

    fn read_paths(&self) -> [(Method, String); 11] {
        let (kb, person, entity, fact, doc) =
            (self.kb, self.person, self.entity, self.fact, self.doc);
        [
            (Method::GET, format!("/kbs/{kb}/entities?q=Zhang")),
            (Method::GET, format!("/kbs/{kb}/entities/{entity}")),
            (Method::GET, format!("/kbs/{kb}/entities/{entity}/history")),
            (Method::GET, format!("/kbs/{kb}/facts/{fact}/evidence")),
            (Method::GET, format!("/kbs/{kb}/graph/overview")),
            (
                Method::GET,
                format!("/kbs/{kb}/graph/neighborhood?entity={entity}"),
            ),
            (Method::GET, format!("/kbs/{kb}/ontology")),
            (
                Method::GET,
                format!("/kbs/{kb}/ontology/entity-types/{person}/entities"),
            ),
            (Method::GET, format!("/documents/{doc}/extractions")),
            (Method::GET, format!("/kbs/{kb}/export?format=turtle")),
            (Method::POST, format!("/kbs/{kb}/search")),
        ]
    }

    /// 每个请求都带同一个检索体；GET 路由不读它
    async fn send(&self, method: Method, path: &str, token: &str) -> anyhow::Result<StatusCode> {
        self.send_json(method, path, token, r#"{"q":"Zhang"}"#)
            .await
    }

    async fn send_json(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: &str,
    ) -> anyhow::Result<StatusCode> {
        let request = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))?;
        Ok(self.app.clone().oneshot(request).await?.status())
    }

    async fn search(
        &self,
        token: &str,
        body: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let request = Request::post(format!("/api/v1/kbs/{}/search", self.kb))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))?;
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
        Ok(json!({ "status": status, "body": body }))
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
        for (method, path) in f.read_paths() {
            let status = f.send(method, &path, &token).await?;
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
        for (method, path) in f.read_paths() {
            let status = f.send(method, &path, &token).await?;
            anyhow::ensure!(status == StatusCode::NOT_FOUND, "{path}: {status}");
        }
        Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn an_export_by_token_is_audited_under_its_owner_with_the_token_id() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let token = f.read_token(f.kb).await;
        let token_id = utopia_store::tokens::authenticate(&f.pool, &token)
            .await?
            .token_id;
        let path = format!("/kbs/{}/export?format=turtle", f.kb);
        let status = f.send(Method::GET, &path, &token).await?;
        anyhow::ensure!(status == StatusCode::OK, "{path}: {status}");

        let audited: (Option<Uuid>, serde_json::Value) = sqlx::query_as(
            "SELECT actor_id, detail FROM audit_events WHERE kb_id = $1 AND action = 'kb.exported'",
        )
        .bind(f.kb)
        .fetch_one(&f.pool)
        .await?;
        let expected = (
            Some(f.reader),
            json!({ "format": "ttl", "token_id": token_id }),
        );
        anyhow::ensure!(audited == expected, "{audited:?}");
        Ok(())
    }
    .await;
    f.cleanup().await?;
    result
}

#[tokio::test]
async fn a_search_past_depth_100_or_with_an_unknown_filters_field_gets_422() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let token = f.read_token(f.kb).await;
    let path = format!("/kbs/{}/search", f.kb);
    let mut statuses = Vec::new();
    for body in [
        r#"{"q":"Zhang","top_k":50,"offset":51}"#,
        r#"{"q":"Zhang","filters":{"source_id":"x"}}"#,
    ] {
        statuses.push(f.send_json(Method::POST, &path, &token, body).await);
    }
    f.cleanup().await?;
    let statuses = statuses.into_iter().collect::<anyhow::Result<Vec<_>>>()?;
    assert_eq!(statuses, [StatusCode::UNPROCESSABLE_ENTITY; 2]);
    Ok(())
}

/// 一个 api 来源推了 `identities` 里的每一篇，都讲 harvest；另一个库有个来源
async fn seed_pushed_and_foreign_sources(
    f: &Fixture,
    identities: &[&str],
) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (pushed, custom, foreign) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let (kb, other_kb) = (f.kb, f.other_kb);
    sqlx::raw_sql(&format!(
        "INSERT INTO sources(id,kb_id,kind,name) VALUES
             ('{pushed}','{kb}','api','ci'), ('{custom}','{kb}','custom','poll'),
             ('{foreign}','{other_kb}','api','ci');"
    ))
    .execute(&f.pool)
    .await?;
    for identity in identities {
        let (doc, chunk) = (Uuid::now_v7(), Uuid::now_v7());
        sqlx::query(
            "INSERT INTO documents(id,kb_id,source_id,external_key,filename,sha256)
             VALUES($1,$2,$3,$4,$5,$5)",
        )
        .bind(doc)
        .bind(kb)
        .bind(pushed)
        .bind(format!("api:{identity}"))
        .bind(identity)
        .execute(&f.pool)
        .await?;
        sqlx::query(
            "INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,0,'harvest notes')",
        )
        .bind(chunk)
        .bind(kb)
        .bind(doc)
        .execute(&f.pool)
        .await?;
        f.search.reindex_document(
            &kb.to_string(),
            &doc.to_string(),
            &[(chunk.to_string(), "harvest notes".into())],
        )?;
    }
    Ok((pushed, custom, foreign))
}

fn found_external_ids(found: &serde_json::Value) -> serde_json::Value {
    let mut ids: Vec<_> = found["body"]["results"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|hit| hit["external_id"].clone())
        .collect();
    ids.sort_by_key(|id| id.to_string());
    json!({ "status": found["status"], "external_ids": ids })
}

#[tokio::test]
async fn a_search_by_source_and_external_id_prefix_docs_finds_only_docs_a_md() -> anyhow::Result<()>
{
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let (pushed, _, _) =
            seed_pushed_and_foreign_sources(&f, &["docs/a.md", "code/b.rs"]).await?;
        let token = f.read_token(f.kb).await;
        let all = f
            .search(&token, json!({ "q": "harvest", "source_id": pushed }))
            .await?;
        let docs = f
            .search(
                &token,
                json!({ "q": "harvest", "source_id": pushed, "external_id_prefix": "docs/" }),
            )
            .await?;
        anyhow::Ok((all, docs))
    }
    .await;
    f.cleanup().await?;
    let (all, docs) = result?;
    assert_eq!(
        found_external_ids(&all),
        json!({ "status": 200, "external_ids": ["code/b.rs", "docs/a.md"] })
    );
    assert_eq!(
        found_external_ids(&docs),
        json!({ "status": 200, "external_ids": ["docs/a.md"] })
    );
    Ok(())
}

#[tokio::test]
async fn a_search_by_another_bases_source_gets_404_and_a_prefix_without_a_push_source_gets_422(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let (_, custom, foreign) = seed_pushed_and_foreign_sources(&f, &[]).await?;
        let token = f.read_token(f.kb).await;
        let mut codes = Vec::new();
        for body in [
            json!({ "q": "harvest", "source_id": foreign }),
            json!({ "q": "harvest", "external_id_prefix": "docs/" }),
            json!({ "q": "harvest", "source_id": custom, "external_id_prefix": "docs/" }),
        ] {
            let found = f.search(&token, body).await?;
            codes.push(json!([found["status"], found["body"]["code"]]));
        }
        anyhow::Ok(codes)
    }
    .await;
    f.cleanup().await?;
    assert_eq!(
        result?,
        vec![
            json!([404, null]),
            json!([422, "prefix_needs_push_source"]),
            json!([422, "prefix_needs_push_source"]),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn an_external_id_prefix_docs_underscore_finds_docs_a_md_but_not_docsxa_md(
) -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let result = async {
        let (pushed, _, _) = seed_pushed_and_foreign_sources(
            &f,
            &["docs_a.md", "docsXa.md", "50%off.md", "50 percent off.md"],
        )
        .await?;
        let token = f.read_token(f.kb).await;
        let mut found = Vec::new();
        for prefix in ["docs_", "50%"] {
            let body = json!({ "q": "harvest", "source_id": pushed, "external_id_prefix": prefix });
            found.push(found_external_ids(&f.search(&token, body).await?));
        }
        anyhow::Ok(found)
    }
    .await;
    f.cleanup().await?;
    assert_eq!(
        result?,
        [
            json!({ "status": 200, "external_ids": ["docs_a.md"] }),
            json!({ "status": 200, "external_ids": ["50%off.md"] }),
        ]
    );
    Ok(())
}
