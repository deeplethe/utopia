use sqlx::PgPool;
use utopia_core::models::{Role, User, Workspace};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 一次注册建出来的东西。
pub struct Registered {
    pub user: User,
    pub workspace: Workspace,
    /// 首个用户会连带建出 General 库；后续注册加入既有工作区，这里是 None。
    ///
    /// **把它交出去，是为了让调用方装冷启动本体包。** 装包要读内嵌的包文件、
    /// 走 OWL 导入，那需要 `AppState`，进不了这里的事务（#322）。
    pub general_kb: Option<Uuid>,
}

/// 注册（单租户模型）：
/// - 部署内还没有组织 → 首个用户：创建组织 + 默认工作区，成为 owner + 系统管理员；
/// - 已有组织 → 加入该组织，并以 viewer 进入默认（最早的）工作区；
///   `open_registration = false` 时拒绝（仅引导首用户例外）。
pub async fn register(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
    display_name: &str,
    org_name: Option<&str>,
    open_registration: bool,
) -> AppResult<Registered> {
    let mut tx = pool.begin().await?;

    let existing_org: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM organizations ORDER BY created_at LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?;

    let result = match existing_org {
        None => {
            // 首个用户：引导整个部署
            let org_id = Uuid::now_v7();
            sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)")
                .bind(org_id)
                .bind(org_name.unwrap_or("Default Organization"))
                .execute(&mut *tx)
                .await?;

            let user =
                insert_user(&mut tx, org_id, email, password_hash, display_name, true).await?;

            let workspace: Workspace = sqlx::query_as(
                "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3) RETURNING *",
            )
            .bind(Uuid::now_v7())
            .bind(org_id)
            .bind("Default Workspace")
            .fetch_one(&mut *tx)
            .await?;

            insert_membership(&mut tx, user.id, workspace.id, Role::Owner).await?;
            let kb_id = insert_general_kb(&mut tx, workspace.id, user.id).await?;
            (user, workspace, Some(kb_id))
        }
        Some((org_id,)) => {
            if !open_registration {
                return Err(AppError::invalid(
                    "registration_closed",
                    "Registration is closed for this deployment. Contact your administrator.",
                ));
            }
            let user =
                insert_user(&mut tx, org_id, email, password_hash, display_name, false).await?;

            // 加入默认（最早的）工作区；异常情况下组织没有工作区则补建一个
            let default_ws: Option<Workspace> = sqlx::query_as(
                "SELECT * FROM workspaces WHERE org_id = $1 ORDER BY created_at LIMIT 1",
            )
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await?;

            match default_ws {
                Some(ws) => {
                    insert_membership(&mut tx, user.id, ws.id, Role::Viewer).await?;
                    (user, ws, None)
                }
                None => {
                    let ws: Workspace = sqlx::query_as(
                        "INSERT INTO workspaces (id, org_id, name) VALUES ($1, $2, $3) RETURNING *",
                    )
                    .bind(Uuid::now_v7())
                    .bind(org_id)
                    .bind("Default Workspace")
                    .fetch_one(&mut *tx)
                    .await?;
                    insert_membership(&mut tx, user.id, ws.id, Role::Owner).await?;
                    (user, ws, None)
                }
            }
        }
    };

    tx.commit().await?;
    let (user, workspace, general_kb) = result;
    Ok(Registered {
        user,
        workspace,
        general_kb,
    })
}

async fn insert_user(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    org_id: Uuid,
    email: &str,
    password_hash: &str,
    display_name: &str,
    is_admin: bool,
) -> AppResult<User> {
    sqlx::query_as(
        "INSERT INTO users (id, org_id, email, password_hash, display_name, is_admin)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
    )
    .bind(Uuid::now_v7())
    .bind(org_id)
    .bind(email)
    .bind(password_hash)
    .bind(display_name)
    .bind(is_admin)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("This email is already registered".into())
        }
        _ => AppError::Db(e),
    })
}

