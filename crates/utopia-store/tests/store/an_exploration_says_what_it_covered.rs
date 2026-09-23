//! 一轮映射探索留下的账——打在真库上（#503）。
//!
//! 两条都是从前做不到的：
//!
//! 1. **覆盖率答得出来。** 从前一轮探索只留下 `concept_mappings` 里若干行，
//!    十一条提议对着八十列的宽表与十一条覆盖完一个小库看起来一模一样。
//! 2. **跑挂了的那一轮也有账。** 从前失败与「跑了但一条都没提」在页面上
//!    都是「没有新提议」，而该做的事完全不同。

use sqlx::PgPool;
use utopia_store::exploration_runs as runs;
use uuid::Uuid;

async fn fixture(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws, kb) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'run-test')")
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, 'run-test')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO knowledge_bases (id, workspace_id, name) VALUES ($1, $2, 'run-test')")
        .bind(kb)
        .bind(ws)
        .execute(pool)
        .await?;
    Ok((org, kb))
}

#[tokio::test]
async fn a_run_says_what_it_scanned_and_what_it_dropped() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (_org, kb) = fixture(&pool).await?;

    let run = async {
        let id = runs::start(&pool, kb).await?;
        runs::scanned(
            &pool,
            id,
            &["warehouse".into()],
            8,
            61,
            false,
            24,
        )
        .await?;
        runs::finish(
            &pool,
            id,
            12,
            9,
            serde_json::json!({
                runs::drop_reason::SOURCE: { "n": 3, "example": "model said \"tpch\", mounted: tpch-2026" }
            }),
            &["tpch.lineitem".into(), "tpch.orders".into()],
        )
        .await?;

        let got = runs::recent(&pool, kb, 10).await?;
        assert_eq!(got.len(), 1);
        let r = &got[0];
        assert_eq!((r.tables_scanned, r.columns_scanned, r.cap), (8, 61, 24));
        // **回了 12 条、落库 9 条**：两者之差就是被丢掉的，明细在 dropped。
        // 从前这三个数一个都没有，页面上只看得见落库的那 9 条
        assert_eq!((r.returned, r.accepted), (12, 9));
        assert_eq!(r.dropped[runs::drop_reason::SOURCE]["n"], 3);
        assert!(
            r.dropped[runs::drop_reason::SOURCE]["example"]
                .as_str()
                .is_some_and(|s| s.contains("mounted")),
            "光有计数诊断不动，例子要留住"
        );
        assert_eq!(r.tables_covered.len(), 2, "覆盖率的分子");
        assert!(r.finished_at.is_some());
        assert!(r.error.is_none());
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

#[tokio::test]
async fn a_failed_run_is_not_an_empty_one() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (_org, kb) = fixture(&pool).await?;

    let run = async {
        let id = runs::start(&pool, kb).await?;
        runs::fail(&pool, id, "Chat model not configured").await?;

        let got = runs::recent(&pool, kb, 10).await?;
        assert_eq!(got.len(), 1, "跑挂了的那一轮也该留下一行");
        // 这一行与「跑完了但 accepted = 0」的区别就在这里。两者在页面上
        // 从前都是「没有新提议」，而一个要去配模型，一个要去给列加注释
        assert_eq!(got[0].accepted, 0);
        assert!(got[0]
            .error
            .as_deref()
            .is_some_and(|e| e.contains("Chat model")));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(kb)
        .execute(&pool)
        .await?;
    run
}
