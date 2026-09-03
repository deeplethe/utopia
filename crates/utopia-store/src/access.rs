//! KB 级访问判定 —— 全部 KB 作用域路由的唯一鉴权入口。
//! 判定链：系统管理员 → 全通；kb_members 矩阵有记录 → 按矩阵角色；
//! open 库 → 按部署角色（隐形 workspace 的 membership）；
//! restricted 库无记录 → NotFound（不泄露库的存在）。

use sqlx::PgPool;
use utopia_core::models::{KbMemberView, KnowledgeBase, MyKbInfo, Role, User};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 用户在某 KB 的有效角色（None = 不可见）。
pub async fn kb_role(pool: &PgPool, user: &User, kb: &KnowledgeBase) -> AppResult<Option<Role>> {
    if user.is_admin {
        return Ok(Some(Role::Owner));
    }
    let matrix: Option<(String,)> =
        sqlx::query_as("SELECT role FROM kb_members WHERE kb_id = $1 AND user_id = $2")
            .bind(kb.id)
            .bind(user.id)
            .fetch_optional(pool)
            .await?;
    if let Some((r,)) = matrix {
        return Ok(Role::parse(&r));
    }
    if kb.visibility == "open" {
        // open 库 = 部署内人人可读；写权限一律来自本库矩阵
        //（系统管理员在链首已 Owner；部署角色不再向库内映射写权）
        let ws: Option<(String,)> =
            sqlx::query_as("SELECT role FROM memberships WHERE workspace_id = $1 AND user_id = $2")
                .bind(kb.workspace_id)
                .bind(user.id)
                .fetch_optional(pool)
                .await?;
        return Ok(ws.map(|_| Role::Viewer));
    }
    Ok(None)
}

/// `kb_role` 的集合版：这个人**在所有工作区里**能看见的每个 KB → 有效角色。
///
/// **改上面那个函数就必须改这个**。之所以不用循环调 `kb_role` 拼出来：
/// 告警列表是跨库的，逐条一次查询就是 N+1，而告警数会随部署规模长。
/// 三条分支与 `kb_role` 一一对应，顺序也一致：
///   1. is_admin → 全部 KB，Owner
///   2. kb_members 有记录 → 矩阵角色
///   3. open 库 + 该工作区的 membership → Viewer
///
/// 第 3 条这里显式查了 `memberships`，而 `kbs::list_visible` 没查——
/// 那个函数已被工作区路由限定过范围，这个没有。
pub async fn visible_kb_roles(pool: &PgPool, user: &User) -> AppResult<Vec<(Uuid, Role)>> {
    if user.is_admin {
        let ids: Vec<(Uuid,)> = sqlx::query_as("SELECT id FROM knowledge_bases")
            .fetch_all(pool)
            .await?;
        return Ok(ids.into_iter().map(|(id,)| (id, Role::Owner)).collect());
    }
    let rows: Vec<(Uuid, Option<String>, bool)> = sqlx::query_as(
        "SELECT k.id,
                m.role,
                (k.visibility = 'open'
                 AND EXISTS (SELECT 1 FROM memberships ws
                             WHERE ws.workspace_id = k.workspace_id AND ws.user_id = $1))
         FROM knowledge_bases k
         LEFT JOIN kb_members m ON m.kb_id = k.id AND m.user_id = $1",
    )
    .bind(user.id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, matrix, open_member)| {
            // 矩阵优先于 open——跟 kb_role 一样，矩阵有记录就不再看 visibility
            match matrix {
                Some(r) => Role::parse(&r).map(|r| (id, r)),
                None if open_member => Some((id, Role::Viewer)),
                None => None,
            }
        })
        .collect())
}

/// 要求对 KB 至少具备 `min` 角色，返回 KB。
pub async fn require_kb(
    pool: &PgPool,
    user: &User,
    kb_id: Uuid,
    min: Role,
) -> AppResult<KnowledgeBase> {
    let kb = crate::kbs::get(pool, kb_id).await?;
    match kb_role(pool, user, &kb).await? {
        Some(r) if r >= min => Ok(kb),
        Some(_) => Err(AppError::Forbidden),
        None => Err(AppError::NotFound),
    }
}

// ---------------------------------------------------------------------------
// KB 成员矩阵
// ---------------------------------------------------------------------------

