//! 相对的时间词从它自己那一节的日期起算（0064 决定 1、4）。
//!
//! 一篇文档收了两份周报，每一节有自己的提报日期，每一节里都有一句「上周……」。抽取每一块
//! 报上它那一段说到的日期（`t`），服务端从那一块正文里的标题算出这条日期管到哪；解析时
//! 每条时间词按它所在的那一节找起算点。所以两句「上周」算出来差一周，而不是都从第一份
//! 周报的日期起算。模型照字报的粒度 `week` 落在日这一档。事后不再另问一次开头。
//!
//! 块与块之间什么都不传：两块的提示词里都没有对方的日期（#588，块并行抽的时候照样成立）。
//!
//! 脚本化的模型端点、真库，与 `extraction_open_known_flow_tests.rs` 同一个做法。
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<Value>>>,
}

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
    (
        [("content-type", "text/event-stream")],
        format!("data: {frame}\n\ndata: {done}\n\ndata: [DONE]\n\n"),
    )
}

const WEEK_35: &str =
    "# 周报汇编\n\n## 第35周周报\n\n提报日期：2026年8月28日\n\n上周，码表一代在韩国市场开售。";
const WEEK_36: &str =
    "# 周报汇编\n\n## 第36周周报\n\n提报日期：2026年9月4日\n\n上周，码表一代在泰国市场开售。";

#[tokio::test]
async fn last_week_is_counted_from_the_date_of_its_own_section() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let ids: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, doc, chunk0, chunk1) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','time-context');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','time-context');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','time-context');
         INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','周报汇编.md','x');"
    ))
    .execute(&pool)
    .await?;
    for (id, seq, text) in [(chunk0, 0, WEEK_35), (chunk1, 1, WEEK_36)] {
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES ($1,$2,$3,$4,$5)")
            .bind(id)
            .bind(kb)
            .bind(doc)
            .bind(seq)
            .bind(text)
            .execute(&pool)
            .await?;
    }
    let anchored = json!({
        "kind": "anchored", "anchor": {"kind": "document"},
        "offset": {"count": 1, "unit": "week", "direction": "before"}
    });
    let model = Model {
        replies: Arc::new(Mutex::new(vec![
            json!({
                "e": [["码表一代", "产品", 1], ["韩国市场", "市场", 1]],
                "s": [["上周，码表一代在韩国市场开售。", "码表一代", "开售于", "韩国市场", null, null, "上周", null]],
                "n": [],
                "t": [["now", "提报日期", "2026年8月28日", {"y": 2026, "m": 8, "d": 28}, null]]
            })
            .to_string(),
            json!({
                "e": [["泰国市场", "市场", 1]],
                "s": [["上周，码表一代在泰国市场开售。", "码表一代", "开售于", "泰国市场", null, null, "上周", null]],
                "n": [],
                "t": [
                    ["now", "提报日期", "2026年9月4日", {"y": 2026, "m": 9, "d": 4}, null],
                    // 字不在这一块里的日期不算
                    ["date", "数据截止", "2026年8月31日", {"y": 2026, "m": 8, "d": 31}, null]
                ]
            })
            .to_string(),
            // 解析：两条「上周」字和句子都不同，各是一条；粒度照字报 week
            json!({ "m": [[0, "point", anchored, "week"], [1, "point", anchored, "week"]] })
                .to_string(),
        ])),
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

    let run = async {
        crate::extraction::extract_document(&state, doc, utopia_core::models::Proposer::default())
            .await?;
        // 文档上存下了两条起算点，各带它所在的标题路径；字不在块里的那条没进来
        let context: DocumentDating = serde_json::from_value(
            sqlx::query_scalar::<_, Value>("SELECT time_context FROM documents WHERE id=$1")
                .bind(doc)
                .fetch_one(&pool)
                .await?,
        )?;
        let entries: Vec<(String, String, Vec<String>)> = context
            .entries
            .iter()
            .map(|e| (e.kind.clone(), e.words.clone(), e.scope.clone()))
            .collect();
        assert_eq!(
            entries,
            vec![
                (
                    "now".to_string(),
                    "2026年8月28日".to_string(),
                    vec!["周报汇编".to_string(), "第35周周报".to_string()]
                ),
                (
                    "now".to_string(),
                    "2026年9月4日".to_string(),
                    vec!["周报汇编".to_string(), "第36周周报".to_string()]
                ),
            ]
        );
        // 块与块之间什么都没传：第二块的提示词里没有第一块的日期之外的东西可依赖——
        // 它自己的正文里本来就有自己的标题和日期
        let second = model.requests.lock().unwrap()[1]["messages"][1]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();
        assert!(second.contains("第36周周报") && second.contains("2026年9月4日"));

        resolve_document(&state, doc).await?;
        assert_eq!(
            model.requests.lock().unwrap().len(),
            3,
            "two extraction calls and one interpretation call: the opening is not asked for a date"
        );
        let dated: Vec<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
            "SELECT o.canonical_name, f.valid_from, f.valid_from_grade
               FROM facts f JOIN entities o ON o.id = f.object_id
              WHERE f.kb_id = $1 AND f.layer = 'open' ORDER BY o.canonical_name",
        )
        .bind(kb)
        .fetch_all(&pool)
        .await?;
        let day = |y, m, d| Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap();
        // 每条陈述由它自己那一节的提报日期作证，连那条日期的名字；不是处理文档的那一刻
        let attested: Vec<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
            "SELECT o.canonical_name, f.attested_from, f.attested_by
               FROM facts f JOIN entities o ON o.id = f.object_id
              WHERE f.kb_id = $1 AND f.layer = 'open' ORDER BY o.canonical_name",
        )
        .bind(kb)
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            attested,
            vec![
                (
                    "泰国市场".to_string(),
                    Some(day(2026, 9, 4)),
                    Some("提报日期 2026年9月4日".to_string())
                ),
                (
                    "韩国市场".to_string(),
                    Some(day(2026, 8, 28)),
                    Some("提报日期 2026年8月28日".to_string())
                ),
            ]
        );
        assert_eq!(
            dated,
            vec![
                // 第 36 周的「上周」从 9 月 4 日起算
                (
                    "泰国市场".to_string(),
                    Some(day(2026, 8, 28)),
                    Some("B".to_string())
                ),
                // 第 35 周的「上周」从 8 月 28 日起算
                (
                    "韩国市场".to_string(),
                    Some(day(2026, 8, 21)),
                    Some("B".to_string())
                ),
            ]
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    server.abort();
    sqlx::query("DELETE FROM jobs WHERE payload->>'document_id'=$1 OR payload->>'kb_id'=$2")
        .bind(doc.to_string())
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}

