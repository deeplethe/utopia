//! 认证：argon2 密码哈希 + JWT（HttpOnly Cookie，同时接受 Bearer）。
//! Cookie 本身是会话 cookie，过期由 JWT 的 exp 控制（7 天）。

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use utopia_core::models::User;
use utopia_core::AppError;
use uuid::Uuid;

use crate::error::ApiErr;
use crate::state::AppState;

pub const COOKIE_NAME: &str = "utopia_token";
const TOKEN_TTL_DAYS: i64 = 7;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    exp: i64,
}

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Other(anyhow::anyhow!("Password hashing failed: {e}")))
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

pub fn issue_token(state: &AppState, user_id: Uuid) -> Result<String, AppError> {
    let claims = Claims {
        sub: user_id,
        exp: (Utc::now() + chrono::Duration::days(TOKEN_TTL_DAYS)).timestamp(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .map_err(|e| AppError::Other(anyhow::anyhow!("Token issuance failed: {e}")))
}

pub fn auth_cookie(token: String) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .build()
}

fn decode_user_id(state: &AppState, token: &str) -> Result<Uuid, AppError> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError::Unauthorized)?;
    Ok(data.claims.sub)
}

/// 已登录用户提取器：Cookie `utopia_token` 或 `Authorization: Bearer`。
pub struct AuthUser(pub User);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiErr;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(COOKIE_NAME)
            .map(|c| c.value().to_string())
            .or_else(|| {
                parts
                    .headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .map(|v| v.to_string())
            })
            .ok_or(AppError::Unauthorized)?;

        let user_id = decode_user_id(state, &token)?;
        let user = utopia_store::accounts::find_user_by_id(&state.pool, user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(AuthUser(user))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims_in(days: i64) -> Claims {
        Claims {
            sub: Uuid::now_v7(),
            exp: (Utc::now() + chrono::Duration::days(days)).timestamp(),
        }
    }

    /// JWT 校验的护栏：默认配置必须认自家签发的 HS256，且拒绝换密钥与过期。
    /// 加于 jsonwebtoken 9 → 10 升级时（CVE-2026-25537：<10.3.0 的类型混淆可绕过授权）——
    /// 库的默认校验语义是编译器看不见的那部分，回归靠这里兜。
    #[test]
    fn default_validation_accepts_own_token_and_rejects_the_rest() {
        let secret = b"test-secret-not-a-real-key";
        let claims = claims_in(7);
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret),
        )
        .expect("issuing must succeed");

        let decoded = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(secret),
            &Validation::default(),
        )
        .expect("own token must verify under default validation");
        assert_eq!(decoded.claims.sub, claims.sub, "sub must survive the trip");

        assert!(
            decode::<Claims>(
                &token,
                &DecodingKey::from_secret(b"a-different-secret"),
                &Validation::default(),
            )
            .is_err(),
            "a token signed with another key must not verify"
        );

        let expired = encode(
            &Header::default(),
            &claims_in(-1),
            &EncodingKey::from_secret(secret),
        )
        .expect("issuing must succeed");
        assert!(
            decode::<Claims>(
                &expired,
                &DecodingKey::from_secret(secret),
                &Validation::default(),
            )
            .is_err(),
            "exp must be enforced by default"
        );
    }
}
