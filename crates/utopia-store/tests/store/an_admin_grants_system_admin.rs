//! 授予、收回系统管理员——打在真库上。

use sqlx::PgPool;
use uuid::Uuid;

async fn org_with_admin_and_member(pool: &PgPool) -> anyhow::Result<(Uuid, Uuid, Uuid)> {
    let (org, admin, member) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'sysadmin-test')")
        .bind(org)
        .execute(pool)
        .await?;
    for (id, mail, is_admin) in [(admin, "admin", true), (member, "member", false)] {
        sqlx::query(
            "INSERT INTO users (id, org_id, email, display_name, password_hash, is_admin)
             VALUES ($1, $2, $3, 'u', 'x', $4)",
        )
        .bind(id)
        .bind(org)
        .bind(format!("{}-{mail}@sysadmin.test", id.simple()))
        .bind(is_admin)
        .execute(pool)
        .await?;
    }
    Ok((org, admin, member))
}

async fn is_admin(pool: &PgPool, id: Uuid) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

#[tokio::test]
async fn granting_a_member_makes_them_admin() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, admin, member) = org_with_admin_and_member(&pool).await?;

    let run = async {
        utopia_store::accounts::set_system_admin(&pool, member, admin, true).await?;
        assert!(is_admin(&pool, member).await?);
        utopia_store::accounts::set_system_admin(&pool, member, admin, false).await?;
        assert!(!is_admin(&pool, member).await?);
        // A request admitted before the actor's revocation still cannot remove
        // the last administrator. Repeating a non-admin's revocation is harmless.
        let refused = utopia_store::accounts::set_system_admin(&pool, admin, member, false).await;
        assert!(matches!(refused, Err(utopia_core::AppError::Validation(_))));
        utopia_store::accounts::set_system_admin(&pool, member, admin, false).await?;
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
async fn revoking_your_own_flag_is_refused_and_you_stay_admin() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, admin, member) = org_with_admin_and_member(&pool).await?;

    let run = async {
        utopia_store::accounts::set_system_admin(&pool, member, admin, true).await?;
        let refused = utopia_store::accounts::set_system_admin(&pool, admin, admin, false).await;
        assert!(matches!(refused, Err(utopia_core::AppError::Validation(_))));
        assert!(is_admin(&pool, admin).await?);
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
async fn granting_a_deactivated_or_unknown_user_is_not_found() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = PgPool::connect(&url).await?;
    let (org, admin, member) = org_with_admin_and_member(&pool).await?;

    let run = async {
        utopia_store::accounts::deactivate_user(&pool, member, admin).await?;
        for target in [member, Uuid::now_v7()] {
            for value in [true, false] {
                let missing =
                    utopia_store::accounts::set_system_admin(&pool, target, admin, value).await;
                assert!(matches!(missing, Err(utopia_core::AppError::NotFound)));
            }
        }
        assert!(!is_admin(&pool, member).await?);
        Ok::<_, anyhow::Error>(())
    }
    .await;

    sqlx::query("DELETE FROM organizations WHERE id = $1")
        .bind(org)
        .execute(&pool)
        .await?;
    run
}
