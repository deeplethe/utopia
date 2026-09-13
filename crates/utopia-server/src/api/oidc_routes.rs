//! 单点登录：窄范围的 OIDC 授权码流程（PKCE、一次性 state、nonce、
//! issuer/audience 与 RS256 校验）。
//!
//! **身份提供方的一个 subject 由账号本人绑定。** 人先用密码登录，再在账户页走一遍
//! SSO；回调核对「完成这次流程的就是发起绑定的那个已登录的人」才写 `oidc_identities`。
//! 从前是管理员把任意 subject 填给任意账号——那等于给了管理员一条冒充任何人
//! （包括别的管理员）登录的路：绑定不经本人、本人看不见、改密码也断不掉，之后的
//! 操作在台账上记成被冒充的人（0014：身份来自本人）。管理员现在只能看与解绑。
//!
//! 绝不按身份提供方的 email 这类可变声明自动关联，首次登录也绝不自动开账号。

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
use jsonwebtoken::{decode, decode_header, jwk::JwkSet, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use utopia_core::AppError;
use uuid::Uuid;

const FLOW_COOKIE: &str = "utopia_oidc_state";
/// HTTPS 下用 `__Host-` 前缀：浏览器只接受 Secure、Path=/、不带 Domain 的这个名字，
/// 同站点的兄弟子域名或明文链路就种不进来。否则攻击者先种一个自己的 state，
/// 再把受害者带到自己的回调地址上，受害者就登进了攻击者的账号（登录 CSRF）
const FLOW_COOKIE_HOST: &str = "__Host-utopia_oidc_state";
const FLOW_TTL_SECS: u64 = 600;
/// 同时在途的登录流程上限。`start` 不需要登录，每一次都会落一行、问一次身份提供方；
/// 没有上限的话，一个脚本就能把表撑大、把身份提供方对这个部署的限流打满
const MAX_PENDING_FLOWS: i64 = 1000;
/// 发现文档、JWKS、令牌响应的体积上限。它们来自身份提供方，但一个出了问题的
/// 身份提供方不该能把服务器的内存吃掉
const DOC_LIMIT: usize = 256 * 1024;
const TOKEN_LIMIT: usize = 64 * 1024;
/// 发现文档与 JWKS 的缓存时长。轮换密钥时 `kid` 找不到会强制刷新一次
const CACHE_TTL: Duration = Duration::from_secs(600);

struct Config {
    issuer: String,
    client_id: String,
    secret: Option<String>,
    redirect: String,
    /// 本地开发：允许 issuer 与各端点走回环地址上的明文 HTTP。只认 localhost /
    /// 127.0.0.1 / ::1，公网地址照旧只能 HTTPS
    loopback_http: bool,
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
        loopback_http: std::env::var("UTOPIA_OIDC_ALLOW_LOOPBACK_HTTP")
            .is_ok_and(|v| matches!(v.trim(), "1" | "true")),
    };
    secure_url(&c.issuer, c.loopback_http)?;
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
                && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    {
        return Err(AppError::Validation(
            "SSO endpoints require HTTPS (localhost callback may use HTTP)".into(),
        ));
    }
    Ok(url)
}

/// 一次拒绝：`code` 回给界面（登录页 / 账户页按它措辞），`reason` 只进日志与台账
struct Refusal {
    code: &'static str,
    reason: String,
}

fn refuse(code: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal {
        code,
        reason: reason.into(),
    }
}

impl From<sqlx::Error> for Refusal {
    fn from(e: sqlx::Error) -> Self {
        refuse("unavailable", format!("database: {e}"))
    }
}

fn client() -> Result<reqwest::Client, Refusal> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| refuse("unavailable", format!("http client: {e}")))
}

