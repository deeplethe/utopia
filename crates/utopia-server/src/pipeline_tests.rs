//! #513：摄入的嵌入几批并发在飞，配对一条不乱。
//!
//! 并发不许碰的是「哪条向量落在哪个分块上」：错位落库之后看不出来，正文还在，向量是别人的。
//! 所以这里的测试问的是配对与完整，不是快慢。嵌入端点用 wiremock 假扮，每条正文的向量
//! 由正文本身算出来，读回来就能验；顺带记下每个请求的到达时刻和批大小。
//!
//! 1. **每个分块拿到自己正文的向量。** 40 条、三批、四路并发，批次乱序完成，读回来逐条对。
//!    附带：末尾不足一批的余数照样送，空文档一个请求都不发。
//! 2. **数量对不上整批放弃**：少回一条，那一批一条都不写。
//! 3. **上限守得住**：十二批、每批延时 150ms，同一时刻在飞的从不超过 EMBED_JOBS，
//!    总耗时明显短于串行。
//! 4. **一批失败文档不悬着**：走完整的 process_document，第二批回 500，文档落在 failed
//!    并带原因，不是停在 embedding。
//! 5. **就绪之前全部嵌完**：process_document 走通后没有一条向量为空。
//! 6. **记忆摄入走同一条路**：memory_ingest 嵌完自己的分块。
//! 7. **正文夹 NUL 不毁整篇**（#611）：Postgres 的 TEXT 不收 0x00，从前一个字节就让整篇
//!    落在 failed。现在走完整的 process_document 到 ready，库里没有一个分块带 NUL。
//!
//! 8. **扫描件等版面识别服务读**（0040 第二刀）：配上服务重新排队，交一次、挂回去问、按页切块。
//! 9. **录音等会标说话人的转写模型读**（第三刀）：说话人进正文、时刻进锚点；分不出说话人的降级。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use utopia_core::models::Proposer;
use uuid::Uuid;
use wiremock::{
    matchers::method, matchers::path, Mock, MockServer, Request, Respond, ResponseTemplate,
};

/// 正文 → 向量：长度、首字符、字节和取模、常数 1。四维就够分辨每一条
fn vector_of(text: &str) -> Vec<f32> {
    vec![
        text.chars().count() as f32,
        text.chars().next().map(|c| c as u32 as f32).unwrap_or(0.0),
        (text.bytes().map(u32::from).sum::<u32>() % 97) as f32,
        1.0,
    ]
}

/// 假嵌入端点。`short_by` 每批少回几条；`fail_request` 第几个请求回 500；`delay` 每个响应压多久。
/// 可克隆：一份挂进 wiremock，一份留在夹具里读到达记录
#[derive(Clone)]
struct FakeEmbed {
    arrivals: Arc<Mutex<Vec<(Instant, usize)>>>,
    delay: Duration,
    short_by: usize,
    fail_request: Option<usize>,
}

impl FakeEmbed {
    fn new(delay: Duration) -> Self {
        Self {
            arrivals: Arc::new(Mutex::new(Vec::new())),
            delay,
            short_by: 0,
            fail_request: None,
        }
    }
    fn arrivals(&self) -> Vec<(Instant, usize)> {
        self.arrivals.lock().expect("arrivals lock").clone()
    }
}

impl Respond for FakeEmbed {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = request.body_json().expect("embedding request is JSON");
        let inputs = body["input"].as_array().cloned().unwrap_or_default();
        let ordinal = {
            let mut a = self.arrivals.lock().expect("arrivals lock");
            a.push((Instant::now(), inputs.len()));
            a.len()
        };
        if self.fail_request == Some(ordinal) {
            return ResponseTemplate::new(500).set_body_string("model is down");
        }
        let data: Vec<serde_json::Value> = inputs
            .iter()
            .take(inputs.len().saturating_sub(self.short_by))
            .map(|t| serde_json::json!({ "embedding": vector_of(t.as_str().unwrap_or("")) }))
            .collect();
        ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({ "data": data }))
            .set_delay(self.delay)
    }
}

