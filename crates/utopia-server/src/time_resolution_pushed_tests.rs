//! 推送的陈述把 `when` / `ended` 写成完整日期或带区时刻时，`resolve_time` 自己读（#1089，
//! 0054 的悬而未决那条的落点）：不问模型，没有对话模型也照常收工。
//!
//! 与 `time_context_tests.rs` 同一个做法：脚本化的模型端点、真库，
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

/// 陈述的开放行：(宾语名, valid_from, from 精度, valid_to, to 精度, 起点等级)
type FactRow = (
    String,
    Option<DateTime<Utc>>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<String>,
    Option<String>,
);

/// 一条时间提及：(字, 槽, 等级, 形状)
type MentionRow = (String, String, Option<String>, Option<String>);

/// 一个 `statements` 来源、一篇推送来的文档（块就是载荷）、随需一个脚本化的对话端点。
/// `replies` 为 None 时不配模型——这正是这条路的测试点
struct Rig {
    pool: sqlx::PgPool,
    state: AppState,
    org: Uuid,
    kb: Uuid,
    doc: Uuid,
    model: Option<Model>,
    server: Option<tokio::task::JoinHandle<()>>,
    _dir: tempfile::TempDir,
}

impl Rig {
    async fn new(payload: Value, replies: Option<Vec<String>>) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, source, doc, chunk) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // 存的文档是 `push_statements` 自己重排出来的那份：身份、日期（有的话），
        // 再是契约的三个数组，键序固定
        let text = serde_json::to_string_pretty(&payload)?;
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','pushed-time');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','pushed-time');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','pushed-time');
             INSERT INTO sources(id,kb_id,kind,name) VALUES ('{source}','{kb}','statements','robot-1');
             INSERT INTO documents(id,kb_id,filename,sha256,source_id) VALUES
                 ('{doc}','{kb}','obs-1.json','x{doc}','{source}');
             INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
                 ('{chunk}','{kb}','{doc}',0,$${text}$$);"
        ))
        .execute(&pool)
        .await?;
        let (model, server) = match replies {
            Some(replies) => {
                let model = Model {
                    replies: Arc::new(Mutex::new(replies)),
                    requests: Arc::new(Mutex::new(Vec::new())),
                };
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
                let endpoint = format!("http://{}", listener.local_addr()?);
                let router = Router::new()
                    .route("/chat/completions", post(reply))
                    .with_state(model.clone());
                let server =
                    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
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
                (Some(model), Some(server))
            }
            None => (None, None),
        };
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
        // 抽取：推送路径按契约解析、不问模型（0054）；这一步把陈述与时间词写进库
        crate::extraction::extract_document(&state, doc, utopia_core::models::Proposer::default())
            .await?;
        Ok(Some(Self {
            pool,
            state,
            org,
            kb,
            doc,
            model,
            server,
            _dir: dir,
        }))
    }

    /// 陈述的开放行，按宾语名排
    async fn fact_rows(&self) -> anyhow::Result<Vec<FactRow>> {
        Ok(sqlx::query_as(
            "SELECT o.canonical_name, f.valid_from, f.valid_from_precision,
                    f.valid_to, f.valid_to_precision, f.valid_from_grade
               FROM facts f JOIN entities o ON o.id = f.object_id
              WHERE f.kb_id = $1 AND f.layer = 'open' AND f.invalidated_at IS NULL
              ORDER BY o.canonical_name",
        )
        .bind(self.kb)
        .fetch_all(&self.pool)
        .await?)
    }

    /// 时间提及行：(字、槽、等级、解释形状)
    async fn mention_rows(&self) -> anyhow::Result<Vec<MentionRow>> {
        Ok(sqlx::query_as(
            "SELECT text, role, grade, shape FROM time_mentions
              WHERE kb_id = $1 ORDER BY role, text",
        )
        .bind(self.kb)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn requests(&self) -> Vec<Value> {
        match &self.model {
            Some(m) => m.requests.lock().unwrap().clone(),
            None => Vec::new(),
        }
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        if let Some(server) = self.server {
            server.abort();
        }
        sqlx::query("DELETE FROM jobs WHERE payload->>'document_id'=$1 OR payload->>'kb_id'=$2")
            .bind(self.doc.to_string())
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

fn payload(when: &str, ended: Option<&str>) -> Value {
    json!({
        "external_id": "obs-1",
        "doc_time": "2026-10-01T00:00:00Z",
        "e": [["cup-7", "cup", true], ["kitchen table", "table", true]],
        "s": [[null, "cup-7", "is on", "kitchen table", null, null, when, ended]],
        "n": []
    })
}

fn payload_two(when_a: &str, when_b: &str) -> Value {
    json!({
        "external_id": "obs-1",
        "doc_time": "2026-10-01T00:00:00Z",
        "e": [["cup-7", "cup", true], ["kitchen table", "table", true], ["counter", "table", true]],
        "s": [
            [null, "cup-7", "is on", "kitchen table", null, null, when_a, null],
            [null, "cup-7", "is on", "counter", null, null, when_b, null]
        ],
        "n": []
    })
}

fn day(y: i32, m: u32, d: u32) -> Option<DateTime<Utc>> {
    Some(Utc.with_ymd_and_hms(y, m, d, 0, 0, 0).unwrap())
}

fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<DateTime<Utc>> {
    Some(Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap())
}

/// 没有对话模型：载荷写明的日期由代码读成 A 级、字写到的精度，任务照常完成；
/// 字本身（`"2026-09-23"`）留着当提及与证据
#[tokio::test]
async fn a_pushed_date_is_resolved_in_code_without_a_model() -> anyhow::Result<()> {
    let Some(rig) = Rig::new(payload("2026-09-23", None), None).await? else {
        return Ok(());
    };
    resolve_document(&rig.state, rig.doc).await?;
    assert_eq!(
        rig.fact_rows().await?,
        vec![(
            "kitchen table".to_string(),
            day(2026, 9, 23),
            Some("day".to_string()),
            day(2026, 9, 23),
            Some("day".to_string()),
            Some("A".to_string()),
        )],
        "a bare date is a single date at day precision, grade A"
    );
    assert_eq!(
        rig.mention_rows().await?,
        vec![(
            "2026-09-23".to_string(),
            "when".to_string(),
            Some("A".to_string()),
            Some("point".to_string())
        )],
        "the literal string stays as the mention"
    );
    rig.cleanup().await
}

/// 带 `Z` 与带偏移的时刻：精度到秒，值是换算成 UTC 的同一刻——偏移不是贴标签，
/// 是真的进了值
#[tokio::test]
async fn a_pushed_timestamp_resolves_at_second_precision_and_its_zone_counts() -> anyhow::Result<()>
{
    let Some(rig) = Rig::new(payload("2026-09-23T08:14:03Z", None), None).await? else {
        return Ok(());
    };
    resolve_document(&rig.state, rig.doc).await?;
    assert_eq!(
        rig.fact_rows().await?,
        vec![(
            "kitchen table".to_string(),
            at(2026, 9, 23, 8, 14, 3),
            Some("second".to_string()),
            at(2026, 9, 23, 8, 14, 3),
            Some("second".to_string()),
            Some("A".to_string()),
        )]
    );
    rig.cleanup().await?;

    let Some(rig) = Rig::new(payload("2026-09-23T08:14:03+08:00", None), None).await? else {
        return Ok(());
    };
    resolve_document(&rig.state, rig.doc).await?;
    assert_eq!(
        rig.fact_rows().await?,
        vec![(
            "kitchen table".to_string(),
            // +08:00 的 08:14:03 是 UTC 的 00:14:03
            at(2026, 9, 23, 0, 14, 3),
            Some("second".to_string()),
            at(2026, 9, 23, 0, 14, 3),
            Some("second".to_string()),
            Some("A".to_string()),
        )]
    );
    rig.cleanup().await
}

/// 字写到哪一级，精度就到哪一级：`T08` 是小时、`T08:14` 是分钟——不替它补
/// 从没写过的秒（0024）
#[tokio::test]
async fn the_precision_is_the_one_the_payload_writes() -> anyhow::Result<()> {
    for (when, expected, precision) in [
        ("2026-09-23", (0u32, 0u32, 0u32), "day"),
        ("2026-09-23T08Z", (8, 0, 0), "hour"),
        ("2026-09-23T08:14Z", (8, 14, 0), "minute"),
        ("2026-09-23T08:14:03Z", (8, 14, 3), "second"),
    ] {
        let Some(rig) = Rig::new(payload(when, None), None).await? else {
            return Ok(());
        };
        resolve_document(&rig.state, rig.doc).await?;
        let (h, m, s) = expected;
        assert_eq!(
            rig.fact_rows().await?,
            vec![(
                "kitchen table".to_string(),
                at(2026, 9, 23, h, m, s),
                Some(precision.to_string()),
                at(2026, 9, 23, h, m, s),
                Some(precision.to_string()),
                Some("A".to_string()),
            )],
            "{when} resolves at {precision}"
        );
        rig.cleanup().await?;
    }
    Ok(())
}

/// `when` 加 `ended` 是一个区间：起是 when、止是 ended，各带自己的精度
#[tokio::test]
async fn when_plus_ended_is_an_interval() -> anyhow::Result<()> {
    let Some(rig) = Rig::new(payload("2026-09-20", Some("2026-09-23T08:14:03Z")), None).await?
    else {
        return Ok(());
    };
    resolve_document(&rig.state, rig.doc).await?;
    assert_eq!(
        rig.fact_rows().await?,
        vec![(
            "kitchen table".to_string(),
            day(2026, 9, 20),
            Some("day".to_string()),
            at(2026, 9, 23, 8, 14, 3),
            Some("second".to_string()),
            Some("A".to_string()),
        )]
    );
    rig.cleanup().await
}

/// 光杆的年与年月不在这一刀里：没有模型时留着 C，任务照常收工；
/// 自然语言的时间词同样留着 C
#[tokio::test]
async fn a_bare_year_or_year_month_or_words_are_left_for_a_model() -> anyhow::Result<()> {
    for when in ["2026", "2026-09", "last Tuesday"] {
        let Some(rig) = Rig::new(payload(when, None), None).await? else {
            return Ok(());
        };
        resolve_document(&rig.state, rig.doc).await?;
        assert_eq!(
            rig.mention_rows().await?,
            vec![(
                when.to_string(),
                "when".to_string(),
                Some("C".to_string()),
                None
            )],
            "{when:?} is not code-resolved"
        );
        assert_eq!(
            rig.fact_rows().await?,
            vec![(
                "kitchen table".to_string(),
                None,
                None,
                None,
                None,
                Some("C".to_string()),
            )],
            "{when:?}: the statement keeps its time words, unresolved, and the job ends"
        );
        rig.cleanup().await?;
    }
    Ok(())
}

/// 一份载荷两种字：读得动的解成 A，读不动的留 C——一篇文档里两种结局可以并存，
/// 任务照常收工
#[tokio::test]
async fn a_mixed_push_resolves_what_code_can_and_leaves_the_rest() -> anyhow::Result<()> {
    let Some(rig) = Rig::new(payload_two("2026-09-23", "last Tuesday"), None).await? else {
        return Ok(());
    };
    resolve_document(&rig.state, rig.doc).await?;
    assert_eq!(
        rig.mention_rows().await?,
        vec![
            (
                "2026-09-23".to_string(),
                "when".to_string(),
                Some("A".to_string()),
                Some("point".to_string())
            ),
            (
                "last Tuesday".to_string(),
                "when".to_string(),
                Some("C".to_string()),
                None
            ),
        ]
    );
    rig.cleanup().await
}

/// 文档里的字长得像 RFC 3339 也不走推送那条路：判据是来源种类，不是字的形状。
/// 配了模型时这条提及照样送出去问——来源不是 `statements`，代码不代读
#[tokio::test]
async fn a_document_mention_that_looks_machine_readable_stays_on_the_model_path(
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb, doc, chunk) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let text = "The cup was on the kitchen table at 2026-09-23T08:14:03Z.";
    sqlx::raw_sql(&format!(
        "INSERT INTO organizations(id,name) VALUES ('{org}','document-path');
         INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','document-path');
         INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','document-path');
         INSERT INTO documents(id,kb_id,filename,sha256) VALUES ('{doc}','{kb}','note.md','x{doc}');
         INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
             ('{chunk}','{kb}','{doc}',0,$${text}$$);"
    ))
    .execute(&pool)
    .await?;
    let absolute = json!({
        "kind": "absolute",
        // 契约里日期部件用紧凑键（y/m/d/h/min/s）
        "from": {"y": 2026, "m": 9, "d": 23, "h": 8, "min": 14, "s": 3}
    });
    let model = Model {
        replies: Arc::new(Mutex::new(vec![
            json!({
                "e": [["the cup", "cup", 1], ["the kitchen table", "table", 1]],
                "s": [["The cup was on the kitchen table at 2026-09-23T08:14:03Z.",
                       "the cup", "was on", "the kitchen table", null, null,
                       "2026-09-23T08:14:03Z", null]],
                "n": [],
                "t": []
            })
            .to_string(),
            // 解释：模型照答——走的还是模型这条路，代码没代读
            json!({ "m": [[0, "point", absolute, "second"]] }).to_string(),
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
    crate::extraction::extract_document(&state, doc, utopia_core::models::Proposer::default())
        .await?;
    resolve_document(&state, doc).await?;
    // 一次抽取 + 一次解释：长得像日期的字照样问过模型；代码没有替它读
    assert_eq!(
        model.requests.lock().unwrap().len(),
        2,
        "the mention went to the model: source identity, not string shape"
    );
    // 陈述行：宾语是「the kitchen table」的那条（名字事实等别的行不算）
    let row: (Option<DateTime<Utc>>, Option<String>) = sqlx::query_as(
        "SELECT f.valid_from, f.valid_from_grade FROM facts f
           JOIN entities o ON o.id = f.object_id
          WHERE f.kb_id = $1 AND f.layer = 'open' AND o.canonical_name = 'the kitchen table'",
    )
    .bind(kb)
    .fetch_one(&pool)
    .await?;
    assert_eq!(row, (at(2026, 9, 23, 8, 14, 3), Some("A".to_string())));
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
    Ok(())
}

/// 配了模型也一样：代码读得动的提及不送模型（回复里那条 `anchored` 是模型对
/// 「上周」的答法——它锚在另一条提及上，而那一条正是代码读的）。再跑一次
/// `resolve_time` 答案不变：时间戳照样不问，「上周」重问一回得的也是同一个位置——
/// 代码读的没有模型可盖
#[tokio::test]
async fn a_code_resolved_mention_is_never_asked_and_survives_reruns() -> anyhow::Result<()> {
    // 两个时间词：代码能读的时间戳是第 0 条distinct；「上周」是第 1 条，只有它去问模型。
    // 模型把它锚在第 0 条上——代码读出的解释与模型给的解释在同一张锚点表里
    let last_week = json!({ "m": [[1, "point", {
        "kind": "anchored", "anchor": {"kind": "mention", "id": 0},
        "offset": {"count": 1, "unit": "week", "direction": "before"}
    }, "week"]] })
    .to_string();
    let Some(rig) = Rig::new(
        payload_two("2026-09-23T08:14:03Z", "上周"),
        // 重跑还会再替「上周」问一次：给它同一个答案
        Some(vec![last_week.clone(), last_week]),
    )
    .await?
    else {
        return Ok(());
    };
    resolve_document(&rig.state, rig.doc).await?;
    // 抽取不问模型（推送路），这里只有一次解释调用：只问「上周」。
    // 提示词的「提及」一栏里没有那条时间戳——它会作为载荷正文（sentence）出现在
    // 「上周」的语境里，那是文档自己的字；关键是它没有被当成一条提及去问
    let requests = rig.requests().await;
    assert_eq!(requests.len(), 1, "only the interpretation call");
    let asked = requests[0].to_string();
    // 「提及」一栏只有「上周」这一条被问（- id 只出现一次），时间戳不在其中——
    // 它会作为载荷正文出现在「上周」的语境里，那是文档自己的字
    assert!(asked.contains("上周"), "asked: {asked}");
    assert_eq!(asked.matches("- id ").count(), 1, "asked: {asked}");

    let first = rig.fact_rows().await?;
    assert_eq!(
        first,
        vec![
            // 「上周」锚在 9-23T08:14:03Z 上前一周：2026-09-16，日精度，B 级
            (
                "counter".to_string(),
                day(2026, 9, 16),
                Some("day".to_string()),
                day(2026, 9, 16),
                Some("day".to_string()),
                Some("B".to_string()),
            ),
            (
                "kitchen table".to_string(),
                at(2026, 9, 23, 8, 14, 3),
                Some("second".to_string()),
                at(2026, 9, 23, 8, 14, 3),
                Some("second".to_string()),
                Some("A".to_string()),
            ),
        ],
        "code-resolved, precision as written"
    );
    // 重跑：同样的答案；脚本端点有第二份回复等着「上周」
    resolve_document(&rig.state, rig.doc).await?;
    let requests = rig.requests().await;
    assert_eq!(
        requests.len(),
        2,
        "the rerun asked the model for 上周 again"
    );
    assert_eq!(
        requests[1].to_string().matches("- id ").count(),
        1,
        "the rerun also asked only about 上周"
    );
    assert_eq!(
        rig.fact_rows().await?,
        first,
        "a rerun gives the same answer"
    );
    rig.cleanup().await
}