/// 读一个限了体积的 JSON 响应。先看 Content-Length，再边读边数——
/// 不报长度的响应一样截得住
async fn read_json(mut resp: reqwest::Response, limit: usize) -> Result<Value, String> {
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    if resp.content_length().is_some_and(|n| n > limit as u64) {
        return Err(format!("response larger than {limit} bytes"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err(format!("response larger than {limit} bytes"));
        }
    }
    serde_json::from_slice(&body).map_err(|e| format!("not JSON: {e}"))
}

type Cache = tokio::sync::Mutex<HashMap<String, (Instant, Value)>>;

fn cache() -> &'static Cache {
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// 发现文档与 JWKS 按地址缓存。`start` 不需要登录，不缓存的话每一次匿名请求
/// 都会让服务器去问一次身份提供方
async fn cached_json(url: &reqwest::Url, force: bool) -> Result<Value, Refusal> {
    let key = url.to_string();
    if !force {
        if let Some((at, v)) = cache().lock().await.get(&key) {
            if at.elapsed() < CACHE_TTL {
                return Ok(v.clone());
            }
        }
    }
    let resp = client()?
        .get(url.clone())
        .send()
        .await
        .map_err(|e| refuse("unavailable", format!("fetch {key}: {e}")))?;
    let v = read_json(resp, DOC_LIMIT)
        .await
        .map_err(|e| refuse("unavailable", format!("fetch {key}: {e}")))?;
    cache()
        .lock()
        .await
        .insert(key, (Instant::now(), v.clone()));
    Ok(v)
}

async fn metadata(c: &Config) -> Result<Value, Refusal> {
    let url = secure_url(
        &format!(
            "{}/.well-known/openid-configuration",
            c.issuer.trim_end_matches('/')
        ),
        c.loopback_http,
    )
    .map_err(|_| refuse("unavailable", "issuer URL"))?;
    let meta = cached_json(&url, false).await?;
    if meta["issuer"] != c.issuer {
        return Err(refuse(
            "unavailable",
            "discovery document names a different issuer",
        ));
    }
    Ok(meta)
}

fn endpoint(c: &Config, meta: &Value, key: &str) -> Result<reqwest::Url, Refusal> {
    let raw = meta[key]
        .as_str()
        .ok_or_else(|| refuse("unavailable", format!("discovery has no {key}")))?;
    secure_url(raw, c.loopback_http)
        .map_err(|_| refuse("unavailable", format!("{key} is not HTTPS")))
}

fn flow_cookie(value: String, secure: bool) -> Cookie<'static> {
    let (name, path) = if secure {
        (FLOW_COOKIE_HOST, "/")
    } else {
        (FLOW_COOKIE, "/api/v1/auth/oidc")
    };
    Cookie::build((name, value))
        .path(path)
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .max_age(
            Duration::from_secs(FLOW_TTL_SECS)
                .try_into()
                .expect("ten minutes fits cookie duration"),
        )
        .build()
}

/// 当前请求带着的已登录会话（cookie），停用的账号不算
async fn session_user(s: &AppState, jar: &CookieJar) -> Option<Uuid> {
    let token = jar.get(auth::COOKIE_NAME)?.value().to_string();
    let id = auth::decode_user_id(s, &token).ok()?;
    utopia_store::accounts::find_user_by_id(&s.pool, id)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
}

/// RFC 6749 §2.3.1：client id 与 secret 先按表单编码，再放进 Basic 头。
/// 不编码的话含 `:`、`%`、`+` 的 secret 在严格的身份提供方那里认证失败
fn form_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub async fn status() -> Json<Value> {
    Json(json!({"enabled": config().is_ok()}))
}

#[derive(Deserialize)]
pub struct StartQuery {
    #[serde(default)]
    link: Option<String>,
}

/// 浏览器导航过来的：出错一律带着代码跳回页面，不给人看一段 JSON
fn back_to(page: &str, code: &str) -> Redirect {
    Redirect::to(&format!("{page}?sso_error={code}"))
}

