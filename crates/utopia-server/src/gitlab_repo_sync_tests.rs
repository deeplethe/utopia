//! 用录下的真实 GitLab 响应跑项目文件同步（#1327）：树翻页、大小先 `HEAD` 再取、同一 blob
//! 只收一处、没变不再取、树没翻完时不判「不在来源中」。
use super::sync_gitlab_repo_with_client;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const PROJECT: &str = "/projects/QuintinBotes%2Futopia-gitlab-fixture/repository";

/// 阶段 0：录下的树分两页给。阶段 1：去掉 README.md，每一页都说还有下一页，翻到上限也翻不完。
/// 阶段 2：去掉 README.md，一页给完
fn tree_page(fixture: &Value, stage: u8, uri: &str, page: Option<&str>) -> ResponseTemplate {
    let mut entries = fixture["tree"].as_array().unwrap().clone();
    if stage > 0 {
        entries.retain(|e| e["path"] != "README.md");
    }
    let next = format!(
        r#"<{uri}{PROJECT}/tree?page_token=2&pagination=keyset&recursive=true>; rel="next""#
    );
    match (stage, page) {
        (0, None) => ResponseTemplate::new(200)
            .set_body_json(json!(entries[..10]))
            .insert_header("link", next.as_str()),
        (0, Some(_)) => ResponseTemplate::new(200).set_body_json(json!(entries[10..])),
        (1, _) => ResponseTemplate::new(200)
            .set_body_json(json!(entries))
            .insert_header("link", next.as_str()),
        _ => ResponseTemplate::new(200).set_body_json(json!(entries)),
    }
}

#[tokio::test]
async fn a_gitlab_project_sync_takes_each_changed_file_once() -> anyhow::Result<()> {
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
        // 3000 字节的上限把 3368 字节的 big.md 挡在外面
        sqlx::raw_sql(&format!(
            r#"INSERT INTO organizations(id,name) VALUES('{org}','gitlab-test');
               INSERT INTO workspaces(id,org_id,name) VALUES('{ws}','{org}','gitlab-test');
               INSERT INTO knowledge_bases(id,workspace_id,name) VALUES('{kb}','{ws}','gitlab-test');
               INSERT INTO sources(id,kb_id,kind,name,config)
                    VALUES('{sid}','{kb}','gitlab_repo','fixture',
                           '{{"project":"QuintinBotes/utopia-gitlab-fixture","max_file_bytes":3000}}');"#
        ))
        .execute(&pool)
        .await?;
        let source = utopia_store::sources::get(&pool, sid).await?;
        let dir = tempfile::tempdir()?;
        let cfg = utopia_core::config::AppConfig {
            data_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let search = Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?);
        let state = crate::state::AppState::new(pool.clone(), &cfg, search, "test-only".into());

        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/gitlab_repo.json"))?;
        let server = MockServer::start().await;
        let stage = Arc::new(Mutex::new(0u8));
        let (current, fx, uri) = (stage.clone(), fixture.clone(), server.uri());
        Mock::given(wiremock::matchers::any())
            .respond_with(move |request: &Request| {
                let path = request.url.path();
                let Some(rest) = path.strip_prefix(PROJECT) else {
                    return ResponseTemplate::new(404);
                };
                let query = |key: &str| {
                    request
                        .url
                        .query_pairs()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.into_owned())
                };
                if rest == "/tree" {
                    tree_page(&fx, *current.lock().unwrap(), &uri, query("page_token").as_deref())
                } else if let Some(file) = rest.strip_prefix("/files/") {
                    let file = file.replace("%2F", "/");
                    ResponseTemplate::new(200)
                        .insert_header("x-gitlab-size", fx["sizes"][&file].to_string().as_str())
                } else if rest == "/commits" {
                    ResponseTemplate::new(200).set_body_json(fx["commits"][&query("path").unwrap()].clone())
                } else if let Some(sha) = rest.strip_prefix("/blobs/").and_then(|r| r.strip_suffix("/raw")) {
                    ResponseTemplate::new(200).set_body_string(fx["blobs"][sha].as_str().unwrap())
                } else {
                    ResponseTemplate::new(404)
                }
            })
            .mount(&server)
            .await;
        let (http, uri) = (reqwest::Client::builder().no_proxy().build()?, server.uri());
        let sync = || sync_gitlab_repo_with_client(&state, &source, &http, &uri);
        let doc = |path: &str| {
            let key = format!("gitlab:QuintinBotes/utopia-gitlab-fixture:{path}");
            let pool = pool.clone();
            async move { utopia_store::documents::find_by_external_key(&pool, sid, &key).await }
        };
        let requests = || async { server.received_requests().await.unwrap() };

        let first = sync().await?;
        anyhow::ensure!(first.created == 3, "第一轮建了 {}", first.created);
        for path in ["README.md", "docs/copy-of-guide.md", "docs/nested/deep.mdx"] {
            anyhow::ensure!(doc(path).await?.is_some(), "{path} 该收");
        }
        for path in [
            "docs/guide.md",
            "big.md",
            "notes/empty.md",
            ".gitlab/notes.md",
            "node_modules/pkg/README.md",
            "vendor/lib/README.md",
            "link.md",
            "src/main.rs",
        ] {
            anyhow::ensure!(doc(path).await?.is_none(), "{path} 不该收");
        }
        // 第二次提交的日期，毫秒截掉
        let time = doc("docs/copy-of-guide.md").await?.and_then(|d| d.doc_time);
        anyhow::ensure!(time == Some("2026-10-05T14:30:00Z".parse()?), "{time:?}");
        let sent = requests().await;
        let heads: Vec<_> = sent.iter().filter(|r| r.method == wiremock::http::Method::HEAD).collect();
        // README、copy-of-guide、deep.mdx、big.md；空文件按 SHA 挡掉，不问
        anyhow::ensure!(heads.len() == 4, "只有路径过了筛选的才问大小：{}", heads.len());
        let blobs = sent.iter().filter(|r| r.url.path().ends_with("/raw")).count();
        anyhow::ensure!(blobs == 3, "超限与空文件不该下载：取了 {blobs} 个");
        anyhow::ensure!(
            sent.iter().filter(|r| r.url.path().ends_with("/tree")).count() == 2,
            "树该翻两页"
        );

        let seen = requests().await.len();
        let second = sync().await?;
        let again: Vec<_> = requests().await[seen..]
            .iter()
            .filter(|r| !r.url.path().ends_with("/tree"))
            .map(|r| (r.method.to_string(), r.url.path().to_string()))
            .collect();
        // 没变的一个都不再取；超限的 big.md 不落库，每轮再问一次大小，但不下载
        anyhow::ensure!(
            second.total() == 0
                && again.len() == 1
                && again[0].0 == "HEAD"
                && again[0].1.ends_with("/files/big.md"),
            "没变又取了：{again:?}"
        );
        *stage.lock().unwrap() = 1;
        sync().await?;
        let missing = || async { anyhow::Ok(doc("README.md").await?.unwrap().missing_since) };
        anyhow::ensure!(missing().await?.is_none(), "没翻完的树不该判消失");
        *stage.lock().unwrap() = 2;
        sync().await?;
        anyhow::ensure!(missing().await?.is_some(), "完整的树里没了才判消失");
        anyhow::ensure!(doc("docs/nested/deep.mdx").await?.unwrap().missing_since.is_none());
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
