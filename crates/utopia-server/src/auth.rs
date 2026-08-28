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

/// 外层是否在跑 TLS。反代都会带 `X-Forwarded-Proto`；没有这个头（本地直连、
/// 开发环境）就当明文，Secure 不打，登录照常工作。
///
/// 不需要「信任的代理」名单：伪造这个头只会让攻击者自己的 cookie 变成 Secure，
/// 更严格而不是更宽松，没有攻击价值。`UTOPIA_COOKIE_SECURE=true` 可强制打开，
/// 给那些不发这个头的代理兜底。
pub fn behind_tls(headers: &axum::http::HeaderMap, forced: bool) -> bool {
    forced
        || headers
            .get("x-forwarded-proto")
            .and_then(|v| v.to_str().ok())
            // 经过多层代理时这个头是逗号分隔的链，最左边是最初那一跳
            .and_then(|v| v.split(',').next())
            .is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
}

/// 会话 cookie。`secure` 由 [`behind_tls`] 判定——HTTPS 下打上 Secure，
/// 浏览器就不会再把它经明文链路发出去。
pub fn auth_cookie(token: String, secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

/// 注销用的删除指令。**属性必须和签发时一致**：浏览器按 name + domain + path
/// 匹配才认得出要删哪一条，只给名字的话 path 会退化成当前请求路径
/// （`/api/v1/auth`），和签发时的 `/` 对不上——cookie 留在浏览器里，人以为
/// 自己登出了。
pub fn clear_auth_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, ""))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .build()
}

pub(crate) fn decode_user_id(state: &AppState, token: &str) -> Result<Uuid, AppError> {
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

/// 生成一条 JWT 签名密钥：32 字节 CSPRNG，hex 编码成 64 个字符。
///
/// 长度取 32 字节是因为 HS256 的 HMAC 块就是 32 字节——再长会被先哈希一遍，
/// 并不增加强度。hex 而非 base64：这个值会出现在日志、环境变量和运维的复制粘贴里，
/// 不带 +/= 省去一整类转义问题。
pub fn generate_jwt_secret() -> String {
    use argon2::password_hash::rand_core::RngCore;
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
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

    fn headers_with(proto: Option<&str>) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        if let Some(p) = proto {
            h.insert("x-forwarded-proto", p.parse().unwrap());
        }
        h
    }

    /// Secure 的判定必须只在确认走了 TLS 时为真：判错成 true，明文部署的用户
    /// 登录后浏览器直接丢掉 cookie，症状是「点了登录又回到登录页」，且不报错。
    #[test]
    fn secure_only_when_tls_is_actually_in_front() {
        // 没有代理：本地直连、cargo run —— 不能打 Secure，否则 HTTP 下登不上
        assert!(!behind_tls(&headers_with(None), false));
        assert!(!behind_tls(&headers_with(Some("http")), false));

        assert!(behind_tls(&headers_with(Some("https")), false));
        // 头的大小写由代理决定，不能假设
        assert!(behind_tls(&headers_with(Some("HTTPS")), false));

        // 多层代理时这个头是逗号分隔的链，最左边是最初那一跳——
        // 取错一端会把「用户走 HTTPS 到边缘、边缘走 HTTP 回源」判成明文
        assert!(behind_tls(&headers_with(Some("https, http")), false));
        assert!(!behind_tls(&headers_with(Some("http, https")), false));

        // 配置强制打开：给不发这个头的代理兜底
        assert!(behind_tls(&headers_with(None), true));
    }
}