async fn insert_membership(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    workspace_id: Uuid,
    role: Role,
) -> AppResult<()> {
    sqlx::query("INSERT INTO memberships (user_id, workspace_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(workspace_id)
        .bind(role.as_str())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 部署的公共空间。首个用户注册即建，与组织和工作区同一个事务——不然新部署的
/// 第一屏是个空壳：Graph 停在 Loading，切换器里无库可选，而"建第一个库"这件事
/// 从没有人告诉过用户。
///
/// 名字是 General 而非 Public：可见性由 visibility 字段表达，名字再说一遍既冗余，
/// 又会让自部署用户误读成"公开到互联网"。is_default 让它永远 open、不可删
/// （0012 的 CHECK 是 DB 级双保险）。
async fn insert_general_kb(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace_id: Uuid,
    owner_id: Uuid,
) -> AppResult<Uuid> {
    let kb_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO knowledge_bases
            (id, workspace_id, name, kind, description, is_default, visibility)
         VALUES ($1, $2, 'General', 'knowledge', $3, TRUE, 'open')",
    )
    .bind(kb_id)
    .bind(workspace_id)
    .bind("Shared space for the whole deployment. Everyone can read it.")
    .execute(&mut **tx)
    .await?;
    // 建库者记为本库 admin，与手动建库路径一致。首个用户本就是系统管理员、
    // 无需此行也进得来，但系统管理员身份日后可以撤销，本库权限不该随之蒸发。
    sqlx::query(
        "INSERT INTO kb_members (kb_id, user_id, role, added_by)
         VALUES ($1, $2, 'admin', $2)",
    )
    .bind(kb_id)
    .bind(owner_id)
    .execute(&mut **tx)
    .await?;
    Ok(kb_id)
}

/// 管理员代开账号：加入既有组织 + 默认工作区，部署角色由管理员指定。
pub async fn admin_create_user(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
    display_name: &str,
    role: Role,
) -> AppResult<User> {
    let mut tx = pool.begin().await?;
    let (org_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM organizations ORDER BY created_at LIMIT 1")
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::Validation("Deployment not bootstrapped yet".into()))?;
    let user = insert_user(&mut tx, org_id, email, password_hash, display_name, false).await?;
    let ws: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM workspaces WHERE org_id = $1 ORDER BY created_at LIMIT 1")
            .bind(org_id)
            .fetch_optional(&mut *tx)
            .await?;
    if let Some((ws_id,)) = ws {
        insert_membership(&mut tx, user.id, ws_id, role).await?;
    }
    tx.commit().await?;
    Ok(user)
}

/// 按 email 找账号——**只找在职的**。
///
/// 这一句 `deactivated_at IS NULL` 同时管两件事：停用的人登不进来，以及
/// 「一个 email 在停用账号里可能重复」时不会随机返回其中一条（唯一索引
/// 现在是部分的，只约束在职账号，见 `users.deactivated_at`）。
pub async fn find_user_by_email(pool: &PgPool, email: &str) -> AppResult<Option<User>> {
    let user = sqlx::query_as("SELECT * FROM users WHERE email = $1 AND deactivated_at IS NULL")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    Ok(user)
}

/// 按 id 找账号——**只找在职的**。
///
/// 会话校验走这里，所以停用**立即生效**，不必等 token 过期：已经签发出去的
/// token 下一次请求就查不到人。这是软删除唯一必须挡住的路径，其余读 users 的
/// 地方（审计、合并日志、改类账本）**要照旧查得到**——事情是他做的，人停用了
/// 不等于那件事没发生。
pub async fn find_user_by_id(pool: &PgPool, id: Uuid) -> AppResult<Option<User>> {
    let user = sqlx::query_as("SELECT * FROM users WHERE id = $1 AND deactivated_at IS NULL")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(user)
}

/// 改显示名（个人资料页）。
pub async fn update_display_name(pool: &PgPool, id: Uuid, display_name: &str) -> AppResult<User> {
    sqlx::query_as("UPDATE users SET display_name = $2 WHERE id = $1 RETURNING *")
        .bind(id)
        .bind(display_name)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

/// 改密码（server 层已验旧密并完成哈希）。
pub async fn update_password(pool: &PgPool, id: Uuid, password_hash: &str) -> AppResult<()> {
    sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(password_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// 停用一个账号（软删除，见 `users.deactivated_at`）。
///
/// **不删行。** 审计事件、合并日志、改类账本、口径确认的 `actor_id` 都指着这个人，
/// 而那些是审计材料——人走了仍然要能回答「当时是谁做的」。停用只断访问。
///
/// **不能停用自己**：管理员把自己停掉之后就没人能把他放回来了（这个系统没有
/// 「超级管理员」这一层）。挡在这里而不是只挡在界面上——界面挡得住误点，
/// 挡不住直接调接口。
///
/// **最后一个管理员不能停用**：否则这个组织从此没人能管成员。同样的理由。
pub async fn deactivate_user(pool: &PgPool, target: Uuid, actor: Uuid) -> AppResult<()> {
    if target == actor {
        return Err(AppError::Validation("不能停用自己的账号".into()));
    }
    let mut tx = pool.begin().await?;
    let victim: Option<(bool, Uuid, Option<chrono::DateTime<chrono::Utc>>)> =
        sqlx::query_as("SELECT is_admin, org_id, deactivated_at FROM users WHERE id = $1")
            .bind(target)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((is_admin, org_id, already)) = victim else {
        tx.rollback().await?;
        return Err(AppError::NotFound);
    };
    if already.is_some() {
        // 幂等：重复停用不是错误，也不该把 deactivated_by 改成第二个人
        tx.rollback().await?;
        return Ok(());
    }
    if is_admin {
        let (others,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM users
              WHERE org_id = $1 AND is_admin AND deactivated_at IS NULL AND id <> $2",
        )
        .bind(org_id)
        .bind(target)
        .fetch_one(&mut *tx)
        .await?;
        if others == 0 {
            tx.rollback().await?;
            return Err(AppError::Validation(
                "这是组织里最后一个管理员，停用之后没人能管成员".into(),
            ));
        }
    }
    sqlx::query("UPDATE users SET deactivated_at = now(), deactivated_by = $2 WHERE id = $1")
        .bind(target)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// 把停用的账号放回来。
///
/// **可能失败,而且失败得有道理**：停用期间有人用同一个 email 建了新账号，
/// 那个部分唯一索引会挡住这次恢复。这时该做的是让管理员看见冲突，而不是
/// 悄悄让两个在职账号共用一个 email。
pub async fn reactivate_user(pool: &PgPool, target: Uuid) -> AppResult<()> {
    let res = sqlx::query(
        "UPDATE users SET deactivated_at = NULL, deactivated_by = NULL
          WHERE id = $1 AND deactivated_at IS NOT NULL",
    )
    .bind(target)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}
