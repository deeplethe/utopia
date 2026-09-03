//! 忽略一个未匹配说法之后会发生什么，打在真库上。
//!
//! 这里全是 SQL，`cargo check` 看不见。而这一段的行为**曾经是错的且不可见**：
//! `record_miss` 带着 `WHERE dismissed_at IS NULL`，于是点一次「忽略」既停止呈现、
//! 也停止计数。第一篇里出现一次的说法被忽略掉之后，后面二十篇都在用它，
//! 计数仍停在 1——当初那个判断的依据早就不成立了，而没有任何人看得见。
//!
//! 要钉住的是**抑制与计数分开**：
//!
//! - 忽略之后 `record_miss` 照样累加
//! - `list_misses` 不再返回它（提案与自动扩本体一步没变）
//! - `list_dismissed_misses` 返回它，且带的是**更新后的**计数
//! - `restore_miss` 撤回之后它回到正常列表，计数是连续的而不是从头来过
//!
//! 没有 `UTOPIA_DATABASE_URL` 时跳过而不是失败。自建自拆，绝不碰已有的库。

use sqlx::PgPool;
use uuid::Uuid;

async fn fresh_kb(pool: &PgPool) -> anyhow::Result<Uuid> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'dismissal-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'dismissal-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'dismissal-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(kb)
}

fn count_of(rows: &[utopia_core::models::OntologyMiss], key: &str) -> Option<i32> {
    rows.iter().find(|m| m.key == key).map(|m| m.count)
}

#[tokio::test]
async fn dismissing_stops_the_suggestion_but_not_the_counting() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = fresh_kb(&pool).await?;

    let run = async {
        use utopia_store::ontology as ont;
        // 第一篇文档里出现了一次
        ont::record_miss(&pool, kb, "relation_type", "acquired", Some("A → B")).await?;
        assert_eq!(
            count_of(&ont::list_misses(&pool, kb).await?, "acquired"),
            Some(1)
        );

        // 用户看着「出现 1 次」，判断这是一次性措辞
        ont::dismiss_miss(&pool, kb, "relation_type", "acquired").await?;
        assert_eq!(
            count_of(&ont::list_misses(&pool, kb).await?, "acquired"),
            None,
            "忽略之后不该再进建议列表"
        );

        // 后面两篇也在说它。**关键断言**：计数必须继续走，否则那个判断
        // 依据过期了也没人知道
        ont::record_miss(&pool, kb, "relation_type", "acquired", Some("C → D")).await?;
        ont::record_miss(&pool, kb, "relation_type", "acquired", Some("E → F")).await?;
        assert_eq!(
            count_of(&ont::list_dismissed_misses(&pool, kb).await?, "acquired"),
            Some(3),
            "忽略期间的出现次数必须照记"
        );
        assert_eq!(
            count_of(&ont::list_misses(&pool, kb).await?, "acquired"),
            None,
            "计数在涨，但抑制照旧"
        );

        // 人看见它涨到 3 了，撤回忽略
        ont::restore_miss(&pool, kb, "relation_type", "acquired").await?;
        assert_eq!(
            count_of(&ont::list_misses(&pool, kb).await?, "acquired"),
            Some(3),
            "撤回之后计数是连续的，不是从头来过"
        );
        assert_eq!(
            count_of(&ont::list_dismissed_misses(&pool, kb).await?, "acquired"),
            None
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
