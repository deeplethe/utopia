//! 换会话走完整条路由：发现文档与 JWKS 来自本机的假身份提供方，映射与一次性走真库。
use crate::state::AppState;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::get,
    Json, Router,
};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

// 这把钥匙只为测试生成，应用从不使用
#[tokio::test]
async fn exchange_maps_only_linked_active_people_and_spends_each_token_once() -> anyhow::Result<()>
{
    let Some(url) = utopia_store::test_db::url() else {
        return Ok(());
    };
    let pool = sqlx::PgPool::connect(&url).await?;
    utopia_store::db::migrate(&pool).await?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let issuer = format!("http://{}", listener.local_addr()?);
    let discovery = json!({"issuer": issuer, "jwks_uri": format!("{issuer}/jwks")});
    let jwks: Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/oidc_test_only_jwks.json"
    ))?;
    let provider = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || async move { Json(discovery) }),
        )
        .route("/jwks", get(move || async move { Json(jwks) }));
    let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    // 只有这个测试读写这几个变量
    std::env::set_var("UTOPIA_OIDC_ISSUER", &issuer);
    std::env::set_var("UTOPIA_OIDC_CLIENT_ID", "utopia-test");
    std::env::set_var(
        "UTOPIA_OIDC_REDIRECT_URI",
        "http://localhost/api/v1/auth/oidc/callback",
    );
    std::env::set_var("UTOPIA_OIDC_ALLOW_LOOPBACK_HTTP", "1");
    std::env::remove_var("UTOPIA_OIDC_EXCHANGE_AUDIENCES");

    let (org, user) = (Uuid::now_v7(), Uuid::now_v7());
    let subject = format!("person-{user}");
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
            "INSERT INTO organizations(id,name) VALUES('{org}','exchange-test');
             INSERT INTO users(id,org_id,email,password_hash,display_name) VALUES('{user}','{org}','{user}@example.test','unused','Test');
             INSERT INTO oidc_identities(issuer,subject,user_id) VALUES('{issuer}','{subject}','{user}');"
        ))
        .execute(&pool)
        .await?;
        let key = EncodingKey::from_rsa_pem(include_bytes!(
            "../../tests/fixtures/oidc_test_only_key.pem"
        ))?;
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        // `jti` 只为让每次签出来的令牌不同；Utopia 不读它
        let sign_at = |sub: &str, issued_ago: i64, expires_in: i64| {
            let now = chrono::Utc::now().timestamp();
            encode(
                &header,
                &json!({"iss": issuer, "aud": "sibling-app", "sub": sub, "iat": now - issued_ago, "exp": now + expires_in, "jti": Uuid::now_v7()}),
                &key,
            )
        };
        let sign = |sub: &str| sign_at(sub, 0, 300);
        let exchange = |id_token: String| {
            let state = state.clone();
            async move {
                let request = Request::builder()
                    .method("POST")
                    .uri("/api/v1/auth/oidc/exchange")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"id_token": id_token}).to_string()))?;
                let response = super::router(state, &Default::default())
                    .oneshot(request)
                    .await?;
                let status = response.status();
                let body = axum::body::to_bytes(response.into_body(), 65536).await?;
                anyhow::Ok((status, serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null)))
            }
        };

        // 没配受信 audience：端点不存在
        let (status, _) = exchange(sign(&subject)?).await?;
        anyhow::ensure!(status == StatusCode::NOT_FOUND, "closed: {status}");
        std::env::set_var("UTOPIA_OIDC_EXCHANGE_AUDIENCES", "other-app, sibling-app");

        let token = sign(&subject)?;
        let (status, body) = exchange(token.clone()).await?;
        anyhow::ensure!(status == StatusCode::OK, "linked: {status} {body}");
        anyhow::ensure!(crate::auth::decode_user_id(&state, body["token"].as_str().unwrap())? == user);
        let expires_at: chrono::DateTime<chrono::Utc> =
            serde_json::from_value(body["expires_at"].clone())?;
        anyhow::ensure!(expires_at > chrono::Utc::now() + chrono::Duration::days(6));
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE actor_id = $1 AND action = 'auth.oidc_exchange'",
        )
        .bind(user)
        .fetch_one(&pool)
        .await?;
        anyhow::ensure!(audited == 1);

        let (status, body) = exchange(token).await?;
        anyhow::ensure!(body["code"] == "oidc_replayed", "replay: {status} {body}");

        // 刚过 `exp` 的令牌还在校验的宽限里、验得过：换过一次的记录不能先于它失效，
        // 否则这一分钟里它能再换一次
        let late = sign_at(&subject, 100, -30)?;
        let (status, body) = exchange(late.clone()).await?;
        anyhow::ensure!(status == StatusCode::OK, "within leeway: {status} {body}");
        let (status, body) = exchange(late).await?;
        anyhow::ensure!(body["code"] == "oidc_replayed", "replay within leeway: {status} {body}");

        let (status, body) = exchange(sign("nobody-linked-this")?).await?;
        anyhow::ensure!(body["code"] == "oidc_unlinked", "unlinked: {status} {body}");

        sqlx::query("UPDATE users SET deactivated_at = now() WHERE id = $1")
            .bind(user)
            .execute(&pool)
            .await?;
        let (status, body) = exchange(sign(&subject)?).await?;
        anyhow::ensure!(body["code"] == "oidc_unlinked", "deactivated: {status} {body}");
        anyhow::Ok(())
    }
    .await;
    server.abort();
    sqlx::query("DELETE FROM organizations WHERE id=$1")
        .bind(org)
        .execute(&pool)
        .await?;
    result
}
