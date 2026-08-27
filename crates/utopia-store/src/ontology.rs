//! 本体编辑器仓储：类型/关系 CRUD（带使用量与删除保护）+ 未匹配统计。

use utopia_core::models::{EntityInstance, EntityTypeView, OntologyMiss, RelationTypeView};
use utopia_core::{AppError, AppResult};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn entity_type_views(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<EntityTypeView>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.key, t.label, t.color, t.shape, t.builtin, t.parent_id, t.description,
                (SELECT count(*) FROM entities e
                 WHERE e.type_id = t.id AND e.merged_into IS NULL) AS usage
         FROM entity_types t WHERE t.kb_id = $1 ORDER BY lower(t.label)",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 某个类下的实体实例（按名称序，分页）。返回 (rows, total)。
pub async fn entity_instances(
    pool: &PgPool,
    kb_id: Uuid,
    type_id: Uuid,
    limit: i64,
    offset: i64,
) -> AppResult<(Vec<EntityInstance>, i64)> {
    let rows: Vec<EntityInstance> = sqlx::query_as(
        "SELECT e.id, e.canonical_name AS name,
                (SELECT count(*) FROM facts f
                 WHERE (f.subject_id = e.id OR f.object_id = e.id)
                   AND f.invalidated_at IS NULL) AS fact_count
         FROM entities e
         WHERE e.kb_id = $1 AND e.type_id = $2 AND e.merged_into IS NULL
         ORDER BY lower(e.canonical_name)
         LIMIT $3 OFFSET $4",
    )
    .bind(kb_id)
    .bind(type_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    let (total,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM entities e
         WHERE e.kb_id = $1 AND e.type_id = $2 AND e.merged_into IS NULL",
    )
    .bind(kb_id)
    .bind(type_id)
    .fetch_one(pool)
    .await?;
    Ok((rows, total))
}

fn validate_shape(shape: &str) -> AppResult<()> {
    if !matches!(shape, "circle" | "square") {
        return Err(AppError::Validation("shape must be circle or square".into()));
    }
    Ok(())
}

pub async fn relation_type_views(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<RelationTypeView>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.key, r.label, r.temporal, r.functional, r.inverse_functional, r.builtin, r.description,
                r.kind, r.domain_type_id, r.datatype, r.unit,
                (SELECT count(*) FROM facts f
                 WHERE f.predicate_id = r.id AND f.invalidated_at IS NULL) AS usage
         FROM relation_types r WHERE r.kb_id = $1 ORDER BY lower(r.label)",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

fn validate_key(key: &str) -> AppResult<()> {
    let ok = !key.is_empty()
        && key.len() <= 40
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !ok {
        return Err(AppError::Validation(
            "Key must be lowercase snake_case (a-z, 0-9, _), max 40 chars".into(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn create_entity_type(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    label: &str,
    color: &str,
    shape: &str,
    parent_id: Option<Uuid>,
    description: &str,
) -> AppResult<Uuid> {
    validate_key(key)?;
    validate_shape(shape)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label, color, shape, parent_id, description)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(color)
    .bind(shape)
    .bind(parent_id)
    .bind(description)
    .execute(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(format!("Type key '{key}' already exists"))
        }
        _ => AppError::Db(e),
    })?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_entity_type(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    label: &str,
    color: &str,
    shape: &str,
    parent_id: Option<Uuid>,
    description: &str,
) -> AppResult<()> {
    if parent_id == Some(id) {
        return Err(AppError::Validation("A class cannot be its own parent".into()));
    }
    validate_shape(shape)?;
    let res = sqlx::query(
        "UPDATE entity_types SET label = $3, color = $4, shape = $5, parent_id = $6,
                description = $7
         WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(id)
    .bind(label)
    .bind(color)
    .bind(shape)
    .bind(parent_id)
    .bind(description)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn delete_entity_type(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<()> {
    let (usage,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM entities WHERE type_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    if usage > 0 {
        return Err(AppError::Conflict(format!(
            "Cannot delete: {usage} entities use this type"
        )));
    }
    let res = sqlx::query("DELETE FROM entity_types WHERE id = $2 AND kb_id = $1 AND NOT builtin")
        .bind(kb_id)
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::Conflict("Built-in types cannot be deleted".into()));
    }
    Ok(())
}

/// 属性字段校验：attribute 必须有所属类和合法 datatype；relation 三者强制为空。
fn validate_attribute_fields(
    kind: &str,
    domain_type_id: Option<Uuid>,
    datatype: Option<&str>,
) -> AppResult<()> {
    match kind {
        "relation" => Ok(()),
        "attribute" => {
            if domain_type_id.is_none() {
                return Err(AppError::Validation(
                    "An attribute needs a class (domain)".into(),
                ));
            }
            if !matches!(datatype, Some("text" | "number" | "date" | "bool")) {
                return Err(AppError::Validation(
                    "datatype must be text / number / date / bool".into(),
                ));
            }
            Ok(())
        }
        _ => Err(AppError::Validation(
            "kind must be relation / attribute".into(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn create_relation_type(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    label: &str,
    temporal: &str,
    functional: bool,
    inverse_functional: bool,
    description: &str,
    kind: &str,
    domain_type_id: Option<Uuid>,
    datatype: Option<&str>,
    unit: Option<&str>,
) -> AppResult<Uuid> {
    validate_key(key)?;
    if !matches!(temporal, "state" | "event" | "eternal") {
        return Err(AppError::Validation(
            "temporal must be state / event / eternal".into(),
        ));
    }
    validate_attribute_fields(kind, domain_type_id, datatype)?;
    let is_attr = kind == "attribute";
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal, functional, inverse_functional, description,
                                     kind, domain_type_id, datatype, unit)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(temporal)
    .bind(functional)
    .bind(inverse_functional)
    .bind(description)
    .bind(kind)
    .bind(is_attr.then_some(domain_type_id).flatten())
    .bind(is_attr.then_some(datatype).flatten())
    .bind(is_attr.then_some(unit).flatten())
    .execute(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(format!("Relation key '{key}' already exists"))
        }
        _ => AppError::Db(e),
    })?;
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
pub async fn update_relation_type(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    label: &str,
    temporal: &str,
    functional: bool,
    inverse_functional: bool,
    description: &str,
    datatype: Option<&str>,
    unit: Option<&str>,
) -> AppResult<()> {
    if !matches!(temporal, "state" | "event" | "eternal") {
        return Err(AppError::Validation(
            "temporal must be state / event / eternal".into(),
        ));
    }
    if datatype.is_some() && !matches!(datatype, Some("text" | "number" | "date" | "bool")) {
        return Err(AppError::Validation(
            "datatype must be text / number / date / bool".into(),
        ));
    }
    // kind 与 domain 不可变（改 kind 会让存量事实语义错乱；挪类=删了重建）；
    // datatype/unit 只对 attribute 行生效，datatype 缺省保持原值
    let res = sqlx::query(
        "UPDATE relation_types SET label = $3, temporal = $4, functional = $5, inverse_functional = $6, description = $7,
                datatype = CASE WHEN kind = 'attribute' AND $8 IS NOT NULL THEN $8 ELSE datatype END,
                unit = CASE WHEN kind = 'attribute' THEN $9 ELSE unit END
         WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(id)
    .bind(label)
    .bind(temporal)
    .bind(functional)
    .bind(inverse_functional)
    .bind(description)
    .bind(datatype)
    .bind(unit)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

pub async fn delete_relation_type(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<()> {
    let (usage,): (i64,) = sqlx::query_as("SELECT count(*) FROM facts WHERE predicate_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if usage > 0 {
        return Err(AppError::Conflict(format!(
            "Cannot delete: {usage} facts use this relation"
        )));
    }
    let res =
        sqlx::query("DELETE FROM relation_types WHERE id = $2 AND kb_id = $1 AND NOT builtin")
            .bind(kb_id)
            .bind(id)
            .execute(pool)
            .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::Conflict("Built-in relations cannot be deleted".into()));
    }
    Ok(())
}

/* ---- 未匹配统计 ---- */

pub async fn record_miss(
    pool: &PgPool,
    kb_id: Uuid,
    kind: &str,
    key: &str,
    example: Option<&str>,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO ontology_misses (kb_id, kind, key, example)
         VALUES ($1, $2, left($3, 80), left($4, 200))
         ON CONFLICT (kb_id, kind, key)
         DO UPDATE SET count = ontology_misses.count + 1,
                       example = COALESCE(EXCLUDED.example, ontology_misses.example),
                       updated_at = now()",
    )
    .bind(kb_id)
    .bind(kind)
    .bind(key)
    .bind(example)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_misses(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<OntologyMiss>> {
    Ok(sqlx::query_as(
        "SELECT kind, key, example, count FROM ontology_misses
         WHERE kb_id = $1 ORDER BY count DESC, updated_at DESC LIMIT 50",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

pub async fn clear_miss(pool: &PgPool, kb_id: Uuid, kind: &str, key: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM ontology_misses WHERE kb_id = $1 AND kind = $2 AND key = $3")
        .bind(kb_id)
        .bind(kind)
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}