struct Fx {
    pool: sqlx::PgPool,
    state: crate::state::AppState,
    server: MockServer,
    fake: FakeEmbed,
    org: Uuid,
    ws: Uuid,
    kb: Uuid,
    dir: std::path::PathBuf,
}

async fn fixture(fake: FakeEmbed) -> anyhow::Result<Option<Fx>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'pipeline-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'pipeline-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'pipeline-test')")
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(fake.clone())
        .mount(&server)
        .await;
    utopia_store::settings::upsert(
        &pool,
        ws,
        None,
        None,
        None,
        Some(&server.uri()),
        None,
        Some("fake-embed"),
        None,
    )
    .await?;
    let dir = std::env::temp_dir().join(format!("utopia-pipeline-{kb}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    Ok(Some(Fx {
        pool,
        state,
        server,
        fake,
        org,
        ws,
        kb,
        dir,
    }))
}

impl Fx {
    /// 一篇只有分块、没有向量的文档；正文各不相同，配对错了就对不上
    async fn document_with_chunks(&self, n: usize) -> anyhow::Result<Uuid> {
        let doc = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO documents(id,kb_id,filename,sha256,status) VALUES($1,$2,'m.md',$3,'ready')",
        )
        .bind(doc)
        .bind(self.kb)
        .bind(format!("sha-{doc}"))
        .execute(&self.pool)
        .await?;
        for seq in 0..n {
            sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text) VALUES($1,$2,$3,$4,$5)")
                .bind(Uuid::now_v7())
                .bind(self.kb)
                .bind(doc)
                .bind(seq as i32)
                .bind(format!("chunk {seq} of {doc}: {}", "x".repeat(seq % 7)))
                .execute(&self.pool)
                .await?;
        }
        Ok(doc)
    }

    /// 一篇真文档：正文进 blob 存储，行是 pending，等 process_document 来解析分块
    async fn document_to_process(&self, paragraphs: usize) -> anyhow::Result<Uuid> {
        let text: String = (0..paragraphs)
            .map(|i| format!("Paragraph {i}. {}\n\n", format!("word{i} ").repeat(140)))
            .collect();
        self.document_with_text(&text).await
    }

    /// 同上，正文由调用方给
    async fn document_with_text(&self, text: &str) -> anyhow::Result<Uuid> {
        self.document_with_bytes("long.md", text.as_bytes()).await
    }

    /// 任意字节、任意文件名：图片、录音、二进制都从这里进
    async fn document_with_bytes(&self, filename: &str, bytes: &[u8]) -> anyhow::Result<Uuid> {
        use sha2::{Digest, Sha256};
        let sha: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.state.blob.put(&sha, bytes).await?;
        Ok(utopia_store::documents::create(
            &self.pool,
            self.kb,
            filename,
            "application/octet-stream",
            bytes.len() as i64,
            &sha,
            None,
            None,
            None,
        )
        .await?
        .id)
    }

    async fn alerts(&self, kind: &str) -> anyhow::Result<Vec<serde_json::Value>> {
        Ok(sqlx::query_scalar(
            "SELECT detail FROM alerts WHERE kb_id = $1 AND kind = $2 ORDER BY created_at",
        )
        .bind(self.kb)
        .bind(kind)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn embed(&self, doc: Uuid) -> anyhow::Result<usize> {
        let settings = utopia_store::settings::get(&self.pool, self.ws)
            .await?
            .expect("settings were written by the fixture");
        let client = crate::llm_util::embed_client(&settings).expect("embed model is configured");
        super::embed_pending(&self.state, &settings, &client, doc).await
    }

    /// (正文, 向量) 按 seq；向量为空就是没嵌
    async fn stored(&self, doc: Uuid) -> anyhow::Result<Vec<(String, Option<Vec<f32>>)>> {
        Ok(sqlx::query_as(
            "SELECT text, embedding::real[] FROM chunks WHERE document_id = $1 ORDER BY seq",
        )
        .bind(doc)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        let _ = std::fs::remove_dir_all(&self.dir);
        drop(self.server);
        Ok(())
    }
}

#[tokio::test]
async fn every_chunk_gets_the_vector_of_its_own_text() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(30))).await? else {
        return Ok(());
    };
    let doc = f.document_with_chunks(40).await?;
    assert_eq!(f.embed(doc).await?, 40);
    for (text, vector) in f.stored(doc).await? {
        assert_eq!(
            vector.as_deref(),
            Some(vector_of(&text).as_slice()),
            "chunk {text:?} must carry its own vector"
        );
    }
    let mut sizes: Vec<usize> = f.fake.arrivals().into_iter().map(|(_, n)| n).collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![8, 16, 16], "two full batches and the remainder");

    let empty = f.document_with_chunks(0).await?;
    assert_eq!(f.embed(empty).await?, 0);
    assert_eq!(
        f.fake.arrivals().len(),
        3,
        "an empty document makes no request"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_batch_that_answers_with_the_wrong_count_is_abandoned_whole() -> anyhow::Result<()> {
    let mut fake = FakeEmbed::new(Duration::ZERO);
    fake.short_by = 1;
    let Some(f) = fixture(fake).await? else {
        return Ok(());
    };
    let doc = f.document_with_chunks(10).await?;
    assert!(
        f.embed(doc).await.is_err(),
        "15 vectors for 16 texts is an error"
    );
    assert!(
        f.stored(doc).await?.iter().all(|(_, v)| v.is_none()),
        "nothing from a misaligned batch is written"
    );
    f.cleanup().await
}

#[tokio::test]
async fn the_embedding_gate_is_never_held_beyond_its_ceiling() -> anyhow::Result<()> {
    let delay = Duration::from_millis(150);
    let Some(f) = fixture(FakeEmbed::new(delay)).await? else {
        return Ok(());
    };
    let batches = 12;
    let doc = f.document_with_chunks(batches * super::EMBED_BATCH).await?;
    let started = Instant::now();
    assert_eq!(f.embed(doc).await?, batches * super::EMBED_BATCH);
    let elapsed = started.elapsed();

    // 在飞数 = 到达时刻落在同一个响应延时窗口里的请求数（窗口略短于延时，吃掉抖动）
    let arrivals = f.fake.arrivals();
    let window = delay - Duration::from_millis(20);
    let peak = arrivals
        .iter()
        .map(|(t, _)| {
            arrivals
                .iter()
                .filter(|(u, _)| *u <= *t && t.duration_since(*u) < window)
                .count()
        })
        .max()
        .unwrap_or(0);
    assert!(
        peak <= super::EMBED_JOBS,
        "peak in-flight batches {peak} exceeds the ceiling {}",
        super::EMBED_JOBS
    );
    assert!(peak >= 2, "batches actually overlap; peak was {peak}");
    let serial = delay * batches as u32;
    assert!(
        elapsed < serial / 2,
        "twelve batches took {elapsed:?}; serial would be {serial:?}"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_failed_batch_does_not_strand_the_document() -> anyhow::Result<()> {
    let mut fake = FakeEmbed::new(Duration::from_millis(20));
    fake.fail_request = Some(2);
    let Some(f) = fixture(fake).await? else {
        return Ok(());
    };
    let doc = f.document_to_process(20).await?;
    assert!(super::process_document(&f.state, doc).await.is_err());
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "failed", "not left sitting in embedding");
    assert!(
        row.error.as_deref().is_some_and(|e| !e.is_empty()),
        "the reason is on the document"
    );
    f.cleanup().await
}

/// #611：一个 NUL 从前让整篇文档失败——坏的只是几个字节，丢的是整篇
#[tokio::test]
async fn a_nul_byte_does_not_fail_the_whole_document() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    // PDF 文本层里夹带 NUL 的样子：落在词中间、段落之间、一连好几个
    let text = "Revenue grew\0 twelve percent.\n\n\0\0Margins held at thirty\0-one.\n";
    let doc = f.document_with_text(text).await?;
    super::process_document(&f.state, doc).await?;

    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "ready", "a NUL byte must not fail the document");
    let stored = f.stored(doc).await?;
    assert!(!stored.is_empty(), "the document was chunked");
    assert!(
        stored.iter().all(|(t, _)| !t.contains('\0')),
        "no stored chunk carries a NUL"
    );
    let joined: String = stored.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        joined.contains("Revenue grew twelve percent."),
        "the words around the NUL survive, joined as written"
    );
    f.cleanup().await
}

