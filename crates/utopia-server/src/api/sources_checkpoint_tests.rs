//! 来源的增量游标在 API 里看得见（ADR 0072）。
//!
//! **说的是「读到哪」，不是「这一轮什么时候跑」。** 一轮读不完是常事——同一个更新
//! 时刻上可能挂着一千条工单——游标记的是「消费到了哪个更新时刻，且那一刻的整组都
//! 读完了」。没有它，界面上只看得见「同步成功」，看不见这一轮停在了哪里；要排
//! 「这一轮到底覆盖了多少」，只能去翻日志。
//!
//! 用不到的来源那一格是 NULL，而不是 0 也不是别的时刻：没有游标与「游标在零点」
//! 是两回事，前者是「还没读到哪」，后者是「读到 1970 年」。
//!
//! 连库的测试，没有 `UTOPIA_DATABASE_URL` 就跳过（同 documents_routes_tests）。

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

/// 游标落在这一刻：秒精度，正是工单更新时刻的粒度。
///
/// **响应里写的是 `Z` 结尾的那一种**（chrono 对 `DateTime<Utc>` 的写法），
/// 不是 `+00:00`——断言照着实际发出去的形式写，变了就是变了。
const CHECKPOINT: &str = "2026-01-02T00:00:00Z";

fn checkpoint() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(CHECKPOINT)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

struct Fixture {
    app: axum::Router,
    kb: Uuid,
    /// 工单来源：有游标
    ticket: Uuid,
    /// 订阅来源：没有游标
    feed: Uuid,
    token: String,
    _dir: tempfile::TempDir,
}

impl Fixture {
    async fn new() -> anyhow::Result<Option<Self>> {
        let Some(url) = utopia_store::test_db::url() else {
            return Ok(None);
        };
        let pool = sqlx::PgPool::connect(&url).await?;
        utopia_store::db::migrate(&pool).await?;
        let (org, ws, kb, user, ticket, feed) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        // Only locally generated UUIDs are interpolated into fixture SQL.
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES ('{org}','checkpoint-test');
             INSERT INTO workspaces(id,org_id,name) VALUES ('{ws}','{org}','checkpoint-test');
             INSERT INTO users(id,org_id,email,display_name,password_hash)
                 VALUES ('{user}','{org}','{user}@checkpoint.test','checkpoint-test','unused');
             INSERT INTO knowledge_bases(id,workspace_id,name) VALUES ('{kb}','{ws}','observations');
             INSERT INTO kb_members(kb_id,user_id,role) VALUES ('{kb}','{user}','editor');
             INSERT INTO sources(id,kb_id,kind,name,config)
                 VALUES ('{ticket}','{kb}','github_issues','tickets','{{\"repo\":\"o/r\"}}');
             INSERT INTO sources(id,kb_id,kind,name,config)
                 VALUES ('{feed}','{kb}','rss','feed','{{\"feed_url\":\"https://example.test/f\"}}');"
        ))
        .execute(&pool)
        .await?;
        // 工单来源读到了那一刻；订阅来源不按时刻分组，没有这一格
        sqlx::query("UPDATE sources SET sync_checkpoint = $2 WHERE id = $1")
            .bind(ticket)
            .bind(checkpoint())
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
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
        let token = crate::auth::issue_token(&state, user)?;
        let app = super::super::router(state.clone(), &cfg);
        Ok(Some(Self {
            app,
            kb,
            ticket,
            feed,
            token,
            _dir: dir,
        }))
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> anyhow::Result<(StatusCode, Value)> {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("Authorization", format!("Bearer {}", self.token));
        let request = match body {
            Some(body) => request
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))?,
            None => request.body(Body::empty())?,
        };
        let response = self.app.clone().oneshot(request).await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1 << 20).await?;
        Ok((
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        ))
    }
}

/// **列表里有这一格。** 工单来源带着游标，用不到游标的来源是 NULL。
#[tokio::test]
async fn the_source_list_carries_the_checkpoint() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, body) = fx
        .call("GET", &format!("/api/v1/kbs/{}/sources", fx.kb), None)
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sources = body["sources"].as_array().expect("sources[]");
    let ticket = sources
        .iter()
        .find(|s| s["id"] == fx.ticket.to_string())
        .expect("工单来源在列表里");
    let feed = sources
        .iter()
        .find(|s| s["id"] == fx.feed.to_string())
        .expect("订阅来源在列表里");

    // **时刻本身要给出来**，不是"有几个"：那一格说的是读到哪
    assert_eq!(
        ticket["sync_checkpoint"],
        json!(CHECKPOINT),
        "工单来源的游标要原样给出"
    );
    // 用不到的来源是 NULL——没有游标与"游标在零点"是两回事
    assert_eq!(
        feed["sync_checkpoint"],
        Value::Null,
        "用不到游标的来源不该编一个时刻出来"
    );
    // 游标与"这一轮什么时候跑"是两件事，两个都在
    assert!(
        ticket["last_sync_at"].is_null(),
        "还没同步过就没有这一轮的时刻"
    );
    Ok(())
}

/// **改一个来源，回来的那一格也带着游标。** 列表与详情说的是同一件事，
/// 不能一处有一处没有。
#[tokio::test]
async fn a_source_update_carries_the_checkpoint_too() -> anyhow::Result<()> {
    let Some(fx) = Fixture::new().await? else {
        return Ok(());
    };
    let (status, body) = fx
        .call(
            "PATCH",
            &format!("/api/v1/kbs/{}/sources/{}", fx.kb, fx.ticket),
            Some(json!({ "name": "renamed" })),
        )
        .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["source"]["sync_checkpoint"],
        json!(CHECKPOINT),
        "更新回来的那一格也要带游标"
    );
    Ok(())
}
