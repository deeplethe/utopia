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
        use sha2::{Digest, Sha256};
        let sha: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.state.blob.put(&sha, text.as_bytes()).await?;
        Ok(utopia_store::documents::create(
            &self.pool,
            self.kb,
            "long.md",
            "text/markdown",
            text.len() as i64,
            &sha,
            None,
            None,
            None,
        )
        .await?
        .id)
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

#[test]
fn without_nul_borrows_when_there_is_nothing_to_strip() {
    use std::borrow::Cow;
    assert!(matches!(super::without_nul("plain text"), Cow::Borrowed(_)));
    assert_eq!(super::without_nul("a\0b\0\0c"), "abc");
    assert_eq!(super::without_nul("\0"), "");
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
