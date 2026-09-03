//! #246：来源列表不带任何凭据。
//!
//! 列表接口给 Viewer 看，此前只剔了 `auth_header`；对象存储、WebDAV、Notion 各自的
//! 密钥原样下发。现在凭据键列在 `SOURCE_SECRET_KEYS` 一张表上，列表 SQL 按表剔。
//! 这里守两件事：
//!
//! 1. **列表里一个凭据键都没有**，每一种连接器都试一遍。
//! 2. **身份标识留着**（bucket、username、account_name），界面要显示得出「这是哪个账号」；
//!    而同步那条路（`sources::get`）拿到的仍是完整配置——凭据只是不出去，不是没了。
//!
//! 直接插表而不走 `sources::create`：`KINDS` 少了五种（#247），那是另一个修复。
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_core::models::SOURCE_SECRET_KEYS;
use utopia_store::sources;
use uuid::Uuid;

async fn seed(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'secret-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'secret-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'secret-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok((org, kb))
}

#[tokio::test]
async fn a_viewer_never_sees_a_credential() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, kb) = seed(&pool).await?;

    let run = async {
        // 每种连接器一条，配置按各自界面真正会写的键
        let fixtures: Vec<(&str, serde_json::Value)> = vec![
            (
                "custom",
                serde_json::json!({ "endpoint": "https://x.test/items", "auth_header": "Bearer c" }),
            ),
            (
                "github_issues",
                serde_json::json!({ "repo": "o/r", "auth_header": "Bearer g" }),
            ),
            (
                "jira_issues",
                serde_json::json!({ "base_url": "https://j.test", "project": "P", "auth_header": "Basic j" }),
            ),
            (
                "s3",
                serde_json::json!({ "bucket": "b", "region": "r", "access_key_id": "AKIA",
                                    "secret_access_key": "s3-secret" }),
            ),
            (
                "azure_blob",
                serde_json::json!({ "bucket": "c", "account_name": "acct", "account_key": "az-key" }),
            ),
            (
                "gcs",
                serde_json::json!({ "bucket": "g", "service_account_key": "{\"private_key\":\"x\"}" }),
            ),
            (
                "webdav",
                serde_json::json!({ "base_url": "https://d.test", "path": "/", "username": "u",
                                    "password": "dav-pass" }),
            ),
            ("notion", serde_json::json!({ "token": "secret_n", "query": "q" })),
        ];
        for (kind, config) in &fixtures {
            sqlx::query(
                "INSERT INTO sources (id, kb_id, kind, name, config) VALUES ($1, $2, $3, $3, $4)",
            )
            .bind(Uuid::now_v7())
            .bind(kb)
            .bind(kind)
            .bind(config)
            .execute(&pool)
            .await?;
        }

        let listed = sources::list(&pool, kb).await?;
        assert_eq!(listed.len(), fixtures.len());
        for s in &listed {
            let obj = s.config.as_object().expect("config is an object");
            for key in SOURCE_SECRET_KEYS {
                assert!(
                    !obj.contains_key(*key),
                    "{}: `{key}` must not reach a viewer, got {:?}",
                    s.kind,
                    obj
                );
            }
        }
        // 身份标识留着
        let by_kind = |k: &str| {
            listed
                .iter()
                .find(|s| s.kind == k)
                .map(|s| s.config.clone())
                .expect("listed")
        };
        assert_eq!(by_kind("s3")["bucket"], "b");
        assert_eq!(by_kind("s3")["access_key_id"], "AKIA");
        assert_eq!(by_kind("azure_blob")["account_name"], "acct");
        assert_eq!(by_kind("webdav")["username"], "u");
        assert_eq!(by_kind("custom")["endpoint"], "https://x.test/items");
        assert_eq!(by_kind("notion")["query"], "q");

        // 同步那条路仍拿完整配置：凭据只是不出去，不是没了
        for s in &listed {
            let full = sources::get(&pool, s.id).await?;
            let want = &fixtures.iter().find(|(k, _)| *k == s.kind).unwrap().1;
            assert_eq!(&full.config, want, "{}: sync still sees the credentials", s.kind);
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;

    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await;
    run
}
