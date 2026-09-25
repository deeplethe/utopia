//! 推陈述的来源（0054）：请求体就是开放抽取契约，门口拒绝契约之外的键，通过的载荷
//! 整份成一块，抽取按契约解析而**不问模型**——夹具故意不配对话模型，证明这条路不需要它。
//! 连库的部分没有 `UTOPIA_DATABASE_URL` 就跳过（同 documents_routes_tests）。

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use utopia_core::models::Proposer;
use utopia_store::documents;
use uuid::Uuid;

/// 门口的校验不连库，任何环境都跑
#[test]
fn the_door_refuses_what_the_contract_has_no_slot_for() {
    let ok = json!({
        "external_id": "obs-1",
        "e": [["cup-7", "cup", true], ["kitchen table", "table", true]],
        "s": [[null, "cup-7", "is on", "kitchen table", null, {}, "08:14:03", null]],
        "n": []
    });
    let (_, content) = super::validate_statements_payload(ok.to_string().as_bytes())
        .expect("a well-formed contract passes");
    let content = content.expect("a non-tombstone yields the document text");
    // 存的是我们重新序列化的那份：带着这次观测的身份，能被抽取用的同一个解析器读回
    // （它只读三个数组，身份和日期两个键它不看）
    let stored: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(stored["external_id"], "obs-1");
    assert!(
        stored.get("doc_time").is_none(),
        "no date was given, none is stored"
    );
    assert_eq!(
        stored.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["e", "external_id", "n", "s"],
        "identity plus the three arrays, nothing else"
    );
    let parsed = utopia_extract::open::parse_open_response(&content).unwrap();
    assert_eq!(parsed.statements.len(), 1);
    assert_eq!(parsed.statements[0].phrase, "is on");

    let refuse = |body: Value, needle: &str| {
        let err = super::validate_statements_payload(body.to_string().as_bytes())
            .err()
            .unwrap_or_else(|| panic!("{body} must be refused"));
        assert!(err.contains(needle), "{err:?} should mention {needle:?}");
    };
    // 契约里没有属性的格子：一个 `predicate` 键在门口就拦下，而不是静默忽略
    let mut typed = ok.clone();
    typed["predicate"] = json!("located_in");
    refuse(typed, "unknown key");
    // 引文格必须为空：条目自己就是证据
    let mut quoted = ok.clone();
    quoted["s"][0][0] = json!("cup-7 is on the kitchen table");
    refuse(quoted, "quote");
    // 八格少一格不是截断，是形状错
    let mut short = ok.clone();
    short["s"][0] = json!([null, "cup-7", "is on", "kitchen table"]);
    refuse(short, "eight");
    // 没有身份就没有更新语义
    let mut anon = ok.clone();
    anon["external_id"] = json!("  ");
    refuse(anon, "external_id");
    // 空陈述数组：什么都推不进图，直说
    let mut empty = ok.clone();
    empty["s"] = json!([]);
    refuse(empty, "at least one");
    // 主语没在 `e` 里：抽取会把它作为 UNKNOWN_REF 静默丢掉，门口就得说不
    let mut stray = ok.clone();
    stray["s"][0][1] = json!("cup-8");
    refuse(stray, "not a thing listed in e");
    // 只差空白和大小写的算同一个名字（与抽取的 `name_key` 同一条折叠规则）
    let mut folded = ok.clone();
    folded["s"][0][1] = json!("  Cup-7 ");
    super::validate_statements_payload(folded.to_string().as_bytes())
        .expect("whitespace and case do not make a different thing");
    // 别名所属的东西也一样；别名的引文格同样必须为空
    let mut alias = ok.clone();
    alias["n"] = json!([["mug-7", "the cup", null]]);
    refuse(alias, "n[0][0]");
    let mut alias_quoted = ok.clone();
    alias_quoted["n"] = json!([["cup-7", "the cup", "the cup sat there"]]);
    refuse(alias_quoted, "n[0][2]");
    // 条数和字节数的上限：第一刀的限制，超过直说而不是截断
    let mut many = ok.clone();
    many["s"] = json!(vec![ok["s"][0].clone(); 201]);
    refuse(many, "limit is 200");
    let mut fat = ok.clone();
    fat["s"][0][5] = json!({ "note": "x".repeat(64 * 1024) });
    refuse(fat, "limit is 65536");
}