/// 0040：图片、扫描件、录音的字要靠模型读。没配那种模型时**降级**：文件留着、文档停在
/// failed 并记下缺哪一种、报一条库级告警；不重试，也不再解出一堆乱码去分块、嵌入、抽取
#[tokio::test]
async fn a_file_that_needs_a_reader_waits_and_says_so() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let png = [
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H',
    ];
    let mp3 = [b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 0, 0xFF, 0xFB];
    let image = f.document_with_bytes("contract-scan.png", &png).await?;
    let recording = f.document_with_bytes("board-meeting.mp3", &mp3).await?;

    for (doc, reader) in [(image, "ocr"), (recording, "transcribe")] {
        let err = super::process_document(&f.state, doc)
            .await
            .expect_err("nothing can be read without the model");
        assert!(
            utopia_core::is_terminal(&err),
            "retrying cannot configure a model"
        );
        let row = utopia_store::documents::get(&f.pool, doc).await?;
        assert_eq!(row.status, "failed");
        assert_eq!(row.reader_needed.as_deref(), Some(reader));
        assert!(row.error.is_some(), "the document says why");
        assert!(f.stored(doc).await?.is_empty(), "no garbage chunks");
    }
    let alerts = f.alerts("document.needs_reader").await?;
    assert_eq!(alerts.len(), 2);
    assert_eq!(alerts[0]["name"], "contract-scan.png");
    assert_eq!(alerts[0]["reader"], "ocr");
    assert_eq!(alerts[1]["reader"], "transcribe");
    f.cleanup().await
}

