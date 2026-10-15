//! 单点登录按核实过的 email 找账号，找不到就开一个——打在真库上。

use sqlx::PgPool;
use uuid::Uuid;

/// 一个比库里其它组织都早的组织，开户会落进它的默认工作区
async fn earliest_org(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO organizations (id, name, created_at) VALUES ($1, 'sso-test', '2000-01-01')",
    )
    .bind(org)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name, created_at) VALUES ($1, $2, 'sso-ws', '2000-01-01')")
        .bind(ws)
        .bind(org)
        .execute(pool)
        .await?;
    Ok((org, ws))
}

#[tokio::test]
async fn an_existing_account_is_found_whatever_the_case_of_its_email() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, user) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'sso-test')")
        .bind(org)
        .execute(&pool)
        .await?;
    let email = format!("{}-ann@sso.test", user.simple());

    let run = async {
        sqlx::query(
            "INSERT INTO users (id, org_id, email, display_name, password_hash)
             VALUES ($1, $2, $3, 'Ann', 'x')",
        )
        .bind(user)
        .bind(org)
        .bind(email.to_uppercase())
        .execute(&pool)
        .await?;
        let found = utopia_store::accounts::account_for_sso(&pool, &email, "Ann").await?;
        assert_eq!(found.id, user);
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
async fn an_unknown_email_opens_a_viewer_account_without_a_password() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, ws) = earliest_org(&pool).await?;
    let email = format!("{}-bea@sso.test", Uuid::now_v7().simple());

    let run = async {
        let opened = utopia_store::accounts::account_for_sso(&pool, &email, "Bea").await?;
        let role: String = sqlx::query_scalar(
            "SELECT role FROM memberships WHERE user_id = $1 AND workspace_id = $2",
        )
        .bind(opened.id)
        .bind(ws)
        .fetch_one(&pool)
        .await?;
        let hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
            .bind(opened.id)
            .fetch_one(&pool)
            .await?;
        assert_eq!(
            (
                opened.email.as_str(),
                opened.display_name.as_str(),
                opened.is_admin,
                opened.org_id,
                role.as_str()
            ),
            (email.as_str(), "Bea", false, org, "viewer")
        );
        assert!(
            !hash.starts_with("$argon2"),
            "an SSO account must not have a usable password"
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
