use utopia_core::models::{Role, Workspace};
use utopia_core::{AppError, AppResult};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<Workspace>> {
    let rows = sqlx::query_as(
        "SELECT w.* FROM workspaces w
         JOIN memberships m ON m.workspace_id = w.id
         WHERE m.user_id = $1
         ORDER BY w.created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 在用户所属组织内新建工作区，创建者为 owner。
pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    user_id: Uuid,
    name: &str,
) -> AppResult<Workspace> {
    let mut tx = pool.begin().await?;
    let ws: Workspace =
        sqlx::query_as("INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3) RETURNING *")
            .bind(Uuid::now_v7())
            .bind(org_id)
            .bind(name)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("INSERT INTO memberships (user_id, workspace_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(ws.id)
        .bind(Role::Owner.as_str())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ws)
}

pub async fn get(pool: &PgPool, id: Uuid) -> AppResult<Workspace> {
    sqlx::query_as("SELECT * FROM workspaces WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// 取用户在工作区中的角色；非成员返回 None。
pub async fn role_of(pool: &PgPool, user_id: Uuid, workspace_id: Uuid) -> AppResult<Option<Role>> {
    let role: Option<(String,)> =
        sqlx::query_as("SELECT role FROM memberships WHERE user_id = $1 AND workspace_id = $2")
            .bind(user_id)
            .bind(workspace_id)
            .fetch_optional(pool)
            .await?;
    Ok(role.and_then(|(r,)| Role::parse(&r)))
}

/// 权限检查：非成员 → NotFound（不泄露存在性）；角色不足 → Forbidden。
pub async fn require_role(
    pool: &PgPool,
    user_id: Uuid,
    workspace_id: Uuid,
    min: Role,
) -> AppResult<Role> {
    match role_of(pool, user_id, workspace_id).await? {
        None => Err(AppError::NotFound),
        Some(r) if r >= min => Ok(r),
        Some(_) => Err(AppError::Forbidden),
    }
}

pub async fn rename(pool: &PgPool, id: Uuid, name: &str) -> AppResult<Workspace> {
    sqlx::query_as("UPDATE workspaces SET name = $2 WHERE id = $1 RETURNING *")
        .bind(id)
        .bind(name)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let res = sqlx::query("DELETE FROM workspaces WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}
