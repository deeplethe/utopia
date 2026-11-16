//! 前面的分块认下的实体，后面的分块认得（#588 cut 1）。
//!
//! 抽取按分块走：每一块的提示词里列着这篇文档前面的块已经认下的实体（`known`），模型
//! 在后面的块里提到它，不必再列一遍，落库时认回同一行。分块并发调模型（cut 2）要保住的
//! 就是这一条，所以先把它钉住：第一块列出 `Acme`，第二块的回复**不列**它、只在陈述里
//! 提到——两条陈述挂在同一个实体上，而且第二块的提示词里有 `- Acme (organization)`。
//! 第二块要是自己再列一遍 `Acme`，按名字去库里也认得回来，那测的就不是 `known` 了。
//!
//! `extract_known_in_prompt` 关掉之后（召回台子量并发的影响用它）提示词里没有那一行，
//! 落库照样认回去。
//!
//! 脚本化的模型端点、真库，与 `phrase_alignment_tests.rs` 同一个做法。
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。
use super::*;
use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

#[derive(Default)]
struct Pause {
    arrived: Notify,
    resume: Notify,
}

#[derive(Clone)]
struct Model {
    replies: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    pause_second: Option<Arc<Pause>>,
}

/// 开放抽取的回复：脚本里的字符串就是 chat 流里的 `delta.content`。
/// 与 `errata_tests.rs::reply` 同一档（text/event-stream + `usage` 收尾），
/// 但 `chat_retrying_rate_limits_at` 解析流时只读 `content` 这一段。
async fn reply(State(m): State<Model>, Json(body): Json<Value>) -> impl IntoResponse {
    let request = {
        let mut requests = m.requests.lock().unwrap();
        requests.push(body);
        requests.len()
    };
    if let Some(pause) = m.pause_second.filter(|_| request == 2) {
        pause.arrived.notify_one();
        pause.resume.notified().await;
    }
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
    async fn new(
        replies: Vec<String>,
        known_in_prompt: bool,
        pause_second: Option<Arc<Pause>>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let ids: Vec<Uuid> = (0..6).map(|_| Uuid::now_v7()).collect();
        let (org, ws, kb, doc, chunk0, chunk1) = (ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]);
        // 插进去的只有本地生成的 UUID
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','open-known-flow');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','open-known-flow');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','open-known-flow');
             INSERT INTO documents(id,kb_id,filename,sha256,status) VALUES ('{doc}','{kb}','acme.txt','x','ready');
             INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES
                 ('{chunk0}','{kb}','{doc}',0,'Acme is based in London.'),
                 ('{chunk1}','{kb}','{doc}',1,'Acme runs the harbour facility.');"
        ))
        .execute(&pool)
        .await?;
        let model = Model {
            replies: Arc::new(Mutex::new(replies)),
            requests: Arc::new(Mutex::new(Vec::new())),
            pause_second,
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
            extract_known_in_prompt: known_in_prompt,
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = AppState::new(pool.clone(), &cfg, search, "test-only".into());
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
        self._server.abort();
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

/// 两块的回复：第一块列出 `Acme`，第二块不列、只在陈述里提到它。
/// 紧凑数组的 8 格是 `[quote, subject, phrase, object, value, qualifiers, when, ended]`
fn replies() -> Vec<String> {
    vec![
        json!({
            "e": [["Acme", "organization", 1]],
            "s": [["Acme is based in London.", "Acme", "based in", null, "London", null, null, null]],
            "n": []
        })
        .to_string(),
        json!({
            "e": [],
            "s": [["Acme runs the harbour facility.", "Acme", "runs", null, "harbour", null, null, null]],
            "n": []
        })
        .to_string(),
    ]
}

/// 跑完抽取：只有一个 `Acme`，两条陈述都挂在它上面。交回第二块的提示词
async fn extract_and_read_the_second_prompt(f: &Fx) -> anyhow::Result<String> {
    crate::extraction::extract_document(&f.state, f.doc, utopia_core::models::Proposer::default())
        .await?;
    let acme: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM entities WHERE kb_id=$1 AND canonical_name='Acme'")
            .bind(f.kb)
            .fetch_all(&f.pool)
            .await?;
    anyhow::ensure!(
        acme.len() == 1,
        "one Acme across both chunks, got {}",
        acme.len()
    );
    let subjects: Vec<Uuid> =
        sqlx::query_scalar("SELECT subject_id FROM facts WHERE kb_id=$1 AND layer='open'")
            .bind(f.kb)
            .fetch_all(&f.pool)
            .await?;
    anyhow::ensure!(
        subjects == vec![acme[0], acme[0]],
        "both statements hang on the one Acme: {subjects:?}"
    );
    let requests = f.model.requests.lock().unwrap();
    anyhow::ensure!(
        requests.len() == 2,
        "one request a chunk, got {}",
        requests.len()
    );
    Ok(requests[1]["messages"][1]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

#[tokio::test]
async fn a_thing_listed_by_an_earlier_chunk_reaches_a_later_one_via_known() -> anyhow::Result<()> {
    let Some(f) = Fx::new(replies(), true, None).await? else {
        return Ok(());
    };
    let run = extract_and_read_the_second_prompt(&f).await;
    f.cleanup().await?;
    let prompt = run?;
    assert!(
        prompt.contains("- Acme (organization)"),
        "the second chunk is told what the first one listed: {prompt}"
    );
    Ok(())
}

/// 旋钮关掉：第二块的提示词里不再列前面认下的实体，落库仍然认回同一个 `Acme`
#[tokio::test]
async fn with_known_left_out_of_the_prompt_a_later_chunk_still_lands_on_the_same_entity(
) -> anyhow::Result<()> {
    let Some(f) = Fx::new(replies(), false, None).await? else {
        return Ok(());
    };
    let run = extract_and_read_the_second_prompt(&f).await;
    f.cleanup().await?;
    let prompt = run?;
    assert!(
        !prompt.contains("- Acme (organization)"),
        "nothing listed as known: {prompt}"
    );
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Interruption {
    RebuildInFlight,
    ReextractInFlight,
    RebuildDuringApply,
    PurgeDuringApply,
}

// facts 上的触发器 DDL 会锁整张表，两条屏障用例必须串行到清理结束。
static FACTS_TRIGGER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 第一块留下缓存的 Acme id；重建会删掉它，单篇重抽则保留实体、重置分块。
/// 写入阶段的屏障放在真正的 INSERT 上，确保重建发生在 epoch 检查之后。
async fn interrupt_the_second_chunk(interruption: Interruption) -> anyhow::Result<()> {
    let during_apply = matches!(
        interruption,
        Interruption::RebuildDuringApply | Interruption::PurgeDuringApply
    );
    let _trigger_guard = if during_apply {
        Some(FACTS_TRIGGER_LOCK.lock().await)
    } else {
        None
    };
    let pause = Arc::new(Pause::default());
    let Some(f) = Fx::new(replies(), true, Some(pause.clone())).await? else {
        return Ok(());
    };
    let rebuild = interruption != Interruption::ReextractInFlight;
    let barrier = format!("pause_extraction_{}", f.doc.simple());
    let mut gate = None;
    let mut gate_pid = 0i32;
    if during_apply {
        let mut tx = f.pool.begin().await?;
        gate_pid = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock(1361, $1)")
            .bind(gate_pid)
            .execute(&mut *tx)
            .await?;
        gate = Some(tx);
    }
    let run = tokio::time::timeout(Duration::from_secs(30), async {
        let extraction = crate::extraction::extract_document(&f.state, f.doc, Proposer::default());
        tokio::pin!(extraction);
        tokio::select! {
            result = &mut extraction => {
                result?;
                anyhow::bail!("extraction finished before the second reply was released");
            }
            _ = pause.arrived.notified() => {}
        }
        if during_apply {
            // 标识符和条件都来自本夹具的 UUID；别的知识库不会等这把锁。
            sqlx::raw_sql(&format!(
                "CREATE FUNCTION {barrier}() RETURNS trigger LANGUAGE plpgsql AS $$
                 BEGIN
                   IF NEW.kb_id = '{}'::uuid THEN
                     PERFORM pg_advisory_xact_lock(1361, {gate_pid});
                   END IF;
                   RETURN NEW;
                 END $$;
                 CREATE TRIGGER {barrier} BEFORE INSERT ON facts
                 FOR EACH ROW EXECUTE FUNCTION {barrier}();",
                f.kb
            ))
            .execute(&f.pool)
            .await?;
            pause.resume.notify_one();
            loop {
                let blocked: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)))",
                )
                .bind(gate_pid)
                .fetch_one(&f.pool)
                .await?;
                if blocked {
                    break;
                }
                tokio::select! {
                    result = &mut extraction => {
                        result?;
                        anyhow::bail!("extraction finished before the INSERT barrier");
                    }
                    _ = tokio::time::sleep(Duration::from_millis(10)) => {}
                }
            }
        }
        if rebuild {
            utopia_store::graph::purge_graph(&f.pool, f.kb).await?;
            if interruption != Interruption::PurgeDuringApply {
                let queued = utopia_store::documents::queue_extraction(&f.pool, f.kb, None).await?;
                anyhow::ensure!(queued == vec![f.doc], "rebuild must queue the document");
            }
        } else {
            utopia_store::documents::queue_extraction_one(&f.pool, f.doc).await?;
        }
        if let Some(tx) = gate.take() {
            tx.rollback().await?;
        }
        pause.resume.notify_one();
        let result = extraction.await;
        if interruption == Interruption::PurgeDuringApply {
            // 同样的外键失败，没有新 epoch 就仍是当前任务的错误，不能一律吞掉。
            let error = result.err().ok_or_else(|| anyhow::anyhow!("current job lost its error"))?;
            anyhow::ensure!(format!("{error:#}").contains("facts_subject_same_kb"), "{error:#}");
            return Ok(());
        }
        result?;

        let doc = utopia_store::documents::get(&f.pool, f.doc).await?;
        anyhow::ensure!(doc.graph_status == "queued", "the new job keeps its status");
        anyhow::ensure!(doc.graph_error.is_none(), "superseded work is not a failure");
        let extracted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM chunks WHERE document_id=$1 AND extracted_at IS NOT NULL",
        )
        .bind(f.doc)
        .fetch_one(&f.pool)
        .await?;
        anyhow::ensure!(extracted == 0, "the old reply must not mark a chunk extracted");
        let statements: i64 =
            sqlx::query_scalar("SELECT count(*) FROM facts WHERE kb_id=$1 AND layer='open'")
                .bind(f.kb)
                .fetch_one(&f.pool)
                .await?;
        anyhow::ensure!(
            statements == if rebuild { 0 } else { 1 },
            "only statements from before re-extract may remain, got {statements}"
        );
        let jobs: Vec<(String, String)> = sqlx::query_as(
            "SELECT kind, status FROM jobs WHERE payload->>'document_id'=$1 OR payload->>'kb_id'=$2",
        )
        .bind(f.doc.to_string())
        .bind(f.kb.to_string())
        .fetch_all(&f.pool)
        .await?;
        anyhow::ensure!(
            jobs == vec![("extract_document".into(), "queued".into())],
            "only the replacement job remains, with no follow-up jobs: {jobs:?}"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await;
    pause.resume.notify_one();
    if let Some(tx) = gate.take() {
        tx.rollback().await?;
    }
    if during_apply {
        sqlx::raw_sql(&format!(
            "DROP TRIGGER IF EXISTS {barrier} ON facts; DROP FUNCTION IF EXISTS {barrier}();"
        ))
        .execute(&f.pool)
        .await?;
    }
    f.cleanup().await?;
    run?
}

#[tokio::test]
async fn a_rebuild_discards_an_extraction_reply_that_is_already_in_flight() -> anyhow::Result<()> {
    interrupt_the_second_chunk(Interruption::RebuildInFlight).await
}

#[tokio::test]
async fn reextract_discards_an_extraction_reply_that_is_already_in_flight() -> anyhow::Result<()> {
    interrupt_the_second_chunk(Interruption::ReextractInFlight).await
}

#[tokio::test]
async fn a_rebuild_after_the_epoch_check_does_not_fail_the_superseded_job() -> anyhow::Result<()> {
    interrupt_the_second_chunk(Interruption::RebuildDuringApply).await
}

#[tokio::test]
async fn an_apply_error_without_a_replacement_epoch_is_still_returned() -> anyhow::Result<()> {
    interrupt_the_second_chunk(Interruption::PurgeDuringApply).await
}
