//! 读字服务的密钥跟着供应商走（0065 决定 2）：同一供应商留空保留，换供应商留空清掉。
//! 比较和写在同一条 SQL 里，所以两次保存并发也带不过去。

use sqlx::PgPool;
use utopia_core::secrets;
use utopia_store::settings;
use uuid::Uuid;

#[tokio::test]
async fn a_reader_keeps_its_key_for_its_provider() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    secrets::init(secrets::generate_key());
    let pool = PgPool::connect(&url).await?;
    let (org, ws) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'reader-key-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    let run = async {
        sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'reader-key-test')")
            .bind(ws)
            .bind(org)
            .execute(&pool)
            .await?;
        let save = |provider: &'static str, key: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                settings::upsert_ocr(
                    &pool,
                    ws,
                    Some(provider),
                    Some("https://reader.example"),
                    key,
                    None,
                    Some("m"),
                )
                .await
            }
        };
        let first = settings::upsert_ocr(&pool, ws, None, None, None, None, None).await?;
        assert_eq!(
            first.ocr_provider, "mineru",
            "old clients default on first save"
        );
        let s = save("mineru", Some("mineru-key")).await?;
        assert_eq!(s.ocr_provider, "mineru");
        assert_eq!(s.ocr_api_key.as_deref(), Some("mineru-key"));
        // 同一供应商留空：保留
        assert_eq!(
            save("mineru", None).await?.ocr_api_key.as_deref(),
            Some("mineru-key")
        );
        // 换供应商留空：清掉，方舟拿不到 MinerU 的密钥
        let switched = save("ark", None).await?;
        assert_eq!(
            (
                switched.ocr_provider.as_str(),
                switched.ocr_api_key.as_deref()
            ),
            ("ark", None)
        );
        assert!(!switched.ocr_ready(), "ark without a key is not ready");
        // 换供应商带新密钥：用新的
        let keyed = save("ark", Some("ark-key")).await?;
        assert_eq!(keyed.ocr_api_key.as_deref(), Some("ark-key"));
        assert!(keyed.ocr_ready());
        // 老客户端只改地址：缺席的供应商和模型不能把已配好的方舟清掉。
        let legacy = settings::upsert_ocr(
            &pool,
            ws,
            None,
            Some("https://reader.example/new"),
            None,
            None,
            None,
        )
        .await?;
        assert_eq!(legacy.ocr_provider, "ark");
        assert_eq!(legacy.ocr_model.as_deref(), Some("m"));
        assert_eq!(legacy.ocr_api_key.as_deref(), Some("ark-key"));
        assert!(legacy.ocr_ready());
        // 新客户端显式选供应商仍能清空模型；留空密钥只保留这一家的密钥。
        let cleared = settings::upsert_ocr(
            &pool,
            ws,
            Some("ark"),
            Some("https://reader.example"),
            None,
            None,
            None,
        )
        .await?;
        assert_eq!(cleared.ocr_model, None);
        assert_eq!(cleared.ocr_api_key.as_deref(), Some("ark-key"));
        assert!(!cleared.ocr_ready());
        // 库里存的是封印过的
        let stored: Option<String> =
            sqlx::query_scalar("SELECT ocr_api_key FROM llm_settings WHERE workspace_id = $1")
                .bind(ws)
                .fetch_one(&pool)
                .await?;
        assert!(secrets::is_sealed(stored.as_deref().unwrap_or_default()));
        // 换回 MinerU 一样清
        assert_eq!(save("mineru", None).await?.ocr_api_key, None);
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
