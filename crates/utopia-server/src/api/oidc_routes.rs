//! 单点登录：窄范围的 OIDC 授权码流程（PKCE、一次性 state、nonce、
//! issuer/audience 与 RS256 校验）。账号必须由管理员显式开通——
//! 见 `oidc_identities`，绝不按身份提供方的 email 声明自动关联。

use crate::{
    auth::{self, AuthUser},
    error::ApiResult,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Redirect,
    Json,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use utopia_core::AppError;
use uuid::Uuid;

const FLOW_COOKIE: &str = "utopia_oidc_state";

struct Config {
    issuer: String,
    client_id: String,
    secret: Option<String>,
    redirect: String,
}

fn config() -> Result<Config, AppError> {
    let get = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| AppError::Validation("SSO is not configured".into()))
    };
    let c = Config {
        issuer: get("UTOPIA_OIDC_ISSUER")?,
        client_id: get("UTOPIA_OIDC_CLIENT_ID")?,
        secret: std::env::var("UTOPIA_OIDC_CLIENT_SECRET")
            .ok()
            .filter(|s| !s.is_empty()),
        redirect: get("UTOPIA_OIDC_REDIRECT_URI")?,
    };
    secure_url(&c.issuer, false)?;
    secure_url(&c.redirect, true)?;
    Ok(c)
}

fn secure_url(s: &str, loopback: bool) -> Result<reqwest::Url, AppError> {
    let url = reqwest::Url::parse(s).map_err(|_| AppError::Validation("Invalid SSO URL".into()))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https"
            || (loopback
                && url.scheme() == "http"
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))))
    {
        return Err(AppError::Validation(
            "SSO endpoints require HTTPS (localhost callback may use HTTP)".into(),
        ));
    }
    Ok(url)
}

fn client() -> Result<reqwest::Client, AppError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::Other(e.into()))
}

async fn metadata(c: &Config) -> Result<Value, AppError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        c.issuer.trim_end_matches('/')
    );
    let value: Value = client()?
        .get(url)
        .send()
        .await
        .map_err(|_| AppError::Validation("Identity provider is unreachable".into()))?
        .error_for_status()
        .map_err(|_| AppError::Validation("Identity discovery failed".into()))?
        .json()
        .await
        .map_err(|_| AppError::Validation("Invalid identity discovery response".into()))?;
    if value["issuer"] != c.issuer {
        return Err(AppError::Unauthorized);
    };
    Ok(value)
}

fn endpoint(meta: &Value, key: &str) -> Result<reqwest::Url, AppError> {
    secure_url(
        meta[key]
            .as_str()
            .ok_or_else(|| AppError::Validation("Identity provider endpoint is missing".into()))?,
        false,
    )
}

fn flow_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((FLOW_COOKIE, value))
        .path("/api/v1/auth/oidc")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(
            std::time::Duration::from_secs(600)
                .try_into()
                .expect("ten minutes fits cookie duration"),
        )
        .build()
}

pub async fn status() -> Json<Value> {
    Json(json!({"enabled": config().is_ok()}))
}

pub async fn start(
    State(s): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> ApiResult<(CookieJar, Redirect)> {
    let c = config()?;
    let meta = metadata(&c).await?;
    let mut url = endpoint(&meta, "authorization_endpoint")?;
    let state = auth::generate_jwt_secret();
    let nonce = auth::generate_jwt_secret();
    let verifier = auth::generate_jwt_secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    sqlx::query("DELETE FROM oidc_flows WHERE expires_at < now()")
        .execute(&s.pool)
        .await?;
    sqlx::query(
        "INSERT INTO oidc_flows (state, nonce, verifier, expires_at)
         VALUES ($1, $2, $3, now() + interval '10 minutes')",
    )
    .bind(&state)
    .bind(&nonce)
    .bind(&verifier)
    .execute(&s.pool)
    .await?;
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("scope", "openid profile email"),
        ("client_id", &c.client_id),
        ("redirect_uri", &c.redirect),
        ("state", &state),
        ("nonce", &nonce),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ]);
    let secure = auth::behind_tls(&headers, s.cookie_secure);
    let jar = jar.add(flow_cookie(state, secure));
    Ok((jar, Redirect::to(url.as_str())))
}

#[derive(Deserialize)]
pub struct Callback {
    code: String,
    state: String,
}

#[derive(Clone, Deserialize)]
struct Claims {
    sub: String,
    nonce: String,
    iat: i64,
    #[serde(default)]
    azp: Option<String>,
    aud: Value,
}