/// 假的 `mineru-api`：交一次拿到 `t-n`；问状态前 `processing_polls` 次说还在读，之后说读完；
/// `fail` 时说读失败。记下交了几次、每次带了什么
#[derive(Clone, Default)]
struct FakeMineru {
    submissions: Arc<Mutex<Vec<String>>>,
    polls: Arc<Mutex<usize>>,
    processing_polls: usize,
    fail: bool,
}

impl Respond for FakeMineru {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if request.method.as_str() == "POST" {
            let mut subs = self.submissions.lock().expect("lock");
            let auth = request
                .headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            subs.push(format!(
                "{auth}\n{}",
                String::from_utf8_lossy(&request.body)
            ));
            return ResponseTemplate::new(202).set_body_json(
                serde_json::json!({ "task_id": format!("t-{}", subs.len()), "status": "pending" }),
            );
        }
        if request.url.path().ends_with("/result") {
            let list = serde_json::json!([
                { "type": "header", "text": "ACME", "page_idx": 0, "bbox": [0, 0, 1000, 20] },
                { "type": "text", "text": "Lease Agreement", "text_level": 1, "page_idx": 0, "bbox": [100, 40, 900, 80] },
                { "type": "text", "text": "Beta Robotics pays Alpha 1,000 per month.", "page_idx": 0, "bbox": [100, 100, 900, 140] },
                { "type": "text", "text": "Signed on 12 February 2025.", "page_idx": 1, "bbox": [100, 60, 700, 90] }
            ]);
            return ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "completed", "backend": "vlm-auto-engine", "version": "2.5.4",
                "results": { "contract-scan": { "content_list": list.to_string() } }
            }));
        }
        let mut polls = self.polls.lock().expect("lock");
        *polls += 1;
        let status = if self.fail {
            serde_json::json!({ "status": "failed", "error": "CUDA out of memory" })
        } else if *polls <= self.processing_polls {
            serde_json::json!({ "status": "processing" })
        } else {
            serde_json::json!({ "status": "completed" })
        };
        ResponseTemplate::new(200).set_body_json(status)
    }
}

async fn with_mineru(f: &Fx, fake: &FakeMineru) -> anyhow::Result<()> {
    Mock::given(wiremock::matchers::path_regex("^/ocr/tasks.*"))
        .respond_with(fake.clone())
        .mount(&f.server)
        .await;
    utopia_store::settings::upsert_ocr(
        &f.pool,
        f.ws,
        Some(&format!("{}/ocr/", f.server.uri())),
        Some("ocr-secret"),
        Some("vlm-auto-engine"),
    )
    .await?;
    Ok(())
}

