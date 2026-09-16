//! 库的数据描述与约定是两列、两条写入路径——打在真库上（#570）。
//!
//! 探索每跑一次重写描述；人写的约定探索不碰。混在一个字段里，下一次探索就把
//! 人的答案盖了。这条测试守的就是「各写各的、互不覆盖」。

use sqlx::PgPool;
use uuid::Uuid;

async fn base(pool: &PgPool) -> anyhow::Result<Uuid> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'describe-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'describe-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'describe-test')",
    )
    .bind(kb)
    .bind(ws)
    .execute(pool)
    .await?;
    Ok(kb)
}

#[tokio::test]
async fn the_description_and_the_conventions_do_not_overwrite_each_other() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let kb = base(&pool).await?;

    let run = async {
        // 人先写约定
        let no: Option<&str> = None;
        utopia_store::kbs::update(
            &pool,
            kb,
            no,
            no,
            no,
            None,
            no,
            None,
            None,
            None,
            None,
            Some("Amounts are in cents.\nis_test = 1 is excluded."),
        )
        .await?;
        // 探索写描述与问题
        utopia_store::kbs::set_data_description(
            &pool,
            kb,
            "dw.dwd_ord_dtl: one row per order line.",
            &["Which ord_st values count?".into()],
        )
        .await?;
        let got = utopia_store::kbs::get(&pool, kb).await?;
        assert_eq!(
            got.data_conventions.as_deref(),
            Some("Amounts are in cents.\nis_test = 1 is excluded.")
        );
        assert_eq!(
            got.data_description.as_deref(),
            Some("dw.dwd_ord_dtl: one row per order line.")
        );
        assert_eq!(
            got.data_questions,
            serde_json::json!(["Which ord_st values count?"])
        );

        // 探索再跑一次：描述换了，约定原样
        utopia_store::kbs::set_data_description(&pool, kb, "second run", &[]).await?;
        let got = utopia_store::kbs::get(&pool, kb).await?;
        assert_eq!(got.data_description.as_deref(), Some("second run"));
        assert_eq!(got.data_questions, serde_json::json!([]));
        assert!(
            got.data_conventions
                .as_deref()
                .is_some_and(|c| c.contains("is_test")),
            "探索不碰人写的约定"
        );

        // 人改约定（PATCH 只送这一项）：描述原样。送 null 等于不改——与 description 同一约定
        utopia_store::kbs::update(
            &pool,
            kb,
            no,
            no,
            no,
            None,
            no,
            None,
            None,
            None,
            None,
            Some("cents only"),
        )
        .await?;
        let got = utopia_store::kbs::get(&pool, kb).await?;
        assert_eq!(got.data_conventions.as_deref(), Some("cents only"));
        assert_eq!(got.data_description.as_deref(), Some("second run"));
        utopia_store::kbs::update(
            &pool,
            kb,
            Some("renamed"),
            no,
            no,
            None,
            no,
            None,
            None,
            None,
            None,
            no,
        )
        .await?;
        assert_eq!(
            utopia_store::kbs::get(&pool, kb)
                .await?
                .data_conventions
                .as_deref(),
            Some("cents only")
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    // 只删知识库，不删 org——用户是软删除的，测试也不该造一个产品里不存在的动作
    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