fn verify_token(
    c: &Config,
    id_token: &str,
    keys: &jsonwebtoken::jwk::JwkSet,
    nonce: &str,
) -> Result<Claims, AppError> {
    let header = decode_header(id_token).map_err(|_| AppError::Unauthorized)?;
    if header.alg != Algorithm::RS256 {
        return Err(AppError::Unauthorized);
    }
    let key = keys
        .find(header.kid.as_deref().ok_or(AppError::Unauthorized)?)
        .ok_or(AppError::Unauthorized)?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.validate_nbf = true;
    validation.set_issuer(&[&c.issuer]);
    validation.set_audience(&[&c.client_id]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
    let claims = decode::<Claims>(
        id_token,
        &DecodingKey::from_jwk(key).map_err(|_| AppError::Unauthorized)?,
        &validation,
    )
    .map_err(|_| AppError::Unauthorized)?
    .claims;
    if claims.nonce != nonce
        || claims.sub.is_empty()
        || claims.iat > chrono::Utc::now().timestamp() + 60
        || claims.azp.as_ref().is_some_and(|a| a != &c.client_id)
        || (claims.aud.as_array().is_some_and(|a| a.len() > 1)
            && claims.azp.as_deref() != Some(c.client_id.as_str()))
    {
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}

pub async fn callback(
    State(s): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<Callback>,
) -> ApiResult<(CookieJar, Redirect)> {
    let c = config()?;
    let cookie = jar.get(FLOW_COOKIE).ok_or(AppError::Unauthorized)?;
    use subtle::ConstantTimeEq;
    if !bool::from(cookie.value().as_bytes().ct_eq(q.state.as_bytes())) {
        return Err(AppError::Unauthorized.into());
    }
    let flow: Option<(String, String)> = sqlx::query_as(
        "DELETE FROM oidc_flows WHERE state = $1 AND expires_at > now()
         RETURNING nonce, verifier",
    )
    .bind(&q.state)
    .fetch_optional(&s.pool)
    .await?;
    let (nonce, verifier) = flow.ok_or(AppError::Unauthorized)?;
    let meta = metadata(&c).await?;
    let http = client()?;
    let mut req = http.post(endpoint(&meta, "token_endpoint")?).form(&[
        ("grant_type", "authorization_code"),
        ("code", q.code.as_str()),
        ("redirect_uri", c.redirect.as_str()),
        ("client_id", c.client_id.as_str()),
        ("code_verifier", verifier.as_str()),
    ]);
    if let Some(secret) = &c.secret {
        req = req.basic_auth(&c.client_id, Some(secret));
    }
    let tokens: Value = req
        .send()
        .await
        .map_err(|_| AppError::Unauthorized)?
        .error_for_status()
        .map_err(|_| AppError::Unauthorized)?
        .json()
        .await
        .map_err(|_| AppError::Unauthorized)?;
    let id_token = tokens["id_token"].as_str().ok_or(AppError::Unauthorized)?;
    let header = decode_header(id_token).map_err(|_| AppError::Unauthorized)?;
    if header.alg != Algorithm::RS256 {
        return Err(AppError::Unauthorized.into());
    }
    let keys: jsonwebtoken::jwk::JwkSet = http
        .get(endpoint(&meta, "jwks_uri")?)
        .send()
        .await
        .map_err(|_| AppError::Unauthorized)?
        .error_for_status()
        .map_err(|_| AppError::Unauthorized)?
        .json()
        .await
        .map_err(|_| AppError::Unauthorized)?;
    let claims = verify_token(&c, id_token, &keys, &nonce)?;
    let user_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT i.user_id FROM oidc_identities i JOIN users u ON u.id = i.user_id
          WHERE i.issuer = $1 AND i.subject = $2 AND u.deactivated_at IS NULL",
    )
    .bind(&c.issuer)
    .bind(&claims.sub)
    .fetch_optional(&s.pool)
    .await?;
    let user_id = user_id.ok_or_else(|| {
        AppError::Validation("This SSO identity has not been linked by an administrator".into())
    })?;
    let secure = auth::behind_tls(&headers, s.cookie_secure);
    let jar = jar
        .remove(flow_cookie(String::new(), secure))
        .add(auth::auth_cookie(auth::issue_token(&s, user_id)?, secure));
    let _ = utopia_store::audit::record(
        &s.pool,
        None,
        user_id,
        "auth.oidc_login",
        "user",
        Some(user_id),
        json!({"issuer": c.issuer}),
    )
    .await;
    Ok((jar, Redirect::to("/")))
}

#[derive(Deserialize)]
pub struct Binding {
    user_id: Uuid,
    subject: String,
}

pub async fn identities(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
) -> ApiResult<Json<Value>> {
    if !u.is_admin {
        return Err(AppError::Forbidden.into());
    };
    let c = config()?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('user_id', i.user_id, 'subject', i.subject, 'email', u.email)
           FROM oidc_identities i JOIN users u ON u.id = i.user_id
          WHERE i.issuer = $1 ORDER BY u.email",
    )
    .bind(&c.issuer)
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(
        json!({"issuer": c.issuer, "client_id": c.client_id, "redirect_uri": c.redirect, "identities": rows}),
    ))
}

