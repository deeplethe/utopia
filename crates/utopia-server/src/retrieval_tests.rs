//! #515：混合检索的两路各走各的，丢一路是降级不是失败。
//!
//! 夹具：一个库、一篇文档、两个分块。A 的正文能被 BM25 命中，B 不能；
//! 两块各有一个 4 维向量，嵌入端点用 wiremock 假扮，想回什么向量就回什么。
//! 四件事：
//! 1. **没配嵌入模型也能答**：只剩 BM25 那一路，A 回来，B 不回来。
//! 2. **嵌入请求失败退化成 BM25**：端点回 500，结果与上一条一样，不是报错。
//! 3. **两路都在时按 BM25 在前融合**：端点回的向量离 A 最近，A 同时在两路里，排第一；
//!    B 只在向量那一路，排第二。
//! 4. **历史过滤先于最终截断**：当时不存在的高排名块不能挤掉已召回的有效块。

use std::sync::Arc;
use uuid::Uuid;
use wiremock::{matchers::method, matchers::path, Mock, MockServer, ResponseTemplate};

struct Fixture {
    pool: sqlx::PgPool,
    state: crate::state::AppState,
    org: Uuid,
    ws: Uuid,
    kb: Uuid,
    doc: Uuid,
    a: Uuid,
    b: Uuid,
    dir: std::path::PathBuf,
}

async fn fixture() -> anyhow::Result<Option<Fixture>> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(None);
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, ws, kb, doc, a, b) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'retrieval-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'retrieval-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'retrieval-test')")
        .bind(kb)
        .bind(ws)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO documents(id,kb_id,filename,sha256) VALUES($1,$2,'orchard.md',repeat('0',64))",
    )
    .bind(doc)
    .bind(kb)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO chunks(id,kb_id,document_id,seq,text,embedding) VALUES
         ($1,$3,$4,0,'apple orchard harvest report','[1,0,0,0]'::vector),
         ($2,$3,$4,1,'bridge concrete inspection','[0,1,0,0]'::vector)",
    )
    .bind(a)
    .bind(b)
    .bind(kb)
    .bind(doc)
    .execute(&pool)
    .await?;

    let dir = std::env::temp_dir().join(format!("utopia-retrieval-{kb}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    search.reindex_document(
        &kb.to_string(),
        &doc.to_string(),
        &[
            (a.to_string(), "apple orchard harvest report".into()),
            (b.to_string(), "bridge concrete inspection".into()),
        ],
    )?;
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    Ok(Some(Fixture {
        pool,
        state,
        org,
        ws,
        kb,
        doc,
        a,
        b,
        dir,
    }))
}

impl Fixture {
    async fn point_embeddings_at(&self, server: &MockServer) -> anyhow::Result<()> {
        utopia_store::settings::upsert(
            &self.pool,
            self.ws,
            None,
            None,
            None,
            Some(&server.uri()),
            None,
            Some("fake-embed"),
            None,
        )
        .await?;
        Ok(())
    }

    async fn search(&self, query: &str) -> anyhow::Result<Vec<Uuid>> {
        let chunks = super::hybrid(&self.state, self.kb, self.ws, query, 8, None).await?;
        Ok(chunks.into_iter().map(|c| c.id).collect())
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id=$1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        let _ = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

#[tokio::test]
async fn a_search_without_an_embedding_model_still_answers() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let ids = f.search("apple harvest").await?;
    assert_eq!(
        ids,
        vec![f.a],
        "BM25 alone finds the orchard chunk and nothing else"
    );
    f.cleanup().await
}

#[tokio::test]
async fn a_failed_query_embedding_degrades_to_bm25() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(500).set_body_string("model is down"))
        .mount(&server)
        .await;
    f.point_embeddings_at(&server).await?;
    let ids = f.search("apple harvest").await?;
    assert_eq!(
        ids,
        vec![f.a],
        "a dead embedding endpoint costs the vector channel, not the answer"
    );
    f.cleanup().await
}

#[tokio::test]
async fn both_channels_fuse_with_bm25_first() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    let server = MockServer::start().await;
    // 回的向量就是 A 的向量：向量那一路按距离排出 [A, B]，BM25 那一路只有 [A]
    Mock::given(method("POST"))
        .and(path("/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{ "embedding": [1.0, 0.0, 0.0, 0.0] }]
        })))
        .mount(&server)
        .await;
    f.point_embeddings_at(&server).await?;
    let ids = f.search("apple harvest").await?;
    assert_eq!(
        ids,
        vec![f.a, f.b],
        "the chunk both channels found ranks first; the vector-only chunk still comes back"
    );
    f.cleanup().await
}

#[tokio::test]
async fn historical_search_keeps_live_candidates_before_limiting_results() -> anyhow::Result<()> {
    let Some(f) = fixture().await? else {
        return Ok(());
    };
    sqlx::query("UPDATE documents SET created_at='2026-01-01' WHERE id=$1")
        .bind(f.doc)
        .execute(&f.pool)
        .await?;
    sqlx::query("UPDATE chunks SET text='apple harvest', created_at='2026-05-01' WHERE id=$1")
        .bind(f.a)
        .execute(&f.pool)
        .await?;
    sqlx::query("UPDATE chunks SET text='apple orchard bridge inspection', created_at='2026-01-01' WHERE id=$1")
        .bind(f.b)
        .execute(&f.pool)
        .await?;
    let mut indexed = vec![
        (f.a.to_string(), "apple harvest".to_string()),
        (
            f.b.to_string(),
            "apple orchard bridge inspection".to_string(),
        ),
    ];
    for seq in 2..7 {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO chunks(id,kb_id,document_id,seq,text,created_at) VALUES($1,$2,$3,$4,'apple harvest','2026-05-01')")
            .bind(id)
            .bind(f.kb)
            .bind(f.doc)
            .bind(seq)
            .execute(&f.pool)
            .await?;
        indexed.push((id.to_string(), "apple harvest".to_string()));
    }
    f.state
        .search
        .reindex_document(&f.kb.to_string(), &f.doc.to_string(), &indexed)?;

    // All seven hits are already in today's index. This is not the separate
    // limitation that BM25 cannot recall historical versions absent from it.
    let raw = f
        .state
        .search
        .search(&f.kb.to_string(), "apple harvest", 24)?;
    assert_eq!(raw.len(), 7);
    assert_eq!(raw.last().unwrap().chunk_id, f.b.to_string());
    let current = super::hybrid(&f.state, f.kb, f.ws, "apple harvest", 6, None).await?;
    assert_eq!(
        current.iter().map(|c| c.id.to_string()).collect::<Vec<_>>(),
        raw.iter()
            .take(6)
            .map(|h| h.chunk_id.clone())
            .collect::<Vec<_>>(),
        "the current search still honors its limit and ranking",
    );

    // The first six hits do not exist in March. They must not consume the
    // final limit and discard the one recalled chunk that does exist then.
    let at = Some("2026-03-01T00:00:00Z".parse()?);
    let six = super::hybrid(&f.state, f.kb, f.ws, "apple harvest", 6, at).await?;
    let seven = super::hybrid(&f.state, f.kb, f.ws, "apple harvest", 7, at).await?;
    let six: Vec<_> = six.into_iter().map(|c| c.id).collect();
    let seven: Vec<_> = seven.into_iter().map(|c| c.id).collect();
    let expected = vec![f.b];
    f.cleanup().await?;
    assert_eq!(seven, expected, "the eligible chunk was recalled");
    assert_eq!(
        six, expected,
        "ineligible hits must not consume the final limit"
    );
    Ok(())
}