pub async fn start(
    State(s): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<StartQuery>,
) -> (CookieJar, Redirect) {
    let linking = q.link.as_deref().is_some_and(|v| matches!(v, "1" | "true"));
    let page = if linking { "/account" } else { "/login" };
    match begin(&s, &jar, linking).await {
        Ok((state, url)) => {
            let secure = auth::behind_tls(&headers, s.cookie_secure);
            (
                jar.add(flow_cookie(state, secure)),
                Redirect::to(url.as_str()),
            )
        }
        Err(r) => {
            tracing::warn!(code = r.code, reason = %r.reason, "单点登录没能开始");
            (jar, back_to(page, r.code))
        }
    }
}

async fn begin(
    s: &AppState,
    jar: &CookieJar,
    linking: bool,
) -> Result<(String, reqwest::Url), Refusal> {
    let c = config().map_err(|_| refuse("unavailable", "not configured"))?;
    // 绑定只替已登录的本人发起：流程上记下是谁，回调时再核对一次
    let link_user = if linking {
        Some(
            session_user(s, jar)
                .await
                .ok_or_else(|| refuse("session", "link started without a session"))?,
        )
    } else {
        None
    };
    sqlx::query("DELETE FROM oidc_flows WHERE expires_at < now()")
        .execute(&s.pool)
        .await?;
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_flows")
        .fetch_one(&s.pool)
        .await?;
    if pending >= MAX_PENDING_FLOWS {
        return Err(refuse("busy", format!("{pending} sign-ins in progress")));
    }
    let meta = metadata(&c).await?;
    let mut url = endpoint(&c, &meta, "authorization_endpoint")?;
    let state = auth::generate_jwt_secret();
    let nonce = auth::generate_jwt_secret();
    let verifier = auth::generate_jwt_secret();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    sqlx::query(
        "INSERT INTO oidc_flows (state, nonce, verifier, link_user_id, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(secs => $5))",
    )
    .bind(&state)
    .bind(&nonce)
    .bind(&verifier)
    .bind(link_user)
    .bind(FLOW_TTL_SECS as f64)
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
    Ok((state, url))
}

#[derive(Deserialize)]
pub struct Callback {
    code: Option<String>,
    state: Option<String>,
    /// 人在身份提供方那边点了取消，或者它拒绝了这次请求
    error: Option<String>,
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

/// 按 `kid` 找钥匙；令牌不带 `kid` 时，只有 JWKS 里恰好一把 RSA 钥匙才用它
fn pick_key<'a>(keys: &'a JwkSet, kid: Option<&str>) -> Option<&'a jsonwebtoken::jwk::Jwk> {
    match kid {
        Some(kid) => keys.find(kid),
        None => {
            let mut rsa = keys
                .keys
                .iter()
                .filter(|k| matches!(k.algorithm, jsonwebtoken::jwk::AlgorithmParameters::RSA(_)));
            match (rsa.next(), rsa.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        }
    }
}

