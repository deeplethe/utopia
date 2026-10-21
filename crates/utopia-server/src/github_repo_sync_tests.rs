//! 用录下的真实 GitHub 响应跑仓库同步：筛选、同一 blob 只收一处、没变不再取、
//! 树截断时不判「不在来源中」。
use super::sync_github_repo_with_client;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use wiremock::{matchers::method, Mock, MockServer, Request, ResponseTemplate};

const PACKS: &str = "crates/utopia-server/packs/README.md";
const TESTDATA: &str = "crates/utopia-server/testdata/README.md";

/// 录下的树再加一个 testdata/README.md 的副本。阶段 1、2 去掉 packs，阶段 1 还标了截断
fn tree(fixture: &Value, stage: u8) -> Value {
    let mut entries = fixture["tree"]["tree"].as_array().unwrap().clone();
    let mut copy = entries
        .iter()
        .find(|e| e["path"] == TESTDATA)
        .unwrap()
        .clone();
    copy["path"] = json!("docs/copy/README.md");
    entries.push(copy);
    if stage > 0 {
        entries.retain(|e| e["path"] != PACKS);
    }
    json!({ "tree": entries, "truncated": stage == 1 })
}

#[tokio::test]
async fn a_repository_sync_takes_each_changed_file_once() -> anyhow::Result<()> {
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
        // 2000 字节的上限把 2920 字节的 SECURITY.md 挡在外面
        sqlx::raw_sql(&format!(
            r#"INSERT INTO organizations(id,name) VALUES('{org}','repo-test');
               INSERT INTO workspaces(id,org_id,name) VALUES('{ws}','{org}','repo-test');
               INSERT INTO knowledge_bases(id,workspace_id,name) VALUES('{kb}','{ws}','repo-test');
               INSERT INTO sources(id,kb_id,kind,name,config)
                    VALUES('{sid}','{kb}','github_repo','fixture',
                           '{{"repo":"deeplethe/utopia","max_file_bytes":2000}}');"#
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
            serde_json::from_str(include_str!("../tests/fixtures/github_repo.json"))?;
        let server = MockServer::start().await;
        let stage = Arc::new(Mutex::new(0u8));
        let (current, fx) = (stage.clone(), fixture.clone());
        Mock::given(method("GET"))
            .respond_with(move |request: &Request| {
                let path = request.url.path();
                let body = if path.ends_with("/git/trees/HEAD") {
                    tree(&fx, *current.lock().unwrap())
                } else if path.ends_with("/commits") {
                    let (_, of) = request
                        .url
                        .query_pairs()
                        .find(|(k, _)| k == "path")
                        .unwrap();
                    fx["commits"][&*of].clone()
                } else if let Some((_, sha)) = path.split_once("/git/blobs/") {
                    return ResponseTemplate::new(200)
                        .set_body_string(fx["blobs"][sha].as_str().unwrap());
                } else {
                    return ResponseTemplate::new(404);
                };
                ResponseTemplate::new(200).set_body_json(body)
            })
            .mount(&server)
            .await;
        let (http, uri) = (reqwest::Client::builder().no_proxy().build()?, server.uri());
        let sync = || sync_github_repo_with_client(&state, &source, &http, &uri);
        let doc = |path: &str| {
            let key = format!("github:deeplethe/utopia:{path}");
            let pool = pool.clone();
            async move { utopia_store::documents::find_by_external_key(&pool, sid, &key).await }
        };

        let first = sync().await?;
        for path in [PACKS, TESTDATA] {
            let date = fixture["commits"][path][0]["commit"]["committer"]["date"].as_str();
            let time = doc(path).await?.and_then(|d| d.doc_time);
            anyhow::ensure!(
                time == date.and_then(|d| d.parse().ok()),
                "{path}：{time:?}"
            );
        }
        for path in [
            "SECURITY.md",
            ".github/PULL_REQUEST_TEMPLATE.md",
            "docs/copy/README.md",
        ] {
            anyhow::ensure!(doc(path).await?.is_none(), "{path} 不该收");
        }
        anyhow::ensure!(first.created == 2, "第一轮建了 {}", first.created);

        let seen = server.received_requests().await.unwrap().len();
        let second = sync().await?;
        let refetched = server.received_requests().await.unwrap()[seen..]
            .iter()
            .filter(|r| !r.url.path().ends_with("/git/trees/HEAD"))
            .count();
        anyhow::ensure!(
            second.total() == 0 && refetched == 0,
            "没变又取了 {refetched} 次"
        );

        *stage.lock().unwrap() = 1;
        sync().await?;
        let missing = || async { anyhow::Ok(doc(PACKS).await?.unwrap().missing_since) };
        anyhow::ensure!(missing().await?.is_none(), "截断的树不该判消失");
        *stage.lock().unwrap() = 2;
        sync().await?;
        anyhow::ensure!(missing().await?.is_some(), "完整的树里没了才判消失");
        anyhow::ensure!(doc(TESTDATA).await?.unwrap().missing_since.is_none());
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