pub async fn bind(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Json(b): Json<Binding>,
) -> ApiResult<Json<Value>> {
    if !u.is_admin {
        return Err(AppError::Forbidden.into());
    };
    let c = config()?;
    if b.subject.trim().is_empty() || b.subject.len() > 512 {
        return Err(AppError::Validation(
            "Provide the exact identity provider subject (sub)".into(),
        )
        .into());
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND org_id = $2 AND deactivated_at IS NULL)",
    )
    .bind(b.user_id)
    .bind(u.org_id)
    .fetch_one(&s.pool)
    .await?;
    if !exists {
        return Err(AppError::NotFound.into());
    }
    sqlx::query("INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, $2, $3)")
        .bind(&c.issuer)
        .bind(&b.subject)
        .bind(b.user_id)
        .execute(&s.pool)
        .await?;
    let _ = utopia_store::audit::record(
        &s.pool,
        None,
        u.id,
        "auth.oidc_link",
        "user",
        Some(b.user_id),
        json!({"issuer": c.issuer, "subject": b.subject}),
    )
    .await;
    Ok(Json(json!({"ok": true})))
}

pub async fn unbind(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    if !u.is_admin {
        return Err(AppError::Forbidden.into());
    };
    let c = config()?;
    sqlx::query(
        "DELETE FROM oidc_identities WHERE issuer = $1 AND user_id = $2
          AND user_id IN (SELECT id FROM users WHERE org_id = $3)",
    )
    .bind(&c.issuer)
    .bind(id)
    .bind(u.org_id)
    .execute(&s.pool)
    .await?;
    let _ = utopia_store::audit::record(
        &s.pool,
        None,
        u.id,
        "auth.oidc_unlink",
        "user",
        Some(id),
        json!({"issuer": c.issuer}),
    )
    .await;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_local_callbacks_can_use_http() {
        assert!(super::secure_url("http://localhost:1516/callback", true).is_ok());
        assert!(super::secure_url("http://example.org/callback", true).is_err());
        assert!(super::secure_url("http://localhost:1516", false).is_err());
        assert!(super::secure_url("https://user:password@example.org", false).is_err());
    }

    // This key is generated solely for these tests and is never used by the app.
    #[test]
    fn verifies_signature_issuer_audience_nonce_and_expiry() {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use serde_json::json;
        let c = super::Config {
            issuer: "https://id.example.test".into(),
            client_id: "utopia-test".into(),
            secret: None,
            redirect: "https://utopia.example.test/api/v1/auth/oidc/callback".into(),
        };
        let key = EncodingKey::from_rsa_pem(include_bytes!(
            "../../tests/fixtures/oidc_test_only_key.pem"
        ))
        .unwrap();
        let keys = serde_json::from_str(include_str!(
            "../../tests/fixtures/oidc_test_only_jwks.json"
        ))
        .unwrap();
        let now = chrono::Utc::now().timestamp();
        let valid = json!({"iss":c.issuer,"aud":c.client_id,"sub":"person-123","nonce":"expected","iat":now,"exp":now+300});
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("fixture".into());
        let token = encode(&header, &valid, &key).unwrap();
        assert!(super::verify_token(&c, &token, &keys, "expected").is_ok());
        assert!(super::verify_token(&c, &token, &keys, "wrong").is_err());
        for (field, value) in [
            ("iss", json!("https://other.example.test")),
            ("aud", json!("other-client")),
            ("exp", json!(now - 3600)),
            ("iat", json!(now + 3600)),
            ("azp", json!("other-client")),
            ("sub", json!("")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            let token = encode(&header, &invalid, &key).unwrap();
            assert!(
                super::verify_token(&c, &token, &keys, "expected").is_err(),
                "accepted invalid {field}"
            );
        }
        let fake = encode(
            &Header::default(),
            &valid,
            &EncodingKey::from_secret(b"not-an-rsa-key"),
        )
        .unwrap();
        assert!(super::verify_token(&c, &fake, &keys, "expected").is_err());
        header.kid = Some("unknown-key".into());
        assert!(super::verify_token(
            &c,
            &encode(&header, &valid, &key).unwrap(),
            &keys,
            "expected"
        )
        .is_err());
    }
}
