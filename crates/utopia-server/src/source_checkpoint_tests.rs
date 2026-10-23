//! 增量游标代表已成功覆盖的窗口，失败的尝试不能把尚未读到的条目跳过去。
use super::sync_source;
use std::sync::Arc;
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

async fn retry_reads_the_uncovered_window(
    prior_success: bool,
    fail_before_retry: bool,
) -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, source) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'checkpoint-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'checkpoint-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'checkpoint-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;
    let server = MockServer::start().await;
    sqlx::query(
        "INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,'custom','fixture',$3)",
    )
    .bind(source)
    .bind(kb)
    .bind(serde_json::json!({"endpoint":server.uri()}))
    .execute(&pool)
    .await?;
    let started = chrono::Utc::now() - chrono::Duration::hours(2);
    let changed = started + chrono::Duration::minutes(30);
    if prior_success {
        let run = utopia_store::sources::start_run(&pool, source).await?;
        utopia_store::sources::finish_run(&pool, run, source, None, 0, 0).await?;
        sqlx::query("UPDATE source_sync_runs SET started_at=$2,finished_at=$3 WHERE id=$1")
            .bind(run)
            .bind(started)
            .bind(started + chrono::Duration::hours(1))
            .execute(&pool)
            .await?;
        utopia_store::sources::touch_sync_time(&pool, source, started + chrono::Duration::hours(1))
            .await?;
    }
    let dir = std::env::temp_dir().join(format!("utopia-checkpoint-{source}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());
    let result = async {
        if fail_before_retry {
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
            assert!(sync_source(&state, source).await.is_err());
            let failed = utopia_store::sources::get(&pool, source).await?;
            assert_eq!(failed.last_sync_status, "failed");
            assert!(failed.last_sync_at.is_some(), "attempt time still drives scheduling and diagnostics");
            server.reset().await;
        }
        // **把真正发出去的下界记下来**：只断言那条漏掉的更新回来了是不够的，
        // 假如下界根本没发（每次都全量拉），这个断言照样过，而增量就悄悄没了
        let asked: std::sync::Arc<std::sync::Mutex<Vec<Option<chrono::DateTime<chrono::Utc>>>>> =
            Default::default();
        let seen = asked.clone();
        Mock::given(method("GET")).respond_with(move |request: &Request| {
            let since = request.url.query_pairs().find(|(k,_)| k == "since")
                .map(|(_,v)| chrono::DateTime::parse_from_rfc3339(&v).unwrap().with_timezone(&chrono::Utc));
            seen.lock().unwrap().push(since);
            let items = if since.is_none_or(|s| s <= changed) {
                serde_json::json!([{"id":"missed", "title":"Missed update", "content":"An update from the uncovered window"}])
            } else { serde_json::json!([]) };
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"items":items}))
        }).mount(&server).await;
        sync_source(&state, source).await?;
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM documents WHERE source_id=$1")
            .bind(source).fetch_one(&pool).await?;
        anyhow::ensure!(count == 1, "incremental cursor skipped an unimported update: {count} documents");
        let ok = utopia_store::sources::get(&pool, source).await?;
        assert_eq!(ok.last_sync_status, "ok");
        // 下界取自上一次**成功**那一轮的开始，不是上一次尝试的结束，也不是没有下界
        let asked = asked.lock().unwrap().clone();
        let last = asked.last().copied().flatten();
        if prior_success {
            let last = last.expect("没有带下界：增量拉取整个没了");
            assert!(
                (last - started).num_seconds().abs() <= 1,
                "下界应当是上一次成功那轮的开始 {started}，实得 {last}"
            );
            assert!(last < changed, "下界晚于那次改动，漏掉的窗口又被跳过了");
        } else {
            assert!(last.is_none(), "没有成功过的源不该带下界，实得 {last:?}");
        }
        Ok::<_, anyhow::Error>(())
    }.await;
    sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    drop(state);
    let _ = std::fs::remove_dir_all(dir);
    result
}

#[tokio::test]
async fn a_failed_first_sync_does_not_skip_the_existing_items() -> anyhow::Result<()> {
    retry_reads_the_uncovered_window(false, true).await
}