/// 文档没说自己是哪天的：抽取报了「没有」，就是没有。不再另问一次开头；相对的时间词
/// 锚不到，等着（C 级），陈述没有见证。
#[tokio::test]
async fn a_document_that_states_no_date_is_not_asked_for_one() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let ids: Vec<Uuid> = (0..5).map(|_| Uuid::now_v7()).collect();
    let (org, ws, kb, doc, chunk) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','time-context-undated');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','time-context-undated');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','time-context-undated');
         INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','背景.md','x');
         INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
             ('{chunk}','{kb}','{doc}',0,'# 项目背景\n\n去年，码表一代进入北美市场。');"
    ))
    .execute(&pool)
    .await?;
    let model = Model {
        replies: Arc::new(Mutex::new(vec![
            json!({
                "e": [["码表一代", "产品", 1], ["北美市场", "市场", 1]],
                "s": [["去年，码表一代进入北美市场。", "码表一代", "进入", "北美市场", null, null, "去年", null]],
                "n": [],
                "t": []
            })
            .to_string(),
            json!({ "m": [[0, "point", {
                "kind": "anchored", "anchor": {"kind": "document"},
                "offset": {"count": 1, "unit": "year", "direction": "before"}
            }, "year"]] })
            .to_string(),
        ])),
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

    let run = async {
        crate::extraction::extract_document(&state, doc, utopia_core::models::Proposer::default())
            .await?;
        resolve_document(&state, doc).await?;
        let requests = model.requests.lock().unwrap().clone();
        assert_eq!(
            requests.len(),
            2,
            "one extraction call and one interpretation call; the opening is not read for a date"
        );
        for r in &requests {
            let system = r["messages"][0]["content"].as_str().unwrap_or("");
            assert!(
                !system.starts_with("You read the opening of a document"),
                "the dating prompt is not sent"
            );
        }
        let row: (Option<DateTime<Utc>>, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT valid_from, valid_from_grade, attested_from FROM facts
              WHERE kb_id = $1 AND layer = 'open'",
        )
        .bind(kb)
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            row,
            (None, Some("C".to_string()), None),
            "no anchor: the time word waits, and the statement has no attestation"
        );
        Ok::<(), anyhow::Error>(())
    }
    .await;

    server.abort();
    sqlx::query("DELETE FROM jobs WHERE payload->>'document_id'=$1 OR payload->>'kb_id'=$2")
        .bind(doc.to_string())
        .bind(kb.to_string())
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
