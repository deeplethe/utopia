//! 停用账号的四条硬性质——打在真库上（见 `users.deactivated_at`）。
//!
//! 软删除的风险全在「漏过滤一处」：只要有一条读 `users` 的路径忘了带
//! `deactivated_at IS NULL`，停用就成了摆设，而且**不会有任何报错**。
//! 所以这几条守的不是函数，是**路径**。
//!
//! 反过来，归因那几处必须**照旧查得到**：审计事件、合并日志、改类账本的
//! `actor_id` 指着这个人，而那些是审计材料——人走了不等于那件事没发生。

use sqlx::PgPool;
use uuid::Uuid;

async fn org_with_two_admins(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (org, a, b) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'retire-test')")
        .bind(org)
        .execute(pool)
        .await?;
    for (id, mail) in [(a, "a"), (b, "b")] {
        sqlx::query(
            "INSERT INTO users (id, org_id, email, display_name, password_hash, is_admin)
             VALUES ($1, $2, $3, 'u', 'x', TRUE)",
        )
        .bind(id)
        .bind(org)
        .bind(format!("{}-{mail}@retire.test", id.simple()))
        .execute(pool)
        .await?;
    }
    Ok((org, a, b))
}

#[tokio::test]
async fn a_retired_account_cannot_get_back_in() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, a, b) = org_with_two_admins(&pool).await?;

    let run = async {
        let email: String = sqlx::query_scalar("SELECT email FROM users WHERE id = $1")
            .bind(b)
            .fetch_one(&pool)
            .await?;
        assert!(
            utopia_store::accounts::find_user_by_email(&pool, &email)
                .await?
                .is_some(),
            "停用前该找得到"
        );

        utopia_store::accounts::deactivate_user(&pool, b, a).await?;

        // 登录这条路
        assert!(
            utopia_store::accounts::find_user_by_email(&pool, &email)
                .await?
                .is_none(),
            "停用的账号还能按 email 找到——登录挡不住"
        );
        // **已经签发出去的 token 那条路**。会话校验走 find_user_by_id，
        // 所以停用立即生效，不必等 token 过期
        assert!(
            utopia_store::accounts::find_user_by_id(&pool, b)
                .await?
                .is_none(),
            "停用的账号还能按 id 找到——已签发的 token 仍然有效"
        );

        // 归因照旧：那一行还在，只是打了时间戳
        let still_there: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = $1")
            .bind(b)
            .fetch_one(&pool)
            .await?;
        assert_eq!(still_there, 1, "软删除不该把行删掉——审计要靠它");
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}

#[tokio::test]
async fn the_last_admin_and_oneself_are_protected() -> anyhow::Result<()> {
    let Ok(url) = std::env::var("UTOPIA_DATABASE_URL") else {
        eprintln!("跳过：未设 UTOPIA_DATABASE_URL");
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, a, b) = org_with_two_admins(&pool).await?;

    let run = async {
        assert!(
            utopia_store::accounts::deactivate_user(&pool, a, a)
                .await
                .is_err(),
            "不能停用自己——这个系统没有超级管理员那一层，停完就没人能放回来"
        );

        utopia_store::accounts::deactivate_user(&pool, b, a).await?;
        assert!(
            utopia_store::accounts::deactivate_user(&pool, a, b)
                .await
                .is_err(),
            "最后一个管理员不能停用，否则组织从此没人能管成员"
        );

        // 幂等：重复停用不是错误
        utopia_store::accounts::deactivate_user(&pool, b, a).await?;

        // 恢复之后一切照旧
        utopia_store::accounts::reactivate_user(&pool, b).await?;
        assert!(
            utopia_store::accounts::find_user_by_id(&pool, b)
                .await?
                .is_some(),
            "恢复之后该能登录了"
        );
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
