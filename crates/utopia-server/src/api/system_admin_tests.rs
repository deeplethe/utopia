//! 授予系统管理员走的那条接口：只有系统管理员能调，调了留审计。
use crate::state::AppState;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[tokio::test]
async fn only_a_system_admin_grants_the_flag_and_the_grant_is_audited() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;
    let (org, admin, member) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let dir = tempfile::tempdir()?;
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    };
    let state = AppState::new(
        pool.clone(),
        &cfg,
        Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?),
        "test-only".into(),
    );
    let result = async {
        sqlx::raw_sql(&format!(
            "INSERT INTO organizations(id,name) VALUES('{org}','sysadmin-route-test');
             INSERT INTO users(id,org_id,email,password_hash,display_name,is_admin) VALUES
               ('{admin}','{org}','{admin}@example.test','unused','Admin',TRUE),
               ('{member}','{org}','{member}@example.test','unused','Member',FALSE);"
        ))
        .execute(&pool)
        .await?;
        let grant = |caller: Uuid| {
            let state = state.clone();
            async move {
                let token = crate::auth::issue_token(&state, caller)?;
                let request = Request::builder()
                    .method("PUT")
                    .uri(format!("/api/v1/admin/users/{member}"))
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "is_admin": true }).to_string()))?;
                let response = super::router(state, &Default::default())
                    .oneshot(request)
                    .await?;
                anyhow::Ok(response.status())
            }
        };

        anyhow::ensure!(grant(member).await? == StatusCode::FORBIDDEN);
        anyhow::ensure!(grant(admin).await? == StatusCode::OK);

        let flag: bool = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = $1")
            .bind(member)
            .fetch_one(&pool)
            .await?;
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events
              WHERE actor_id = $1 AND action = 'user.admin_granted' AND target_id = $2",
        )
        .bind(admin)
        .bind(member)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(flag && audited == 1, "flag={flag} audited={audited}");
        anyhow::Ok(())
    }
    .await;
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
