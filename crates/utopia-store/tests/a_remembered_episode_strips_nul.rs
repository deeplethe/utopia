//! `remember` 工具把一句记忆落成 chunk 之前先剥 NUL 字节（#665）。
//!
//! 与 `pipeline::process_document` 共用 `utopia_core::without_nul`：两份
//! 同一函数，所以测一处。记忆这条路径值得独立测一遍——
//! 它不经 `utopia_ingest::parse`，`pipeline` 那边的 NUL 测试覆盖不到。
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

#[tokio::test]
async fn a_nul_in_a_remembered_episode_is_stripped_before_chunks_text() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    for (q, vals) in [
        (
            "INSERT INTO organizations (id, name) VALUES ($1, 'remember-nul-test')",
            vec![org],
        ),
        (
            "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'remember-nul-test')",
            vec![ws, org],
        ),
        (
            "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'remember-nul-test')",
            vec![kb, ws],
        ),
    ] {
        let mut stmt = sqlx::query(q);
        for v in vals {
            stmt = stmt.bind(v);
        }
        stmt.execute(&pool).await?;
    }

    // 三种 NUL：词里、词与词之间、连续 NUL。`serde_json` 把 JSON 的 \u0000 解回 `\0`，
    // 模型 / MCP 写回路径就是这一路。
    let raw = "Acme\0moved\0its\0\0HQ to Shenzhen on 2026-03-15.";
    let (_doc, chunk) =
        utopia_store::memory::append_episode(&pool, kb, raw, chrono::Utc::now()).await?;

    let (text, char_end): (String, i32) =
        sqlx::query_as("SELECT text, char_end FROM chunks WHERE id = $1")
            .bind(chunk)
            .fetch_one(&pool)
            .await?;

    assert!(!text.contains('\0'), "入库后还剩 NUL：{text:?}");
    // `[YYYY-MM-DD HH:MM] ` 前缀按当时的时间生成；断言只验**它之后**的内容：
    // 词被剥完后该照原样拼回去，NUL 整段消失、词与词之间的 NUL 也消失。
    let stripped = text.split_once("] ").map(|(_, rest)| rest).unwrap_or("");
    assert_eq!(
        stripped, "AcmemoveditsHQ to Shenzhen on 2026-03-15.",
        "剥完 NUL 之后词该照原来拼回去"
    );
    assert_eq!(
        char_end as usize,
        text.chars().count(),
        "char_end 用的是剥完的那份，不是入库前的那份"
    );

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    Ok(())
}