async fn reader_task(f: &Fx, doc: Uuid) -> anyhow::Result<Option<serde_json::Value>> {
    Ok(utopia_store::documents::reader_task(&f.pool, doc).await?)
}

/// 0040 第二刀：没配服务时停下的扫描件，配上服务就重新排队；交一次、问到读完、按页切块，
/// 每块记着读它的服务版本、页码和框。等的时候不烧重试预算，也不重交
#[tokio::test]
async fn a_scan_waits_for_the_layout_service_and_keeps_its_page() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let png = [
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H',
    ];
    let doc = f.document_with_bytes("contract-scan.png", &png).await?;
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("no OCR yet");
    assert!(utopia_core::is_terminal(&err));

    let fake = FakeMineru {
        processing_polls: 1,
        ..Default::default()
    };
    with_mineru(&f, &fake).await?;
    let requeued =
        utopia_store::documents::requeue_waiting_for_reader(&f.pool, f.ws, "ocr").await?;
    assert_eq!(
        requeued,
        vec![(doc, f.kb)],
        "saving the service queues the scan again"
    );
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!((row.status.as_str(), row.reader_needed), ("pending", None));
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = 'process_document' AND status = 'queued'
            AND payload->>'document_id' = $1",
    )
    .bind(doc.to_string())
    .fetch_one(&f.pool)
    .await?;
    assert!(queued >= 1);

    // 交上去：挂回队列等，文档还在 parsing
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("submitted");
    assert!(utopia_core::is_deferred(&err).is_some(), "{err:#}");
    assert!(!utopia_core::is_terminal(&err));
    assert_eq!(
        utopia_store::documents::get(&f.pool, doc).await?.status,
        "parsing"
    );
    let task = reader_task(&f, doc).await?.expect("the task is remembered");
    assert_eq!(task["task_id"], "t-1");

    // 问一次：还在读
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("processing");
    assert!(utopia_core::is_deferred(&err).is_some());

    // 再问：读完了
    super::process_document(&f.state, doc).await?;
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "ready", "{:?}", row.error);
    assert_eq!(
        reader_task(&f, doc).await?,
        None,
        "a finished read forgets its task"
    );
    let subs = fake.submissions.lock().expect("lock").clone();
    assert_eq!(subs.len(), 1, "waiting never submits twice");
    assert!(subs[0].starts_with("Bearer ocr-secret\n"));
    assert!(subs[0].contains("name=\"backend\""));
    assert!(subs[0].contains("vlm-auto-engine"));

    type Stored = (String, String, Option<String>, Option<serde_json::Value>);
    let chunks: Vec<Stored> = sqlx::query_as(
        "SELECT text, origin, origin_model, anchor FROM chunks
          WHERE document_id = $1 AND superseded_at IS NULL ORDER BY seq",
    )
    .bind(doc)
    .fetch_all(&f.pool)
    .await?;
    assert_eq!(chunks.len(), 2, "one chunk per page: {chunks:#?}");
    assert!(chunks.iter().all(|c| c.1 == "ocr"));
    assert_eq!(chunks[0].2.as_deref(), Some("mineru 2.5.4 vlm-auto-engine"));
    assert!(chunks[0].0.contains("1,000 per month"));
    assert!(!chunks[0].0.contains("ACME"), "the page header is not text");
    assert_eq!(
        chunks[0].3,
        Some(serde_json::json!({ "page": 1, "bbox": [100.0, 40.0, 900.0, 140.0] }))
    );
    assert_eq!(
        chunks[1].3.as_ref().map(|a| a["page"].clone()),
        Some(serde_json::json!(2))
    );
    assert!(
        f.stored(doc).await?.iter().all(|(_, v)| v.is_some()),
        "embedded like any text"
    );
    f.cleanup().await
}

