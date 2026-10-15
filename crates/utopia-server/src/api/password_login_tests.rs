//! 关掉密码登录（`UTOPIA_PASSWORD_LOGIN=false`）：登录与注册都拒，登录页据状态藏起表单。
use crate::state::AppState;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn with_password_login_off_login_and_register_are_refused() -> anyhow::Result<()> {
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    let dir = tempfile::tempdir()?;
    let cfg = utopia_core::config::AppConfig {
        data_dir: dir.path().to_string_lossy().into_owned(),
        password_login: false,
        ..Default::default()
    };
    let state = AppState::new(
        pool,
        &cfg,
        Arc::new(utopia_search::SearchIndex::open(
            &dir.path().join("search"),
        )?),
        "test-only".into(),
    );
    let call = |method: &'static str, path: &'static str, body: Value| {
        let state = state.clone();
        async move {
            let request = Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))?;
            let response = super::router(state, &Default::default())
                .oneshot(request)
                .await?;
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 65536).await?;
            anyhow::Ok((
                status,
                serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
            ))
        }
    };
    let credentials =
        json!({"email": "ann@example.test", "password": "long-enough-1", "display_name": "Ann"});

    let (status, body) = call("POST", "/api/v1/auth/login", credentials.clone()).await?;
    assert_eq!(
        (status, body["code"].clone()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!("password_login_disabled")
        )
    );
    let (status, body) = call("POST", "/api/v1/auth/register", credentials).await?;
    assert_eq!(
        (status, body["code"].clone()),
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            json!("password_login_disabled")
        )
    );
    let (_, body) = call("GET", "/api/v1/auth/oidc/status", Value::Null).await?;
    assert_eq!(body["password_login"], json!(false));
    Ok(())
}
