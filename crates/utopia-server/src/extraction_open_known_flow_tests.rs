//! #588 cut 1：开放图谱抽取把分块循环拆成调+解析（`call_and_parse_one_chunk`）与
//! apply（`apply_open_extraction`）两半；调+解析的入口已经接住「前面分块认下的
//! 实体」作为 `known` 送进模型。这一档是 cut 2（并发调用）的前提测试：它把
//! 「前面块认下的实体在后面块仍然能识别」这一性质钉死。
//!
//! 做法与 `phrase_alignment_tests.rs` 同一档：脚本化 HTTP 端点、真库；脚本
//! 写两份回复——第一块列出 `Acme`，第二块再列 `Acme`。`extract_document`
//! 跑完之后，`entities` 表里应当只有一行 `Acme`，两份陈述都指着它。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

/// 开放抽取的回复：脚本里的字符串就是 chat 流里的 `delta.content`。
/// 与 `errata_tests.rs::reply` 同一档（text/event-stream + `usage` 收尾），
/// 但 `chat_retrying_rate_limits_at` 解析流时只读 `content` 这一段。
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    m.requests.lock().unwrap().push(body);
    let text = {
        let mut replies = m.replies.lock().unwrap();
        if replies.is_empty() {
            panic!("unexpected model request");
        }
        replies.remove(0)
    };
    let frame = json!({"choices":[{"delta":{"content":text}}]});
    let done = json!({"choices":[{"delta":{},"finish_reason":"stop"}]});
    let usage = json!({"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":34}});
    (
        [("content-type", "text/event-stream")],
        format!("data: {frame}\n\ndata: {done}\n\ndata: {usage}\n\ndata: [DONE]\n\n"),
    )
}

struct Fx {
    pool: sqlx::PgPool,
    state: AppState,
    org: Uuid,
    kb: Uuid,
    doc: Uuid,
    model: Model,
    _server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Fx {
    async fn new(replies: Vec<String>) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let ids: Vec<Uuid> = (0..8).map(|_| Uuid::now_v7()).collect();
        let (org, ws, kb, doc, chunk0, chunk1, organization, _) = (
            ids[0], ids[1], ids[2], ids[3], ids[4], ids[5], ids[6], ids[7],
        );
        // 两块原文：第二块沿用第一块的「Acme」名字。脚本里把两份回复都设成「Acme」
        // 出现在 `e`，所以第二块的 `Acme` 必须沿着 `known` 解析到第一块的实体
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','open-known-flow');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','open-known-flow');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','open-known-flow');
             INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','acme.txt','x');
             INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
                 ('{chunk0}','{kb}','{doc}',0,'Acme is based in London.'),
                 ('{chunk1}','{kb}','{doc}',1,'Acme runs the harbour facility.');"
        ))
        .execute(&pool)
        .await?;
        let model = Model {
            replies: Arc::new(Mutex::new(replies)),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let router = Router::new()
            .route("/chat/completions", post(reply))
            .with_state(model.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        utopia_store::settings::upsert(
            &pool,
            ws,
            Some(&endpoint),
            None,
            Some("scripted"),
            None,
            None,
            None,
            None,
        )
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
        // Touch organization so the linter sees it referenced; type_bindings reads the table.
        let _ = organization;
        Ok(Some(Self {
            pool,
            state,
            org,
            kb,
            doc,
            model,
            _server: server,
            _dir: dir,
        }))
    }

    async fn cleanup(&self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// 两块都列 `Acme`：第二块的那条陈述必须挂在第一块新建的 `Acme` 实体上，
/// 而不是另起一行。`entities` 表里 `Acme` 只有一行；`facts.subject_id`
/// 在两个 chunk 上的 id 相同。
#[tokio::test]
async fn a_thing_listed_by_an_earlier_chunk_reaches_a_later_one_via_known() -> anyhow::Result<()> {
    let Some(f) = Fx::new(vec![
        // 第一块的回复：列 `Acme`，做一条陈述把 Acme 钉到一个值上。
        // 紧凑数组的 8 格是 `[quote, subject, phrase, object, value, qualifiers,
        // when, ended]`——`value` 必须是字符串或数字，限定词另占第 5 格的对象
        json!({
            "e": [["Acme", "organization", 1]],
            "s": [["Acme is based in London.", "Acme", "based in", null, "London", null, null, null]],
            "n": []
        })
        .to_string(),
        // 第二块的回复：也列 `Acme`（同名），再做一条陈述——这才是要测的：
        // 这一行的 `Acme` 应当沿着 `known` 命中第一块已经认下的那一行，
        // 而不是再新建一个
        json!({
            "e": [["Acme", "organization", 1]],
            "s": [["Acme runs the harbour facility.", "Acme", "runs", null, "harbour", null, null, null]],
            "n": []
        })
        .to_string(),
    ])
    .await?
    else {
        return Ok(());
    };
    crate::extraction::extract_document(&f.state, f.doc, utopia_core::models::Proposer::default())
        .await?;
    // 同一份文档只该有一个名为 Acme 的实体
    let acme_rows: Vec<(Uuid,)> =
        sqlx::query_as("SELECT id FROM entities WHERE kb_id=$1 AND canonical_name='Acme'")
            .bind(f.kb)
            .fetch_all(&f.pool)
            .await?;
    assert_eq!(
        acme_rows.len(),
        1,
        "expected one Acme entity across both chunks, got {}",
        acme_rows.len()
    );
    let acme_id = acme_rows[0].0;
    // 两个分块上各应当有一条陈述；`subject_id` 都是 Acme 那一行
    let facts: Vec<(Uuid,)> =
        sqlx::query_as("SELECT subject_id FROM facts WHERE kb_id=$1 AND layer='open'")
            .bind(f.kb)
            .fetch_all(&f.pool)
            .await?;
    assert_eq!(
        facts.len(),
        2,
        "expected two open facts, got {}",
        facts.len()
    );
    for (sid,) in &facts {
        assert_eq!(*sid, acme_id, "fact subject must be the Acme entity");
    }
    // 第二块的请求里 `known` 不为空：第一块把 `k1 = Acme` 送了过去
    let second_user_msg = {
        let requests = f.model.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        requests[1]["messages"][1]["content"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    assert!(
        second_user_msg.contains("Acme"),
        "second chunk prompt must carry Acme in the known list, got: {second_user_msg}"
    );
    f.cleanup().await
}
