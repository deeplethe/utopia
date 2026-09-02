//! 个人访问令牌（见 `docs/decisions/0014`）。
//!
//! **令牌以发它的人的身份行事，但不必是这个人的全部**：
//!
//! ```text
//! 有效权限 = 这个人的角色 ∩ 这枚令牌的 scope
//! ```
//!
//! 交集不是并集——viewer 的令牌勾上 write 也还是只读。所以这个模块只回答
//! 「这串字符对应谁、这枚令牌准他干到哪一步」，**准不准他碰某个库仍旧由
//! `access::require_kb` 判**，一行都不用改。

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use utopia_core::models::TokenView;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 明文令牌的前缀。与 `sources.ingest_token` 的 `utp_` 区分开——
/// 两者能干的事差很远，在日志或配置文件里一眼要认得出是哪一种
const PREFIX: &str = "utp_pat_";
/// 列表里给人认的那一小截（含前缀）。够对上配置文件里那一串，又不足以复原
const SHOWN: usize = 16;

/// 校验通过后拿到的东西：谁，以及这枚令牌准他干到哪一步。
pub struct Authenticated {
    pub user_id: Uuid,
    pub token_id: Uuid,
    /// read | write
    pub scope: String,
    /// None = 这个人能进的全部库
    pub kb_ids: Option<Vec<Uuid>>,
}

impl Authenticated {
    /// 这枚令牌准不准碰这个库。
    ///
    /// **这不是权限判断，是范围判断。** 返回 true 只说明「令牌没把它排除掉」，
    /// 那个人在这个库里是什么角色，还得照常问 `access::require_kb`。
    pub fn covers(&self, kb_id: Uuid) -> bool {
        match &self.kb_ids {
            None => true,
            Some(ids) => ids.contains(&kb_id),
        }
    }

    pub fn can_write(&self) -> bool {
        self.scope == "write"
    }
}

/// **SHA-256，不是 argon2。** 这一处与密码的存法不同，两个理由：
///
/// 1. **令牌是高熵随机串，不是人选的密码。** argon2 慢是为了让爆破一个
///    「password123」变得不划算；对 244 位随机数，慢一百万倍也照样爆不动，
///    买不到任何东西。
/// 2. **argon2 每行盐不同，查不了。** 校验是热路径（每次工具调用一次），
///    而 `WHERE token_hash = $1` 走唯一索引是一次命中；换成 argon2 就得
///    把所有令牌取回来逐个 verify——发得越多越慢。
fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 发一枚。**返回的明文是它唯一一次出现**——库里只存哈希，丢了只能重发。
pub async fn issue(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    scope: &str,
    kb_ids: Option<&[Uuid]>,
    expires_at: Option<DateTime<Utc>>,
) -> AppResult<(TokenView, String)> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return AppResult::Err(AppError::invalid(
            "bad_token_name",
            "Token name must be 1-64 characters",
        ));
    }
    if !matches!(scope, "read" | "write") {
        return Err(AppError::invalid(
            "bad_token_scope",
            "Scope must be read or write",
        ));
    }
    // 两个 v4 拼起来 ≈ 244 位熵。与 `new_ingest_token` 同一个做法，
    // 换个前缀是为了在日志里分得清
    let plain = format!(
        "{PREFIX}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    );
    let id = Uuid::now_v7();
    let view: TokenView = sqlx::query_as(
        "INSERT INTO personal_tokens
             (id, user_id, name, token_hash, token_prefix, scope, kb_ids, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING id, name, token_prefix, scope, kb_ids, expires_at,
                   last_used_at, revoked_at, created_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(hash(&plain))
    .bind(&plain[..SHOWN])
    .bind(scope)
    .bind(kb_ids)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok((view, plain))
}

/// 我发过哪几把。撤销过的也列——**撤过这件事本身要看得见**。
pub async fn list(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<TokenView>> {
    Ok(sqlx::query_as(
        "SELECT id, name, token_prefix, scope, kb_ids, expires_at,
                last_used_at, revoked_at, created_at
           FROM personal_tokens WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// 撤一把。**打戳不删行**：删了行，「这把钥匙存在过」就查不到了，
/// 而那正是事后追查要问的第一件事。
pub async fn revoke(pool: &PgPool, user_id: Uuid, token_id: Uuid) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE personal_tokens SET revoked_at = now()
          WHERE id = $2 AND user_id = $1 AND revoked_at IS NULL",
    )
    .bind(user_id)
    .bind(token_id)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

/// 明文 → 这是谁、准干到哪一步。
///
/// **过期与撤销在 SQL 里判，不在 Rust 里判。** 取回来再比较的话，
/// 「取回来」和「比较」之间那一段时间里被撤掉的令牌照样能过——而 MCP 的
/// 连接是长命的，这一段能长到有意义。
///
/// 顺手写 `last_used_at`：撤销之前人要答得出「这把还在用吗」，
/// 没有这个数没人敢撤。
pub async fn authenticate(pool: &PgPool, plain: &str) -> AppResult<Authenticated> {
    if !plain.starts_with(PREFIX) {
        return Err(AppError::Unauthorized);
    }
    let row: Option<(Uuid, Uuid, String, Option<Vec<Uuid>>)> = sqlx::query_as(
        "UPDATE personal_tokens SET last_used_at = now()
          WHERE token_hash = $1
            AND revoked_at IS NULL
            AND (expires_at IS NULL OR expires_at > now())
        RETURNING id, user_id, scope, kb_ids",
    )
    .bind(hash(plain))
    .fetch_optional(pool)
    .await?;
    let Some((token_id, user_id, scope, kb_ids)) = row else {
        return Err(AppError::Unauthorized);
    };
    Ok(Authenticated {
        user_id,
        token_id,
        scope,
        kb_ids,
    })
}