fn verify_token(
    c: &Config,
    id_token: &str,
    keys: &JwkSet,
    nonce: &str,
) -> Result<Claims, AppError> {
    let header = decode_header(id_token).map_err(|_| AppError::Unauthorized)?;
    if header.alg != Algorithm::RS256 {
        return Err(AppError::Unauthorized);
    }
    let key = pick_key(keys, header.kid.as_deref()).ok_or(AppError::Unauthorized)?;
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

/// 回调办成的是哪一件事
enum Done {
    SignedIn(Uuid),
    Linked,
}

pub async fn callback(
    State(s): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Query(q): Query<Callback>,
) -> (CookieJar, Redirect) {
    let secure = auth::behind_tls(&headers, s.cookie_secure);
    let mut linking = false;
    // 先读后删：CookieJar 删掉的 cookie 再 `get` 就取不到了
    let outcome = finish(&s, &jar, q, &mut linking).await;
    let jar = jar
        .remove(flow_cookie(String::new(), true))
        .remove(flow_cookie(String::new(), false));
    match outcome {
        Ok(Done::SignedIn(user_id)) => match auth::issue_token(&s, user_id) {
            Ok(token) => (jar.add(auth::auth_cookie(token, secure)), Redirect::to("/")),
            Err(e) => {
                tracing::warn!(error = %e, "单点登录签发会话失败");
                (jar, back_to("/login", "unavailable"))
            }
        },
        Ok(Done::Linked) => (jar, Redirect::to("/account?sso=linked")),
        Err(r) => {
            tracing::warn!(code = r.code, reason = %r.reason, "单点登录被拒");
            // 失败也进台账，与密码登录的 `auth.login_failed` 同一个口径
            let _ = utopia_store::audit::record_opt(
                &s.pool,
                None,
                None,
                if linking {
                    "auth.oidc_link_failed"
                } else {
                    "auth.oidc_login_failed"
                },
                "user",
                None,
                json!({"reason": r.code, "detail": r.reason}),
            )
            .await;
            (
                jar,
                back_to(if linking { "/account" } else { "/login" }, r.code),
            )
        }
    }
}

async fn finish(
    s: &AppState,
    jar: &CookieJar,
    q: Callback,
    linking: &mut bool,
) -> Result<Done, Refusal> {
    if let Some(e) = q.error {
        return Err(refuse("cancelled", format!("provider returned {e}")));
    }
    let c = config().map_err(|_| refuse("unavailable", "not configured"))?;
    let (code, state) = match (q.code, q.state) {
        (Some(code), Some(state)) => (code, state),
        _ => return Err(refuse("invalid", "callback without code or state")),
    };
    let cookie = jar
        .get(FLOW_COOKIE_HOST)
        .or_else(|| jar.get(FLOW_COOKIE))
        .ok_or_else(|| refuse("session", "no state cookie in this browser"))?;
    use subtle::ConstantTimeEq;
    if !bool::from(cookie.value().as_bytes().ct_eq(state.as_bytes())) {
        return Err(refuse("session", "state does not match this browser"));
    }
    let flow: Option<(String, String, Option<Uuid>)> = sqlx::query_as(
        "DELETE FROM oidc_flows WHERE state = $1 AND expires_at > now()
         RETURNING nonce, verifier, link_user_id",
    )
    .bind(&state)
    .fetch_optional(&s.pool)
    .await?;
    let (nonce, verifier, link_user) =
        flow.ok_or_else(|| refuse("expired", "unknown or expired state"))?;
    *linking = link_user.is_some();

    let meta = metadata(&c).await?;
    let mut req = client()?
        .post(endpoint(&c, &meta, "token_endpoint")?)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", c.redirect.as_str()),
            ("client_id", c.client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ]);
    if let Some(secret) = &c.secret {
        req = req.basic_auth(form_encode(&c.client_id), Some(form_encode(secret)));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| refuse("unavailable", format!("token endpoint: {e}")))?;
    let tokens = read_json(resp, TOKEN_LIMIT)
        .await
        .map_err(|e| refuse("denied", format!("token endpoint: {e}")))?;
    let id_token = tokens["id_token"]
        .as_str()
        .ok_or_else(|| refuse("denied", "token response has no id_token"))?;

    let jwks_url = endpoint(&c, &meta, "jwks_uri")?;
    let parse = |v: Value| {
        serde_json::from_value::<JwkSet>(v).map_err(|e| refuse("unavailable", format!("jwks: {e}")))
    };
    let mut keys = parse(cached_json(&jwks_url, false).await?)?;
    // 身份提供方轮换了密钥：缓存里没有这个 kid 就强制刷新一次，再不认就是真不认
    let kid = decode_header(id_token).ok().and_then(|h| h.kid);
    if pick_key(&keys, kid.as_deref()).is_none() {
        keys = parse(cached_json(&jwks_url, true).await?)?;
    }
    let claims = verify_token(&c, id_token, &keys, &nonce)
        .map_err(|_| refuse("denied", "id_token failed verification"))?;

    match link_user {
        Some(user_id) => {
            // 完成绑定的必须还是发起绑定的那个人：流程是在他登录着的时候开始的，
            // 回调时同一个浏览器里得还是他的会话
            if session_user(s, jar).await != Some(user_id) {
                return Err(refuse(
                    "session",
                    "link finished outside the starting session",
                ));
            }
            link(s, &c, user_id, &claims.sub).await?;
            Ok(Done::Linked)
        }
        None => {
            let user_id: Option<Uuid> = sqlx::query_scalar(
                "SELECT i.user_id FROM oidc_identities i JOIN users u ON u.id = i.user_id
                  WHERE i.issuer = $1 AND i.subject = $2 AND u.deactivated_at IS NULL",
            )
            .bind(&c.issuer)
            .bind(&claims.sub)
            .fetch_optional(&s.pool)
            .await?;
            let user_id = user_id.ok_or_else(|| {
                refuse("unlinked", format!("subject {} is not linked", claims.sub))
            })?;
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
            Ok(Done::SignedIn(user_id))
        }
    }
}