/// 服务说读失败：文档带着服务给的原因落在 failed，任务号清掉，这次失败走普通重试（下一次重交）
#[tokio::test]
async fn a_failed_read_is_retried_from_a_fresh_submission() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let fake = FakeMineru {
        fail: true,
        ..Default::default()
    };
    with_mineru(&f, &fake).await?;
    let doc = f
        .document_with_bytes(
            "contract-scan.png",
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .await?;
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("submitted");
    assert!(utopia_core::is_deferred(&err).is_some());
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("failed");
    assert!(utopia_core::is_deferred(&err).is_none());
    assert!(
        !utopia_core::is_terminal(&err),
        "the next attempt may succeed"
    );
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "failed");
    assert!(row
        .error
        .as_deref()
        .is_some_and(|e| e.contains("CUDA out of memory")));
    assert_eq!(reader_task(&f, doc).await?, None);
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("submitted again");
    assert!(utopia_core::is_deferred(&err).is_some());
    assert_eq!(fake.submissions.lock().expect("lock").len(), 2);
    f.cleanup().await
}

/// 假的转写端点：`labels` 时每句带说话人，否则只有时间。记下收到的表单
#[derive(Clone, Default)]
struct FakeTranscriber {
    labels: bool,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Respond for FakeTranscriber {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let auth = request
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        self.requests.lock().expect("lock").push(format!(
            "{auth}\n{}",
            String::from_utf8_lossy(&request.body)
        ));
        let seg = |speaker: &str, start: f64, end: f64, text: &str| {
            if self.labels {
                serde_json::json!({ "type": "transcript.text.segment", "speaker": speaker, "start": start, "end": end, "text": text })
            } else {
                serde_json::json!({ "start": start, "end": end, "text": text })
            }
        };
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "…",
            "segments": [
                seg("A", 0.0, 3.2, "Who delivers the Beta Robotics prototype?"),
                seg("B", 3.5, 7.25, "I will deliver it in Q3."),
            ]
        }))
    }
}

async fn with_transcriber(f: &Fx, fake: &FakeTranscriber, model: &str) -> anyhow::Result<()> {
    f.server.reset().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(f.fake.clone())
        .mount(&f.server)
        .await;
    Mock::given(method("POST"))
        .and(path("/asr/audio/transcriptions"))
        .respond_with(fake.clone())
        .mount(&f.server)
        .await;
    utopia_store::settings::upsert_transcribe(
        &f.pool,
        f.ws,
        Some(&format!("{}/asr", f.server.uri())),
        Some("asr-secret"),
        Some(model),
    )
    .await?;
    Ok(())
}

const MP3: [u8; 12] = [b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 0, 0xFF, 0xFB];