struct Fixture {
    pool: sqlx::PgPool,
    state: crate::state::AppState,
    app: axum::Router,
    org: Uuid,
    kb: Uuid,
    source: Uuid,
    api_source: Uuid,
    token: String,
    api_token: String,
    /// 编辑者的会话令牌：查看和轮换密钥走会话认证，不走推送密钥
    session: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, user, source, api_source) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // Only locally generated UUIDs are interpolated into fixture SQL.
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','statements-push-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','statements-push-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash)
                 VALUES ('{user}','{org}','{user}@statements.test','statements-test','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','observations');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{user}','editor');
             INSERT INTO sources(id,kb_id,kind,name) VALUES
                 ('{source}','{kb}','statements','robot-1'),
                 ('{api_source}','{kb}','api','api-source');"
        ))
        .execute(&pool)
        .await?;
        // 故意**不**配对话模型：这条路不需要它
        let token = super::new_ingest_token();
        utopia_store::sources::set_ingest_token(&pool, source, &token).await?;
        let api_token = super::new_ingest_token();
        utopia_store::sources::set_ingest_token(&pool, api_source, &api_token).await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
        let session = crate::auth::issue_token(&state, user)?;
        let app = super::super::router(state.clone(), &cfg);
        Ok(Some(Self {
            pool,
            state,
            app,
            org,
            kb,
            source,
            api_source,
            token,
            api_token,
            session,
            _dir: dir,
        }))
    }

    async fn push(
        &self,
        source: Uuid,
        token: &str,
        body: &Value,
    ) -> anyhow::Result<(StatusCode, Value)> {
        self.push_raw(source, Some(token), body.to_string().into_bytes())
            .await
    }

    /// 不带 Authorization 头（`None`）或推原始字节：门口的 401 和字节上限要从 HTTP 这一侧看
    async fn push_raw(
        &self,
        source: Uuid,
        token: Option<&str>,
        body: Vec<u8>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let mut request = Request::post(format!("/api/v1/sources/{source}/statements"))
            .header("Content-Type", "application/json");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = self
            .app
            .clone()
            .oneshot(request.body(Body::from(body))?)
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await?;
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, value))
    }

    /// 以编辑者会话调一个来源接口（查看 / 轮换密钥）
    async fn as_editor(&self, method: &str, path: &str) -> anyhow::Result<(StatusCode, Value)> {
        let response = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/v1/kbs/{}/sources/{path}", self.kb))
                    .header("Authorization", format!("Bearer {}", self.session))
                    .body(Body::empty())?,
            )
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await?;
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Ok((status, value))
    }

    async fn cleanup(self) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM organizations WHERE id = $1")
            .bind(self.org)
            .execute(&self.pool)
            .await?;
        self.pool.close().await;
        Ok(())
    }
}

fn observation(when: &str, place: &str) -> Value {
    json!({
        "external_id": "obs-000412",
        "doc_time": "2026-09-23T08:14:03Z",
        "e": [["cup-7", "cup", true], [place, "table", true]],
        "s": [[null, "cup-7", "is on", place, null, {}, when, null]],
        "n": []
    })
}