async fn link(s: &AppState, c: &Config, user_id: Uuid, subject: &str) -> Result<(), Refusal> {
    let holder: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM oidc_identities WHERE issuer = $1 AND subject = $2",
    )
    .bind(&c.issuer)
    .bind(subject)
    .fetch_optional(&s.pool)
    .await?;
    match holder {
        Some(h) if h == user_id => return Ok(()),
        Some(_) => return Err(refuse("taken", "subject already linked to another account")),
        None => {}
    }
    let inserted =
        sqlx::query("INSERT INTO oidc_identities (issuer, subject, user_id) VALUES ($1, $2, $3)")
            .bind(&c.issuer)
            .bind(subject)
            .bind(user_id)
            .execute(&s.pool)
            .await;
    match inserted {
        Ok(_) => {}
        // 两个约束各对应一种撞车：subject 被别人抢先绑了，或者这个账号已经有一个
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            let code = if db.constraint().is_some_and(|n| n.contains("user")) {
                "already_linked"
            } else {
                "taken"
            };
            return Err(refuse(code, db.message().to_string()));
        }
        Err(e) => return Err(e.into()),
    }
    let _ = utopia_store::audit::record(
        &s.pool,
        None,
        user_id,
        "auth.oidc_link",
        "user",
        Some(user_id),
        json!({"issuer": c.issuer, "subject": subject}),
    )
    .await;
    Ok(())
}

/// 我自己的绑定：账户页用
pub async fn me(State(s): State<AppState>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let Ok(c) = config() else {
        return Ok(Json(json!({"enabled": false, "linked": false})));
    };
    let subject: Option<String> = sqlx::query_scalar(
        "SELECT subject FROM oidc_identities WHERE issuer = $1 AND user_id = $2",
    )
    .bind(&c.issuer)
    .bind(u.id)
    .fetch_optional(&s.pool)
    .await?;
    Ok(Json(
        json!({"enabled": true, "linked": subject.is_some(), "subject": subject}),
    ))
}

/// 解绑我自己的身份
pub async fn unlink_me(State(s): State<AppState>, AuthUser(u): AuthUser) -> ApiResult<Json<Value>> {
    let c = config()?;
    let done = sqlx::query("DELETE FROM oidc_identities WHERE issuer = $1 AND user_id = $2")
        .bind(&c.issuer)
        .bind(u.id)
        .execute(&s.pool)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound.into());
    }
    let _ = utopia_store::audit::record(
        &s.pool,
        None,
        u.id,
        "auth.oidc_unlink",
        "user",
        Some(u.id),
        json!({"issuer": c.issuer, "by": "self"}),
    )
    .await;
    Ok(Json(json!({"ok": true})))
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
          WHERE i.issuer = $1 AND u.org_id = $2 ORDER BY u.email",
    )
    .bind(&c.issuer)
    .bind(u.org_id)
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(
        json!({"issuer": c.issuer, "client_id": c.client_id, "redirect_uri": c.redirect, "identities": rows}),
    ))
}

