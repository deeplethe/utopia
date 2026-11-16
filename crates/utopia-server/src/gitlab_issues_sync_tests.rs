//! 用录下的真实 GitLab 响应跑工单同步（#1327）：一张工单一篇文档、历史来自 resource events、
//! 读过的不再取详情、合并请求要打开开关才读、没有令牌就不发请求。
use super::sync_gitlab_issues_with_client;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const PROJECT: &str = "/projects/QuintinBotes%2Futopia-gitlab-fixture/";

#[tokio::test]
async fn a_gitlab_ticket_sync_takes_each_ticket_with_its_history_once() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let (org, ws, kb, sid) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    let result = async {
        sqlx::raw_sql(&format!(
            r#"INSERT INTO organizations(id,name) VALUES('{org}','gitlab-issues-test');
               INSERT INTO workspaces(id,org_id,name) VALUES('{ws}','{org}','gitlab-issues-test');
               INSERT INTO knowledge_bases(id,workspace_id,name) VALUES('{kb}','{ws}','gitlab-issues-test');
               INSERT INTO sources(id,kb_id,kind,name,config)
                    VALUES('{sid}','{kb}','gitlab_issues','fixture',
                           '{{"project":"QuintinBotes/utopia-gitlab-fixture","auth_header":"Bearer fixture-token"}}');"#
        ))
        .execute(&pool)
        .await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = std::sync::Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());

        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gitlab_issues.json"))?;
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(move |request: &Request| {
                let Some(rest) = request.url.path().strip_prefix(PROJECT) else {
                    return ResponseTemplate::new(404);
                };
                // 列表是 `issues` / `merge_requests`，详情是 `issues/1/notes` 这样的三段
                let rows = match rest.rsplit_once('/') {
                    None => &fixture[rest],
                    Some((ticket, what)) => &fixture["details"][ticket][what],
                };
                match rows {
                    Value::Array(_) => ResponseTemplate::new(200).set_body_json(rows),
                    _ => ResponseTemplate::new(404),
                }
            })
            .mount(&server)
            .await;
        let (http, uri) = (reqwest::Client::builder().no_proxy().build()?, server.uri());
        let with_config = |config: Value| {
            let pool = pool.clone();
            async move {
                // 走界面保存时走的那条路
                utopia_store::sources::update(&pool, sid, None, Some(&config), None, None).await?;
                anyhow::Ok(utopia_store::sources::get(&pool, sid).await?)
            }
        };
        let doc = |ticket: &str| {
            let key = format!("gitlab:QuintinBotes/utopia-gitlab-fixture{ticket}");
            let pool = pool.clone();
            async move { utopia_store::documents::find_by_external_key(&pool, sid, &key).await }
        };
        let requests = || async { server.received_requests().await.unwrap() };
        let time = |s: &str| s.parse::<DateTime<Utc>>().unwrap();

        let source = utopia_store::sources::get(&pool, sid).await?;
        let first = sync_gitlab_issues_with_client(&state, &source, None, &http, &uri).await?;
        anyhow::ensure!(first.created == 2, "第一轮建了 {}", first.created);
        // checkpoint 留着毫秒：截掉的话，同一秒里更晚的那张下一轮又会被当成没读过
        anyhow::ensure!(first.checkpoint == Some(time("2026-11-14T21:36:10.304Z")));
        let issue = doc("#1").await?.expect("工单 #1 该收");
        // 文档时刻截到秒
        anyhow::ensure!(issue.doc_time == Some(time("2026-11-14T21:36:07Z")));
        let body = String::from_utf8(state.blob.get(&issue.sha256).await?)?;
        anyhow::ensure!(
            body.contains("— milestoned by quintinraptorbotes (v1)")
                && body.contains("— reopened by quintinraptorbotes")
                && body.contains("Still happens on the nightly build, reopening."),
            "正文里该有事件史和评论：{body}"
        );
        anyhow::ensure!(doc("!1").await?.is_none(), "没开开关，合并请求不该收");
        anyhow::ensure!(
            !requests()
                .await
                .iter()
                .any(|r| r.url.path().contains("/merge_requests")),
            "没开开关，合并请求的列表不该读"
        );

        // 第二轮：列表把读过的又给了一遍（`updated_after` 含边界），不写、也不再取详情
        let seen = requests().await.len();
        let second =
            sync_gitlab_issues_with_client(&state, &source, first.checkpoint, &http, &uri).await?;
        anyhow::ensure!(second.total() == 0 && second.checkpoint == first.checkpoint);
        let again = requests().await;
        anyhow::ensure!(
            again[seen..]
                .iter()
                .all(|r| r.url.path().ends_with("/issues")),
            "读过的工单又取了详情"
        );
        anyhow::ensure!(again[seen..].iter().any(|r| r
            .url
            .query_pairs()
            .any(|(k, v)| k == "updated_after" && time(&v) == time("2026-11-14T21:36:10.304Z"))));

        // 只改名字，checkpoint 留着
        let checkpoint = || async {
            anyhow::Ok(utopia_store::sources::sync_checkpoint(&pool, sid).await?)
        };
        sqlx::query("UPDATE sources SET sync_checkpoint=$2 WHERE id=$1")
            .bind(sid)
            .bind(first.checkpoint)
            .execute(&pool)
            .await?;
        utopia_store::sources::update(&pool, sid, Some("renamed"), None, None, None).await?;
        anyhow::ensure!(checkpoint().await? == first.checkpoint, "改名不该清 checkpoint");

        // 打开开关：checkpoint 清掉，不然它之前的合并请求永远读不到。
        // 合并请求进来，各有各的身份，工单没变不记新版本
        let source = with_config(json!({
            "project": "QuintinBotes/utopia-gitlab-fixture",
            "auth_header": "Bearer fixture-token",
            "include_merge_requests": true,
        }))
        .await?;
        let since = checkpoint().await?;
        anyhow::ensure!(since.is_none(), "打开合并请求该清 checkpoint");
        let third = sync_gitlab_issues_with_client(&state, &source, since, &http, &uri).await?;
        anyhow::ensure!(third.created == 2 && third.updated == 0);
        anyhow::ensure!(third.checkpoint == Some(time("2026-11-14T21:36:20.069Z")));
        let merged = doc("!1").await?.expect("合并请求 !1 该收");
        let body = String::from_utf8(state.blob.get(&merged.sha256).await?)?;
        anyhow::ensure!(
            body.contains("Merged on 2026-11-14T21:36:15Z.") && !body.contains("mentioned in commit"),
            "合并时刻该在，系统备注不该在：{body}"
        );
        anyhow::ensure!(doc("!2").await?.is_some() && doc("#2").await?.is_some());

        let sent = requests().await;
        anyhow::ensure!(sent.iter().all(|r| r
            .headers
            .get("authorization")
            .is_some_and(|v| v == "Bearer fixture-token")));

        // 没有令牌：说清楚要令牌，一个请求都不发
        let source = with_config(json!({"project": "QuintinBotes/utopia-gitlab-fixture"})).await?;
        let refused = sync_gitlab_issues_with_client(&state, &source, None, &http, &uri).await;
        anyhow::ensure!(
            refused.is_err_and(|e| e.to_string().contains("auth_header")),
            "没有令牌该报出来"
        );
        anyhow::ensure!(requests().await.len() == sent.len(), "没有令牌还是发了请求");
        // 换项目也清 checkpoint：新项目里更早的工单不然读不到
        sqlx::query("UPDATE sources SET sync_checkpoint=$2 WHERE id=$1")
            .bind(sid)
            .bind(first.checkpoint)
            .execute(&pool)
            .await?;
        with_config(json!({"project": "other/project", "auth_header": "Bearer fixture-token"}))
            .await?;
        anyhow::ensure!(checkpoint().await?.is_none(), "换项目该清 checkpoint");

        // 第一轮还在跑的时候换了配置：checkpoint 本来就空着，清空挡不住那一轮，
        // 所以它提交不了，按旧配置算出来的 checkpoint 不落库
        let run = utopia_store::sources::start_run(&pool, sid).await?;
        with_config(json!({
            "project": "QuintinBotes/utopia-gitlab-fixture",
            "auth_header": "Bearer fixture-token",
        }))
        .await?;
        let stale = utopia_store::sources::finish_timestamp_sync(
            &pool,
            run,
            sid,
            None,
            first.checkpoint,
            2,
            0,
        )
        .await;
        anyhow::ensure!(stale.is_err(), "按旧配置跑的那一轮不该提交");
        anyhow::ensure!(checkpoint().await?.is_none(), "旧配置的 checkpoint 落库了");
        anyhow::Ok(())
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
    result
}