async fn pause_transcription(
    f: &Fx,
    doc: Uuid,
) -> anyhow::Result<(
    Arc<tokio::sync::Notify>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
    tokio::task::JoinHandle<()>,
)> {
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let app = axum::Router::new().route(
        "/audio/transcriptions",
        axum::routing::post({
            let entered = entered.clone();
            let resume = resume.clone();
            move || {
                let entered = entered.clone();
                let resume = resume.clone();
                async move {
                    entered.notify_one();
                    resume.notified().await;
                    axum::Json(serde_json::json!({"segments": [{
                        "speaker": "A", "start": 0.0, "end": 1.0,
                        "text": "The OLD budget is 100."
                    }]}))
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    utopia_store::settings::upsert_transcribe(
        &f.pool,
        f.ws,
        Some(&base),
        None,
        Some("gpt-4o-transcribe-diarize"),
    )
    .await?;
    let state = f.state.clone();
    let processing = tokio::spawn(async move { super::process_document(&state, doc).await });
    tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
    Ok((resume, processing, server))
}

#[tokio::test]
async fn a_late_reader_preserves_a_newer_processed_revision() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let doc = f.document_with_bytes("budget.mp3", &MP3).await?;
    let source = Uuid::now_v7();
    sqlx::query("INSERT INTO sources (id, kb_id, kind, name) VALUES ($1,$2,'api','audit')")
        .bind(source)
        .bind(f.kb)
        .execute(&f.pool)
        .await?;
    sqlx::query("UPDATE documents SET source_id=$2, external_key='budget' WHERE id=$1")
        .bind(doc)
        .bind(source)
        .execute(&f.pool)
        .await?;
    let (resume, old, server) = pause_transcription(&f, doc).await?;
    let text = "The NEW budget is 200.";
    use sha2::{Digest, Sha256};
    let sha: String = Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    f.state.blob.put(&sha, text.as_bytes()).await?;
    let mut tx = f.pool.begin().await?;
    let updated = utopia_store::documents::upsert_source_document_tx(
        &mut tx,
        f.kb,
        source,
        "budget",
        "budget.txt",
        "text/plain",
        text.len() as i64,
        &sha,
        None,
    )
    .await?;
    tx.commit().await?;
    assert_eq!(updated.id, doc);
    super::process_document(&f.state, doc).await?;
    // 新版本任务已经完成；旧读取若覆盖其分块，不会再有后续重试修复。
    let job: i64 = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind = 'process_document' AND payload->>'document_id' = $1",
    )
    .bind(doc.to_string())
    .fetch_one(&f.pool)
    .await?;
    sqlx::query("UPDATE jobs SET status = 'done', updated_at = now() WHERE id = $1")
        .bind(job)
        .execute(&f.pool)
        .await?;
    let completed = utopia_store::documents::get(&f.pool, doc).await?;
    resume.notify_one();
    let old_result = old.await?;
    server.abort();
    let live: Vec<String> = sqlx::query_scalar(
        "SELECT text FROM chunks WHERE document_id=$1 AND superseded_at IS NULL ORDER BY seq",
    )
    .bind(doc)
    .fetch_all(&f.pool)
    .await?;
    let current = utopia_store::documents::get(&f.pool, doc).await?;
    f.cleanup().await?;
    old_result?;
    assert_eq!(current.sha256, sha);
    assert_eq!(current.status, "ready");
    assert_eq!(current.text_len, completed.text_len);
    assert_eq!(
        live,
        vec![text.to_string()],
        "a completed newer revision must not be replaced by an old read"
    );
    Ok(())
}

#[tokio::test]
async fn a_late_reader_does_not_repopulate_a_deleted_document() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let doc = f.document_with_bytes("budget.mp3", &MP3).await?;
    let (resume, processing, server) = pause_transcription(&f, doc).await?;
    utopia_store::documents::delete(&f.pool, f.kb, doc, None).await?;
    let deleted = utopia_store::documents::get(&f.pool, doc).await?;
    resume.notify_one();
    let result = processing.await?;
    server.abort();
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chunks WHERE document_id=$1 AND superseded_at IS NULL",
    )
    .bind(doc)
    .fetch_one(&f.pool)
    .await?;
    let current = utopia_store::documents::get(&f.pool, doc).await?;
    f.cleanup().await?;
    result?;
    assert!(current.deleted_at.is_some());
    assert_eq!(current.status, deleted.status);
    assert_eq!(live, 0);
    Ok(())
}

/// 0040 第三刀：录音交给会标说话人的转写模型。说话人写进正文，每块记着起止时刻和说话人
#[tokio::test]
async fn a_recording_is_read_with_who_said_what() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let fake = FakeTranscriber {
        labels: true,
        ..Default::default()
    };
    with_transcriber(&f, &fake, "gpt-4o-transcribe-diarize").await?;
    let doc = f.document_with_bytes("board-meeting.mp3", &MP3).await?;
    super::process_document(&f.state, doc).await?;
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "ready", "{:?}", row.error);

    let req = fake.requests.lock().expect("lock").clone();
    assert_eq!(req.len(), 1);
    assert!(req[0].starts_with("Bearer asr-secret\n"));
    assert!(req[0].contains("diarized_json"));
    assert!(req[0].contains("gpt-4o-transcribe-diarize"));

    type Stored = (String, String, Option<String>, Option<serde_json::Value>);
    let chunks: Vec<Stored> = sqlx::query_as(
        "SELECT text, origin, origin_model, anchor FROM chunks
          WHERE document_id = $1 AND superseded_at IS NULL ORDER BY seq",
    )
    .bind(doc)
    .fetch_all(&f.pool)
    .await?;
    assert_eq!(chunks.len(), 1, "{chunks:#?}");
    assert_eq!(chunks[0].1, "transcribed");
    assert_eq!(chunks[0].2.as_deref(), Some("gpt-4o-transcribe-diarize"));
    assert!(chunks[0].0.contains("Speaker B: I will deliver it in Q3."));
    assert_eq!(
        chunks[0].3,
        Some(serde_json::json!({ "start_ms": 0, "end_ms": 7250, "speaker": ["A", "B"] }))
    );
    f.cleanup().await
}