/// 管理员解绑。**只有解绑，没有替人绑定**（见模块说明）。解绑不会让已签发的会话
/// 失效——会话是无状态的 JWT，要立刻切断访问得停用账号
pub async fn unbind(
    State(s): State<AppState>,
    AuthUser(u): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    if !u.is_admin {
        return Err(AppError::Forbidden.into());
    };
    let c = config()?;
    let done = sqlx::query(
        "DELETE FROM oidc_identities WHERE issuer = $1 AND user_id = $2
          AND user_id IN (SELECT id FROM users WHERE org_id = $3)",
    )
    .bind(&c.issuer)
    .bind(id)
    .bind(u.org_id)
    .execute(&s.pool)
    .await?;
    if done.rows_affected() == 0 {
        return Err(AppError::NotFound.into());
    }
    let _ = utopia_store::audit::record(
        &s.pool,
        None,
        u.id,
        "auth.oidc_unlink",
        "user",
        Some(id),
        json!({"issuer": c.issuer, "by": "admin"}),
    )
    .await;
    Ok(Json(json!({"ok": true})))
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_local_callbacks_can_use_http() {
        assert!(super::secure_url("http://localhost:1516/callback", true).is_ok());
        assert!(super::secure_url("http://[::1]:1516/callback", true).is_ok());
        assert!(super::secure_url("http://example.org/callback", true).is_err());
        assert!(super::secure_url("http://localhost:1516", false).is_err());
        assert!(super::secure_url("https://user:password@example.org", false).is_err());
    }

    #[test]
    fn a_client_secret_is_form_encoded_before_basic_auth() {
        assert_eq!(super::form_encode("abc"), "abc");
        assert_eq!(super::form_encode("a:b%c+d e"), "a%3Ab%25c%2Bd+e");
    }

    #[tokio::test]
    async fn a_response_over_its_limit_is_refused() {
        let big = axum::http::Response::new(vec![b' '; 2048]);
        assert!(super::read_json(reqwest::Response::from(big), 1024)
            .await
            .is_err());
        let ok = axum::http::Response::new(br#"{"a":1}"#.to_vec());
        assert_eq!(
            super::read_json(reqwest::Response::from(ok), 1024)
                .await
                .unwrap()["a"],
            1
        );
    }

    fn config() -> super::Config {
        super::Config {
            issuer: "https://id.example.test".into(),
            client_id: "utopia-test".into(),
            secret: None,
            redirect: "https://utopia.example.test/api/v1/auth/oidc/callback".into(),
            loopback_http: false,
        }
    }

    // This key is generated solely for these tests and is never used by the app.
    #[test]
    fn verifies_signature_issuer_audience_nonce_and_expiry() {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use serde_json::json;
        let c = config();
        let key = EncodingKey::from_rsa_pem(include_bytes!(
            "../../tests/fixtures/oidc_test_only_key.pem"
        ))
        .unwrap();
        let keys: jsonwebtoken::jwk::JwkSet = serde_json::from_str(include_str!(
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

        // 不带 kid：JWKS 里只有一把 RSA 钥匙时用它，多于一把就不猜
        header.kid = None;
        let no_kid = encode(&header, &valid, &key).unwrap();
        assert!(super::verify_token(&c, &no_kid, &keys, "expected").is_ok());
        let mut two = keys.clone();
        let mut second = two.keys[0].clone();
        second.common.key_id = Some("other".into());
        two.keys.push(second);
        assert!(super::verify_token(&c, &no_kid, &two, "expected").is_err());
    }
}
