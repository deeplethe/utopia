use sqlx::PgPool;
use utopia_core::models::{MemberView, OrgUser, Role};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

pub async fn list(pool: &PgPool, workspace_id: Uuid) -> AppResult<Vec<MemberView>> {
    let rows = sqlx::query_as(
        "SELECT m.user_id, u.email, u.display_name, m.role, u.is_admin
         FROM memberships m JOIN users u ON u.id = m.user_id
         -- 停用的人不再出现在成员列表里（见 `users.deactivated_at`）。成员关系那一行留着——
         -- 恢复账号时不必重新加回每一个工作区
         WHERE m.workspace_id = $1 AND u.deactivated_at IS NULL
         ORDER BY m.created_at",
    )
    .bind(workspace_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 部署内全部用户（添加成员的选人器）。
pub async fn org_users(pool: &PgPool, org_id: Uuid) -> AppResult<Vec<OrgUser>> {
    let rows = sqlx::query_as(
        "SELECT id, email, display_name, is_admin FROM users
         WHERE org_id = $1 AND deactivated_at IS NULL ORDER BY created_at",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn owner_count(pool: &PgPool, workspace_id: Uuid) -> AppResult<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM memberships WHERE workspace_id = $1 AND role = 'owner'",
    )
    .bind(workspace_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

pub async fn current_role(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
) -> AppResult<Option<Role>> {
    crate::workspaces::role_of(pool, user_id, workspace_id).await
}

/// 设置/添加成员角色（upsert）。防呆逻辑在 API 层。
pub async fn set_role(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    role: Role,
) -> AppResult<()> {
    // 目标用户必须存在于本组织,**而且在职**——否则能把一个已停用的账号
    // 加进工作区,它在成员列表里又看不见（那条查询过滤了停用的）,
    // 于是成了一条谁也发现不了的授权
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE id = $1 AND deactivated_at IS NULL")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    sqlx::query(
        "INSERT INTO memberships (user_id, workspace_id, role) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, workspace_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(user_id)
    .bind(workspace_id)
    .bind(role.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &PgPool, workspace_id: Uuid, user_id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM memberships WHERE workspace_id = $1 AND user_id = $2")
        .bind(workspace_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}