pub async fn kb_members(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<KbMemberView>> {
    let rows: Vec<KbMemberView> = sqlx::query_as(
        "SELECT m.user_id, u.email, u.display_name, m.role
         FROM kb_members m JOIN users u ON u.id = m.user_id
         -- 停用的人不出现在成员列表里（见 `users.deactivated_at`）；成员关系那一行留着
         WHERE m.kb_id = $1 AND u.deactivated_at IS NULL ORDER BY u.display_name",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn set_kb_member(
    pool: &PgPool,
    kb_id: Uuid,
    user_id: Uuid,
    role: &str,
    added_by: Option<Uuid>,
) -> AppResult<()> {
    if !matches!(role, "viewer" | "editor" | "admin") {
        return Err(AppError::Validation(
            "role must be viewer, editor or admin".into(),
        ));
    }
    // 改角色不改写最初邀请人（加入信息记录的是"谁引进来的"）
    sqlx::query(
        "INSERT INTO kb_members (kb_id, user_id, role, added_by) VALUES ($1, $2, $3, $4)
         ON CONFLICT (kb_id, user_id)
         DO UPDATE SET role = $3, added_by = COALESCE(kb_members.added_by, $4)",
    )
    .bind(kb_id)
    .bind(user_id)
    .bind(role)
    .bind(added_by)
    .execute(pool)
    .await?;
    Ok(())
}

/// 我在各可见库的成员信息 + 概览统计（账户层 Knowledge bases 页）。
pub async fn my_kb_infos(
    pool: &PgPool,
    kb_ids: &[Uuid],
    user_id: Uuid,
) -> AppResult<Vec<MyKbInfo>> {
    let rows: Vec<MyKbInfo> = sqlx::query_as(
        "SELECT k.id AS kb_id,
                m.role AS member_role,
                m.created_at AS joined_at,
                inviter.display_name AS added_by_name,
                (SELECT count(*) FROM documents d
                  WHERE d.kb_id = k.id AND d.deleted_at IS NULL) AS doc_count,
                (SELECT count(*) FROM kb_members mm WHERE mm.kb_id = k.id) AS member_count
         FROM knowledge_bases k
         LEFT JOIN kb_members m ON m.kb_id = k.id AND m.user_id = $2
         LEFT JOIN users inviter ON inviter.id = m.added_by
         WHERE k.id = ANY($1)",
    )
    .bind(kb_ids)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn remove_kb_member(pool: &PgPool, kb_id: Uuid, user_id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM kb_members WHERE kb_id = $1 AND user_id = $2")
        .bind(kb_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 部署配置
// ---------------------------------------------------------------------------

pub async fn open_registration(pool: &PgPool) -> AppResult<bool> {
    let row: Option<(bool,)> =
        sqlx::query_as("SELECT open_registration FROM deployment_settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(v,)| v).unwrap_or(true))
}

pub async fn set_open_registration(pool: &PgPool, value: bool) -> AppResult<()> {
    sqlx::query("UPDATE deployment_settings SET open_registration = $1")
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

/// 新建知识库时 `ontology_lang` 的默认值。
///
/// **刻意不叫"系统语言"**：叫那个名字的话，迟早有人试图把界面语言挂上来，
/// 然后发现挂不上（界面语言在客户端）。名字本身就该把误用挡住。见 docs/decisions/0004。
pub async fn default_ontology_lang(pool: &PgPool) -> AppResult<String> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT default_ontology_lang FROM deployment_settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(v,)| v).unwrap_or_else(|| "en".into()))
}

pub async fn set_default_ontology_lang(pool: &PgPool, value: &str) -> AppResult<()> {
    if !matches!(value, "en" | "zh") {
        return Err(AppError::invalid("bad_lang", "language must be en or zh"));
    }
    sqlx::query("UPDATE deployment_settings SET default_ontology_lang = $1")
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

/// 任务 worker 并发数（系统设置可改，改动即时生效——见 jobs::run_worker）。
pub async fn worker_concurrency(pool: &PgPool) -> AppResult<i32> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT worker_concurrency FROM deployment_settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
    // 与 `deployment_settings.worker_concurrency` 的列缺省保持一致（迁移 0011）。
    // 两处分开写是因为一处在 SQL、一处在 Rust，改一处不会带上另一处——
    // `the_backstop_can_be_raised` 那条测试盯着这件事
    Ok(row.map(|(v,)| v).unwrap_or(64))
}

pub async fn set_worker_concurrency(pool: &PgPool, value: i32) -> AppResult<()> {
    if !(1..=256).contains(&value) {
        return Err(AppError::invalid(
            "concurrency_range",
            "worker_concurrency must be between 1 and 256",
        ));
    }
    sqlx::query("UPDATE deployment_settings SET worker_concurrency = $1")
        .bind(value)
        .execute(pool)
        .await?;
    Ok(())
}

/// 落库自动生成的 JWT 密钥，返回最终生效的那一个。
///
/// `COALESCE` 让并发启动的多个实例收敛到同一个值：谁先写谁赢，后来者拿回的是
/// 库里已有的那条而不是自己刚生成的。分成「先读、没有再写」两步做不到这一点——
/// 两个实例会同时读到 NULL，各写各的，先写的那个从此在用一个已经不在库里的密钥，
/// 它签发的 token 到另一个实例上全部失效。
pub async fn ensure_jwt_secret(pool: &PgPool, generated: &str) -> AppResult<String> {
    let (secret,): (Option<String>,) = sqlx::query_as(
        "UPDATE deployment_settings SET jwt_secret = COALESCE(jwt_secret, $1)
         RETURNING jwt_secret",
    )
    .bind(generated)
    .fetch_one(pool)
    .await?;
    secret.ok_or_else(|| {
        AppError::Other(anyhow::anyhow!(
            "deployment_settings 没有单例行，JWT 密钥无处落库"
        ))
    })
}

/// 本体铺进抽取提示词的字符预算。超了改成按分块检索候选。
///
/// **放设置而不是常量**，是因为定死它需要一条曲线：每个本体规模下，全量内联
/// 与按块检索各测一次，看它们在哪里交叉。要重启一次服务才测得了一档的话，
/// 那条曲线不会有人跑第二遍。
pub async fn ontology_prompt_budget(pool: &PgPool) -> AppResult<usize> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT ontology_prompt_budget FROM deployment_settings LIMIT 1")
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(v,)| v.max(0) as usize).unwrap_or(24_000))
}