/// 分不出说话人的转写：跟没配一样降级（决定 5）——不进库、告警说原因、不重试；换一个会标
/// 说话人的模型存下，录音重新排队读成
#[tokio::test]
async fn a_transcript_that_cannot_say_who_spoke_waits_for_one_that_can() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let unlabelled = FakeTranscriber::default();
    with_transcriber(&f, &unlabelled, "whisper-1").await?;
    let doc = f.document_with_bytes("board-meeting.mp3", &MP3).await?;
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("no speakers");
    assert!(utopia_core::is_terminal(&err));
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "failed");
    assert_eq!(row.reader_needed.as_deref(), Some("transcribe"));
    assert!(row
        .error
        .as_deref()
        .is_some_and(|e| e.contains("who spoke")));
    assert!(
        f.stored(doc).await?.is_empty(),
        "nothing unattributed is kept"
    );
    let alerts = f.alerts("document.needs_reader").await?;
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["reader"], "transcribe");

    let labelled = FakeTranscriber {
        labels: true,
        ..Default::default()
    };
    with_transcriber(&f, &labelled, "gpt-4o-transcribe-diarize").await?;
    let requeued =
        utopia_store::documents::requeue_waiting_for_reader(&f.pool, f.ws, "transcribe").await?;
    assert_eq!(requeued, vec![(doc, f.kb)]);
    super::process_document(&f.state, doc).await?;
    assert_eq!(
        utopia_store::documents::get(&f.pool, doc).await?.status,
        "ready"
    );
    f.cleanup().await
}

/// 读不了的格式（老式 .doc、压缩包）：一次失败、不重试，也不进库成乱码；它不缺模型，不报那条告警
#[tokio::test]
async fn a_binary_file_fails_once_instead_of_becoming_text() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(5))).await? else {
        return Ok(());
    };
    let doc = f
        .document_with_bytes(
            "minutes.doc",
            &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0, 0, 0, 0],
        )
        .await?;
    let err = super::process_document(&f.state, doc)
        .await
        .expect_err("not text");
    assert!(utopia_core::is_terminal(&err));
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "failed");
    assert_eq!(row.reader_needed, None);
    assert!(f.stored(doc).await?.is_empty());
    assert!(f.alerts("document.needs_reader").await?.is_empty());
    f.cleanup().await
}

#[test]
fn without_nul_borrows_when_there_is_nothing_to_strip() {
    use std::borrow::Cow;
    assert!(matches!(
        utopia_core::without_nul("plain text"),
        Cow::Borrowed(_)
    ));
    assert_eq!(utopia_core::without_nul("a\0b\0\0c"), "abc");
    assert_eq!(utopia_core::without_nul("\0"), "");
}

#[tokio::test]
async fn a_document_is_fully_embedded_before_it_is_ready() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::from_millis(20))).await? else {
        return Ok(());
    };
    let doc = f.document_to_process(20).await?;
    super::process_document(&f.state, doc).await?;
    let row = utopia_store::documents::get(&f.pool, doc).await?;
    assert_eq!(row.status, "ready");
    let stored = f.stored(doc).await?;
    assert_eq!(stored.len() as i32, row.chunk_count);
    assert!(
        stored.len() > super::EMBED_BATCH,
        "enough chunks for more than one batch"
    );
    assert!(
        stored.iter().all(|(_, v)| v.is_some()),
        "no chunk is left without a vector when the document is ready"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_memory_episode_embeds_by_the_same_path_as_a_document() -> anyhow::Result<()> {
    let Some(f) = fixture(FakeEmbed::new(Duration::ZERO)).await? else {
        return Ok(());
    };
    let doc = f.document_with_chunks(5).await?;
    super::memory_ingest(
        &f.state,
        doc,
        Proposer {
            user_id: None,
            token_id: None,
        },
    )
    .await?;
    for (text, vector) in f.stored(doc).await? {
        assert_eq!(vector.as_deref(), Some(vector_of(&text).as_slice()));
    }
    f.cleanup().await
}
