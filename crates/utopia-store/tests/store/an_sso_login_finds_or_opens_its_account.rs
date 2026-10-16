//! 单点登录按核实过的 email 找账号，找不到就开一个——打在真库上。

use sqlx::PgPool;
use uuid::Uuid;

const ISSUER: &str = "https://id.sso.test";

/// 一个比库里其它组织都早的组织，开户会落进它的默认工作区
/// 用「最早的组织」的测试一个一个跑，组织的时间也比库里现有的都早一天：
/// 并行时谁更早说不准，中断的旧测试留下的组织也会抢走「最早」
static EARLIEST: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn earliest_org(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid)> {
    let (org, ws) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "INSERT INTO organizations (id, name, created_at)
         SELECT $1, 'sso-test', LEAST(min(created_at), '2000-01-01') - interval '1 day' FROM organizations",
    )
    .bind(org)
    .execute(pool)
    .await?;
    sqlx::query("INSERT INTO workspaces (id, org_id, name, created_at) SELECT $1, $2, 'sso-ws', created_at FROM organizations WHERE id = $2")
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
        let found = utopia_store::accounts::account_for_sso(
            &pool,
            ISSUER,
            &format!("sub-{user}"),
            &email,
            "Ann",
        )
        .await?;
        assert_eq!(found.map(|u| u.id), Some(user));
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
    let _serial = EARLIEST.lock().await;
    let (org, ws) = earliest_org(&pool).await?;
    let email = format!("{}-bea@sso.test", Uuid::now_v7().simple());

    let run = async {
        let subject = format!("sub-{email}");
        let opened =
            utopia_store::accounts::account_for_sso(&pool, ISSUER, &subject, &email, "Bea")
                .await?
                .ok_or_else(|| anyhow::anyhow!("no account opened"))?;
        let linked: Uuid = sqlx::query_scalar(
            "SELECT user_id FROM oidc_identities WHERE issuer = $1 AND subject = $2",
        )
        .bind(ISSUER)
        .bind(&subject)
        .fetch_one(&pool)
        .await?;
        assert_eq!(linked, opened.id);
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

/// 停用的账号还占着这个 email：不开新号——否则停用对允许名单里的人形同虚设
#[tokio::test]
async fn an_email_held_by_a_deactivated_account_opens_no_account() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let _serial = EARLIEST.lock().await;
    let (org, _) = earliest_org(&pool).await?;
    let email = format!("{}-cid@sso.test", Uuid::now_v7().simple());

    let run = async {
        sqlx::query(
            "INSERT INTO users (id, org_id, email, display_name, password_hash, deactivated_at)
             VALUES ($1, $2, $3, 'Cid', 'x', now())",
        )
        .bind(Uuid::now_v7())
        .bind(org)
        .bind(email.to_uppercase())
        .execute(&pool)
        .await?;
        let opened = utopia_store::accounts::account_for_sso(
            &pool,
            ISSUER,
            &format!("sub-{email}"),
            &email,
            "Cid",
        )
        .await?;
        let accounts: i64 =
            sqlx::query_scalar("SELECT count(*) FROM users WHERE lower(email) = $1")
                .bind(&email)
                .fetch_one(&pool)
                .await?;
        assert_eq!((opened.map(|u| u.id), accounts), (None, 1));
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}

/// subject 已绑在别人身上：开户与绑定同一个事务，撞车就什么都不留
#[tokio::test]
async fn a_subject_linked_to_another_account_leaves_no_new_account() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let _serial = EARLIEST.lock().await;
    let (org, _) = earliest_org(&pool).await?;
    let (holder, subject) = (Uuid::now_v7(), format!("sub-{}", Uuid::now_v7()));
    let email = format!("{}-dee@sso.test", Uuid::now_v7().simple());

    let run = async {
        sqlx::query(
            "INSERT INTO users (id, org_id, email, display_name, password_hash)
             VALUES ($1, $2, $3, 'Holder', 'x')",
        )
        .bind(holder)
        .bind(org)
        .bind(format!("{holder}@sso.test"))
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, $2, $3)")
            .bind(ISSUER)
            .bind(&subject)
            .bind(holder)
            .execute(&pool)
            .await?;
        let refused =
            utopia_store::accounts::account_for_sso(&pool, ISSUER, &subject, &email, "Dee").await;
        let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email = $1")
            .bind(&email)
            .fetch_one(&pool)
            .await?;
        assert!(
            matches!(
                &refused,
                Err(utopia_core::AppError::CodedConflict { code: "taken", .. })
            ),
            "{:?}",
            refused.map(|u| u.map(|u| u.id))
        );
        assert_eq!(accounts, 0);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
