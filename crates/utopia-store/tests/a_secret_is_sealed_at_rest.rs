//! 凭据静态加密：库里是密文，店里拿到的是明文。
//!
//! 四处凭据——`llm_settings` 的两把 key、`data_sources.conn_string`、来源配置里的
//! 凭据键、api 来源的推送密钥——各守两件事：**写进去是封印的**（裸 SQL 读出来带
//! `enc:v1:` 前缀），**读出来是开封的**（store 函数返回原文）。外加两件：
//! 升级前落库的明文行照常读得出来，补封（`sealing::seal_*`，启动时 `backfill` 扫全部）之后它也封上了；
//! 只传 None 的 upsert 保留旧值、不把它变回明文。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use utopia_core::secrets;
use utopia_store::{datasources, sealing, settings, sources};
use uuid::Uuid;

struct Fx {
    org: Uuid,
    ws: Uuid,
    kb: Uuid,
    user: Uuid,
}

async fn seed(pool: &PgPool) -> anyhow::Result<Fx> {
    let (org, ws, kb, user) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'seal-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'seal-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO users (id, org_id, email, display_name, password_hash)
         VALUES ($1, $2, $1 || '@seal.test', 's', 'x')",
    )
    .bind(user)
    .bind(org)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'seal-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(Fx { org, ws, kb, user })
}

async fn raw(pool: &PgPool, sql: &str, id: Uuid) -> anyhow::Result<Option<String>> {
    Ok(sqlx::query_scalar(sql).bind(id).fetch_one(pool).await?)
}

#[tokio::test]
async fn a_secret_is_sealed_at_rest() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    // 这个测试二进制自己装一把钥匙；服务里由 main 装
    secrets::init(secrets::generate_key());
    let pool = PgPool::connect(&url).await?;
    let f = seed(&pool).await?;
    let mut ds: Option<Uuid> = None;

    let run = async {
        // 1. LLM key：写进去是密文，读出来是原文；只传 None 保留旧值
        let s = settings::upsert(
            &pool,
            f.ws,
            Some("https://llm.example/v1"),
            Some("sk-chat-1"),
            Some("m"),
            None,
            Some("sk-embed-1"),
            None,
            None,
        )
        .await?;
        assert_eq!(s.chat_api_key.as_deref(), Some("sk-chat-1"));
        let stored = raw(
            &pool,
            "SELECT chat_api_key FROM llm_settings WHERE workspace_id = $1",
            f.ws,
        )
        .await?
        .unwrap();
        assert!(secrets::is_sealed(&stored), "sealed in the database");
        assert_ne!(stored, "sk-chat-1");
        let got = settings::get(&pool, f.ws).await?.unwrap();
        assert_eq!(got.chat_api_key.as_deref(), Some("sk-chat-1"));
        assert_eq!(got.embed_api_key.as_deref(), Some("sk-embed-1"));
        settings::upsert(
            &pool,
            f.ws,
            Some("https://llm.example/v1"),
            None,
            Some("m2"),
            None,
            None,
            None,
            None,
        )
        .await?;
        let got = settings::get(&pool, f.ws).await?.unwrap();
        assert_eq!(
            got.chat_api_key.as_deref(),
            Some("sk-chat-1"),
            "None keeps the old key"
        );

        // 2. 连接串（名字全局唯一，带上随机后缀别和残留撞上）
        let ds_name = format!("warehouse-{}", Uuid::now_v7().simple());
        let id = datasources::create(
            &pool,
            &ds_name,
            "postgres",
            "postgres://analyst:pw-secret@db.example:5432/sales",
            f.user,
        )
        .await?;
        ds = Some(id);
        let stored = raw(
            &pool,
            "SELECT conn_string FROM data_sources WHERE id = $1",
            id,
        )
        .await?
        .unwrap();
        assert!(secrets::is_sealed(&stored));
        assert!(!stored.contains("pw-secret"));
        let (engine, conn) = datasources::engine_and_conn(&pool, id).await?;
        assert_eq!(engine, "postgres");
        assert!(conn.contains("pw-secret"));
        let view = datasources::list(&pool)
            .await?
            .into_iter()
            .find(|v| v.id == id)
            .unwrap();
        assert_eq!(
            view.summary, "db.example:5432/sales",
            "the summary opens first"
        );

        // 3. 来源配置里的凭据键 + 推送密钥
        let src = sources::create(
            &pool,
            f.kb,
            "url",
            "feed",
            &serde_json::json!({ "urls": ["https://x.example/a"], "auth_header": "Bearer t0k" }),
            None,
            None,
            None,
        )
        .await?;
        assert_eq!(src.config["auth_header"], "Bearer t0k");
        let stored = raw(
            &pool,
            "SELECT config->>'auth_header' FROM sources WHERE id = $1",
            src.id,
        )
        .await?
        .unwrap();
        assert!(secrets::is_sealed(&stored));
        assert_eq!(
            sources::get(&pool, src.id).await?.config["auth_header"],
            "Bearer t0k"
        );
        sources::set_ingest_token(&pool, src.id, "utp_push_1").await?;
        let stored = raw(
            &pool,
            "SELECT ingest_token FROM sources WHERE id = $1",
            src.id,
        )
        .await?
        .unwrap();
        assert!(secrets::is_sealed(&stored));
        assert_eq!(
            sources::get(&pool, src.id).await?.ingest_token.as_deref(),
            Some("utp_push_1")
        );

        // 4. 旧行：明文照常读，补封之后是密文、读出来还是原文
        sqlx::query(
            "UPDATE llm_settings SET chat_api_key = 'legacy-plain' WHERE workspace_id = $1",
        )
        .bind(f.ws)
        .execute(&pool)
        .await?;
        sqlx::query("UPDATE sources SET ingest_token = 'utp_legacy' WHERE id = $1")
            .bind(src.id)
            .execute(&pool)
            .await?;
        assert_eq!(
            settings::get(&pool, f.ws)
                .await?
                .unwrap()
                .chat_api_key
                .as_deref(),
            Some("legacy-plain")
        );
        // **只补自己的行**：全局 backfill 会拿这把一次性钥匙把共用测试库里别人的
        // 凭据也封上——那些行从此谁也打不开
        let n = sealing::seal_llm_settings(&pool, Some(f.ws)).await?
            + sealing::seal_sources(&pool, Some(src.id)).await?;
        assert_eq!(n, 2, "both legacy rows were sealed");
        let stored = raw(
            &pool,
            "SELECT chat_api_key FROM llm_settings WHERE workspace_id = $1",
            f.ws,
        )
        .await?
        .unwrap();
        assert!(secrets::is_sealed(&stored));
        assert_eq!(
            settings::get(&pool, f.ws)
                .await?
                .unwrap()
                .chat_api_key
                .as_deref(),
            Some("legacy-plain")
        );
        assert_eq!(
            sources::get(&pool, src.id).await?.ingest_token.as_deref(),
            Some("utp_legacy")
        );
        assert_eq!(
            sealing::seal_llm_settings(&pool, Some(f.ws)).await?
                + sealing::seal_sources(&pool, Some(src.id)).await?
                + sealing::seal_data_sources(&pool, Some(id)).await?,
            0,
            "idempotent"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    if let Some(id) = ds {
        let _ = sqlx::query("DELETE FROM data_sources WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(f.kb)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(f.org)
        .execute(&pool)
        .await;
    run
}
