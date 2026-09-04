use sqlx::PgPool;
use utopia_core::models::KnowledgeBase;
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

pub async fn list(pool: &PgPool, workspace_id: Uuid) -> AppResult<Vec<KnowledgeBase>> {
    let rows =
        sqlx::query_as("SELECT * FROM knowledge_bases WHERE workspace_id = $1 ORDER BY created_at")
            .bind(workspace_id)
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

/// 用户可见的 KB：系统管理员全见；其余 = open 库 + 自己在矩阵里的 restricted 库。
pub async fn list_visible(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
) -> AppResult<Vec<KnowledgeBase>> {
    let rows = sqlx::query_as(
        "SELECT * FROM knowledge_bases k
         WHERE k.workspace_id = $1
           AND ($3
                OR k.visibility = 'open'
                OR EXISTS (SELECT 1 FROM kb_members m
                           WHERE m.kb_id = k.id AND m.user_id = $2))
         ORDER BY k.created_at",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(is_admin)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create(
    pool: &PgPool,
    workspace_id: Uuid,
    name: &str,
    kind: &str,
    description: Option<&str>,
) -> AppResult<KnowledgeBase> {
    // 部署的第一个库自动成为默认库：公共空间,永远 open、不可删（API 强制 + DB CHECK）
    //
    // ontology_lang 取部署默认值：中文部署不该每建一个库就手动选一次。
    // 之后按库可改——同一个部署里完全可能一个库读中文合同、另一个读英文论文
    let kb = sqlx::query_as(
        "INSERT INTO knowledge_bases
             (id, workspace_id, name, kind, description, is_default, ontology_lang)
         VALUES ($1, $2, $3, $4, $5,
                 NOT EXISTS (SELECT 1 FROM knowledge_bases WHERE workspace_id = $2),
                 COALESCE((SELECT default_ontology_lang FROM deployment_settings LIMIT 1), 'en'))
         RETURNING *",
    )
    .bind(Uuid::now_v7())
    .bind(workspace_id)
    .bind(name)
    .bind(kind)
    .bind(description)
    .fetch_one(pool)
    .await?;
    Ok(kb)
}

pub async fn get(pool: &PgPool, id: Uuid) -> AppResult<KnowledgeBase> {
    sqlx::query_as("SELECT * FROM knowledge_bases WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)
}

#[allow(clippy::too_many_arguments)]
pub async fn update(
    pool: &PgPool,
    id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    visibility: Option<&str>,
    auto_extend_ontology: Option<bool>,
    ontology_lang: Option<&str>,
    materialize_inferences: Option<bool>,
    inference_interval_minutes: Option<i32>,
    auto_type_resolution: Option<bool>,
) -> AppResult<KnowledgeBase> {
    // 改语言不回头重写已有的类——它们已经是这个库的数据，可能有人手工调过。
    // 这一列往后管的是**新**描述（自动扩本体、AI 建议）写成什么语言
    if let Some(l) = ontology_lang {
        if !matches!(l, "en" | "zh") {
            return Err(AppError::invalid("bad_lang", "language must be en or zh"));
        }
    }
    if let Some(v) = visibility {
        if !matches!(v, "open" | "restricted") {
            return Err(AppError::Validation(
                "visibility must be open or restricted".into(),
            ));
        }
        // 默认库永远 open：公共空间语义可依赖（改名/改描述不受限）
        if v == "restricted" {
            let current = get(pool, id).await?;
            if current.is_default {
                return Err(AppError::invalid(
                    "default_kb_open",
                    "The default knowledge base stays open to everyone.",
                ));
            }
        }
    }
    sqlx::query_as(
        "UPDATE knowledge_bases
         SET name = COALESCE($2, name),
             description = COALESCE($3, description),
             visibility = COALESCE($4, visibility),
             auto_extend_ontology = COALESCE($5, auto_extend_ontology),
             ontology_lang = COALESCE($6, ontology_lang),
             materialize_inferences = COALESCE($7, materialize_inferences),
             inference_interval_minutes = COALESCE($8, inference_interval_minutes),
             auto_type_resolution = COALESCE($9, auto_type_resolution),
             updated_at = now()
         WHERE id = $1 RETURNING *",
    )
    .bind(id)
    .bind(name)
    .bind(description)
    .bind(visibility)
    .bind(auto_extend_ontology)
    .bind(ontology_lang)
    .bind(materialize_inferences)
    .bind(inference_interval_minutes)
    .bind(auto_type_resolution)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> AppResult<()> {
    let current = get(pool, id).await?;
    if current.is_default {
        return Err(AppError::invalid(
            "default_kb_undeletable",
            "The default knowledge base cannot be deleted.",
        ));
    }
    let res = sqlx::query("DELETE FROM knowledge_bases WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}