#[tokio::test]
async fn retry_keeps_updates_from_the_previous_successful_run_window() -> anyhow::Result<()> {
    retry_reads_the_uncovered_window(true, true).await
}

#[tokio::test]
async fn updates_during_a_successful_sync_remain_in_the_next_window() -> anyhow::Result<()> {
    retry_reads_the_uncovered_window(true, false).await
}

/// #1212 / ADR 0072（Custom 一节）：201 条全都变了，第一轮装下 200 条就停，
/// 第二轮把没变的前缀让过去、走到第 201 条。
///
/// 关键的两条性质都在这里：
///
/// 1. 触顶那一轮 `incomplete` → run 写 `failed` → 下界不前进 → 第二轮拿到的还是同一条
///    since，因此前 200 条原样回来、但它们现在都是 `Unchanged` 不占预算，第 201 条
///    这次够得着了。ADR 把「取走同样的前 200 条」列在 Rejected 里说的就是这条。
/// 2. `stats.discovered` 记的是响应里的条数（201），不是跑进来的条数（200）——
///    否则日志与那条警告会读成 `(0 > 200)`。
#[tokio::test]
async fn a_truncated_custom_batch_resumes_on_the_next_run() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, source) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations(id,name) VALUES($1,'truncation-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query("INSERT INTO workspaces(id,org_id,name) VALUES($1,$2,'truncation-test')")
        .bind(ws)
        .bind(org)
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases(id,workspace_id,name) VALUES($1,$2,'truncation-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(&pool)
    .await?;

    // **来源先插，`start_run` 后跑**：source_sync_runs.source_id 上有外键，
    // 反过来插会以 `source_sync_runs_source_id_fkey` 失败。
    let server = MockServer::start().await;
    sqlx::query(
        "INSERT INTO sources(id,kb_id,kind,name,config) VALUES($1,$2,'custom','fixture',$3)",
    )
    .bind(source)
    .bind(kb)
    .bind(serde_json::json!({ "endpoint": server.uri() }))
    .execute(&pool)
    .await?;

    // 201 条，全部是新的：每一轮都会重新拿到它们
    let all: Vec<serde_json::Value> = (0..201u32)
        .map(|i| {
            serde_json::json!({
                "id": format!("item-{i}"),
                "title": format!("Item {i}"),
                "content": format!("Body {i}"),
            })
        })
        .collect();
    let served = all.clone();
    Mock::given(method("GET"))
        .respond_with(move |_req: &Request| {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "items": served }))
        })
        .mount(&server)
        .await;

    let dir = std::env::temp_dir().join(format!("utopia-truncation-{source}"));
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let search = Arc::new(utopia_search::SearchIndex::open(&dir.join("search"))?);
    let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());

    let result = async {
        // 第一轮：201 > 200，触顶
        sync_source(&state, source).await?;
        let first = utopia_store::sources::get(&pool, source).await?;
        assert_eq!(
            first.last_sync_status, "failed",
            "触顶的那一轮不能记为 'ok'，否则下界会越过还没读到的条目"
        );
        assert!(
            first
                .last_sync_error
                .unwrap_or_default()
                .contains("stopped with more"),
            "错误要说明为什么停：{:?}",
            first.last_sync_error
        );
        let imported: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE source_id=$1 AND deleted_at IS NULL",
        )
        .bind(source)
        .fetch_one(&pool)
        .await?;
        assert_eq!(imported, 200, "第一轮装得下 200 条，第 201 条得等下一轮");

        // 第二轮：同一批原样回来，前 200 条是 Unchanged 不占预算，第 201 条这次够得着
        sync_source(&state, source).await?;
        let imported: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE source_id=$1 AND deleted_at IS NULL",
        )
        .bind(source)
        .fetch_one(&pool)
        .await?;
        assert_eq!(imported, 201, "第二轮之后 201 条都在库里了");
        let last = utopia_store::sources::get(&pool, source).await?;
        assert_eq!(
            last.last_sync_status, "ok",
            "这一轮走完了，可以记 ok 让游标前进"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id=$1")
        .bind(kb)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    drop(state);
    let _ = std::fs::remove_dir_all(dir);
    result
}