/// 推一条陈述，走完处理与抽取，它就是一条开放陈述：有短语、有主宾实体、有证据行
/// （块 = 载荷，偏移为空），而工作区没有任何对话模型
#[tokio::test]
async fn a_pushed_statement_reaches_the_open_graph_without_a_model() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "created");

    let doc = documents::find_by_external_key(&f.pool, f.source, "statements:obs-000412")
        .await?
        .expect("the push created a document under its identity");
    assert_eq!(doc.mime, "application/json");

    crate::pipeline::process_document(&f.state, doc.id).await?;
    let chunks: Vec<(String,)> =
        sqlx::query_as("SELECT text FROM chunks WHERE document_id = $1 AND superseded_at IS NULL")
            .bind(doc.id)
            .fetch_all(&f.pool)
            .await?;
    assert_eq!(
        chunks.len(),
        1,
        "the payload is one chunk, not a budgeted split"
    );
    utopia_extract::open::parse_open_response(&chunks[0].0)
        .expect("the chunk is the contract verbatim");

    crate::extraction::extract_document(
        &f.state,
        doc.id,
        Proposer {
            user_id: None,
            token_id: None,
        },
    )
    .await?;
    let (status,): (String,) = sqlx::query_as("SELECT graph_status FROM documents WHERE id = $1")
        .bind(doc.id)
        .fetch_one(&f.pool)
        .await?;
    assert_ne!(status, "failed", "extraction must not need a chat model");

    let facts: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, phrase FROM facts
          WHERE kb_id = $1 AND layer = 'open' AND invalidated_at IS NULL",
    )
    .bind(f.kb)
    .fetch_all(&f.pool)
    .await?;
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(facts[0].1, "is on");
    let (chunk_matches, quote_null, offsets_null): (bool, bool, bool) = sqlx::query_as(
        "SELECT chunk_id = (SELECT id FROM chunks WHERE document_id = $2 AND superseded_at IS NULL),
                quote IS NULL, quote_start IS NULL AND quote_end IS NULL
           FROM fact_evidence WHERE fact_id = $1",
    )
    .bind(facts[0].0)
    .bind(doc.id)
    .fetch_one(&f.pool)
    .await?;
    assert!(chunk_matches, "the evidence is the payload's own chunk");
    assert!(
        quote_null && offsets_null,
        "the item is its own evidence: no quote, no offsets"
    );
    let (entities,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM entities
          WHERE kb_id = $1 AND canonical_name IN ('cup-7', 'kitchen table')",
    )
    .bind(f.kb)
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(
        entities, 2,
        "both things are entities with the pushed names"
    );
    // 时间词落成提及（0054 决定 3）：没有引文时在载荷自己里找，而不是走引文路退出
    let mentions: Vec<(String, String)> =
        sqlx::query_as("SELECT text, role FROM time_mentions WHERE fact_id = $1")
            .bind(facts[0].0)
            .fetch_all(&f.pool)
            .await?;
    assert_eq!(
        mentions,
        vec![("08:14:03".to_string(), "when".to_string())],
        "the pushed `when` is a time mention on the fact"
    );
    f.cleanup().await
}

/// 同一身份再推一份新内容是更新：原地替换并记版本，和 `api` 推送一个语义
#[tokio::test]
async fn a_second_push_under_the_same_identity_updates_in_place() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "unchanged", "same content is a no-op");
    let (status, body) = f
        .push(f.source, &f.token, &observation("08:20:00", "counter"))
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "updated");
    let doc = documents::find_by_external_key(&f.pool, f.source, "statements:obs-000412")
        .await?
        .expect("still one document under the identity");
    let (versions,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM document_versions WHERE document_id = $1")
            .bind(doc.id)
            .fetch_one(&f.pool)
            .await?;
    assert_eq!(versions, 2, "the update recorded a version");
    f.cleanup().await
}

/// 门口的拒绝走到 HTTP 是 422（`AppError::Validation`，与 `api` 推送被拒时同一个码），
/// 并且这次推送留在 run 历史里；`api` 来源不认这条路由（404），钥匙不对是 401
#[tokio::test]
async fn the_route_answers_422_404_and_401_at_the_door() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let mut typed = observation("08:14:03", "kitchen table");
    typed["class"] = json!("Cup");
    let (status, body) = f.push(f.source, &f.token, &typed).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, _) = f
        .push(
            f.api_source,
            &f.api_token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an api source has no statements route"
    );
    let (status, _) = f
        .push(
            f.source,
            "utp_not-the-key",
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = f
        .push_raw(
            f.source,
            None,
            observation("08:14:03", "kitchen table")
                .to_string()
                .into_bytes(),
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no header is no key");
    // 64 KiB 上限从 HTTP 这一侧看仍是 422 带说明，不是路由层的 413
    let mut fat = observation("08:14:03", "kitchen table");
    fat["s"][0][5] = json!({ "note": "x".repeat(64 * 1024) });
    let (status, body) = f.push(f.source, &f.token, &fat).await?;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body.to_string().contains("limit is 65536"),
        "the refusal names the limit: {body}"
    );
    f.cleanup().await
}

/// 墓碑与复活，和 `api` 推送同一语义：`deleted: true` 给该身份打 "Not in source" 标记而不删；
/// 同一身份再推内容就把标记清掉。没见过的身份打墓碑是空操作，照样回 marked_missing
#[tokio::test]
async fn a_tombstone_marks_the_item_missing_and_a_new_push_revives_it() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &json!({ "external_id": "obs-000412", "deleted": true }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "marked_missing");
    let doc = documents::find_by_external_key(&f.pool, f.source, "statements:obs-000412")
        .await?
        .expect("a tombstone marks, it does not delete");
    let missing: (bool, bool) = sqlx::query_as(
        "SELECT missing_since IS NOT NULL, deleted_at IS NULL FROM documents WHERE id = $1",
    )
    .bind(doc.id)
    .fetch_one(&f.pool)
    .await?;
    assert_eq!(missing, (true, true), "marked missing, still present");
    // 复活：同一身份、同样内容——文档没变（unchanged），但标记清掉了
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "unchanged");
    let (revived,): (bool,) =
        sqlx::query_as("SELECT missing_since IS NULL FROM documents WHERE id = $1")
            .bind(doc.id)
            .fetch_one(&f.pool)
            .await?;
    assert!(revived, "a new push under the identity clears the marker");
    // 没见过的身份：打不到任何文档，也不算错
    let (status, body) = f
        .push(
            f.source,
            &f.token,
            &json!({ "external_id": "never-pushed", "deleted": true }),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "marked_missing");
    assert!(
        documents::find_by_external_key(&f.pool, f.source, "statements:never-pushed")
            .await?
            .is_none(),
        "a tombstone never creates a document"
    );
    f.cleanup().await
}

/// 密钥的查看和轮换对 `statements` 来源和 `api` 来源一样可用：创建时给过一次的密钥
/// 之后还查得到，轮换后旧密钥立刻失效、新密钥能推。端到端跑服务时抓到的：这两个接口
/// 原来只认 `api`
#[tokio::test]
async fn the_push_token_can_be_viewed_and_rotated_like_an_api_source() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, body) = f.as_editor("GET", &format!("{}/token", f.source)).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["ingest_token"], f.token,
        "the token given at creation is viewable"
    );
    let (status, body) = f
        .as_editor("POST", &format!("{}/rotate-token", f.source))
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let rotated = body["ingest_token"]
        .as_str()
        .expect("a new token")
        .to_string();
    assert_ne!(rotated, f.token);
    let (status, _) = f
        .push(
            f.source,
            &f.token,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old token is dead");
    let (status, body) = f
        .push(
            f.source,
            &rotated,
            &observation("08:14:03", "kitchen table"),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    f.cleanup().await
}

/// 同一份载荷在新身份下是另一次观测（#900）：两次看到杯子在桌上就是两篇文档，各带自己的
/// 日期。身份写在正文里，所以两篇正文不同，库里「一份内容一篇文档」的唯一性和文件型
/// 来源那条「同内容出现在新路径 = 改名」的识别都碰不到它
#[tokio::test]
async fn the_same_payload_under_a_new_identity_is_a_second_observation() -> anyhow::Result<()> {
    let Some(f) = Fixture::new().await? else {
        return Ok(());
    };
    let mut first = observation("08:14:03", "kitchen table");
    first["external_id"] = json!("obs-1");
    first["doc_time"] = json!("2026-09-23T08:14:03Z");
    let (status, body) = f.push(f.source, &f.token, &first).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "created");
    let mut second = first.clone();
    second["external_id"] = json!("obs-2");
    second["doc_time"] = json!("2026-09-23T08:20:00Z");
    let (status, body) = f.push(f.source, &f.token, &second).await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["action"], "created", "not a move");
    for (key, when) in [
        ("statements:obs-1", "2026-09-23T08:14:03Z"),
        ("statements:obs-2", "2026-09-23T08:20:00Z"),
    ] {
        let doc = documents::find_by_external_key(&f.pool, f.source, key)
            .await?
            .unwrap_or_else(|| panic!("{key} is its own document"));
        assert_eq!(
            doc.doc_time
                .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            Some(when.to_string()),
            "each observation keeps its own date"
        );
    }
    f.cleanup().await
}
