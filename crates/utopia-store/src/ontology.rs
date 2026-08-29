//! 本体编辑器仓储：类型/关系 CRUD（带使用量与删除保护）+ 未匹配统计。

use pgvector::Vector;
use sqlx::PgPool;
use utopia_core::models::{
    EntityInstance, EntityTypeView, OntologyImportView, OntologyMiss, RelationTypeView,
    TypeCandidate,
};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

pub async fn entity_type_views(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<EntityTypeView>> {
    Ok(sqlx::query_as(
        "SELECT t.id, t.key, t.label, t.color, t.shape, t.builtin, t.description,
                ARRAY(SELECT p.parent_id FROM entity_type_parents p
                      WHERE p.child_id = t.id) AS parents,
                (SELECT p.parent_id FROM entity_type_parents p
                  WHERE p.child_id = t.id AND p.is_primary) AS primary_parent,
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
        return Err(AppError::Validation(
            "shape must be circle or square".into(),
        ));
    }
    Ok(())
}

pub async fn relation_type_views(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<RelationTypeView>> {
    Ok(sqlx::query_as(
        "SELECT r.id, r.key, r.label, r.temporal, r.functional, r.inverse_functional, r.builtin, r.description,
                r.kind, r.datatype, r.unit,
                ARRAY(SELECT d.entity_type_id FROM relation_type_domains d
                      WHERE d.relation_type_id = r.id) AS domains,
                ARRAY(SELECT g.entity_type_id FROM relation_type_ranges g
                      WHERE g.relation_type_id = r.id) AS ranges,
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
        return Err(AppError::invalid(
            "bad_key",
            "Key must be lowercase snake_case (a-z, 0-9, _), max 40 chars",
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
    parents: &[Uuid],
    description: &str,
) -> AppResult<Uuid> {
    validate_key(key)?;
    validate_shape(shape)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label, color, shape, description)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(color)
    .bind(shape)
    .bind(description)
    .execute(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict(format!("Type key '{key}' already exists"))
        }
        _ => AppError::Db(e),
    })?;
    set_parents(pool, kb_id, id, parents).await?;
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
    parents: &[Uuid],
    description: &str,
) -> AppResult<()> {
    validate_shape(shape)?;
    let res = sqlx::query(
        "UPDATE entity_types SET label = $3, color = $4, shape = $5,
                description = $6
         WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(id)
    .bind(label)
    .bind(color)
    .bind(shape)
    .bind(description)
    .execute(pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    set_parents(pool, kb_id, id, parents).await?;
    Ok(())
}

/// 把 `parents` 设成这个类的全部父类，第一个当主父（左栏画在那一支下）。
///
/// **先查环再写**。单父时代只需要挡自环，一条链天然不会成环；DAG 里 A→B→A
/// 完全可能，而 `type_matches_domain` 沿父链上溯，成环就是死循环。
/// SQL 拦不住这个——外键只挡自环，更长的要应用来查。
pub async fn set_parents(
    pool: &PgPool,
    kb_id: Uuid,
    child: Uuid,
    parents: &[Uuid],
) -> AppResult<()> {
    if parents.contains(&child) {
        return Err(AppError::invalid(
            "self_parent",
            "A class cannot be its own parent",
        ));
    }
    if !parents.is_empty() {
        // 候选父类的全部祖先里出现 child，就说明这条边会成环
        let (cycles,): (i64,) = sqlx::query_as(
            "WITH RECURSIVE up(id) AS (
                 SELECT unnest($2::uuid[])
                 UNION
                 SELECT p.parent_id FROM entity_type_parents p JOIN up ON p.child_id = up.id
             )
             SELECT count(*) FROM up WHERE id = $1",
        )
        .bind(child)
        .bind(parents)
        .fetch_one(pool)
        .await?;
        if cycles > 0 {
            return Err(AppError::invalid(
                "parent_cycle",
                "That parent is already a subclass of this one",
            ));
        }
    }
    sqlx::query("DELETE FROM entity_type_parents WHERE child_id = $1")
        .bind(child)
        .execute(pool)
        .await?;
    for (i, p) in parents.iter().enumerate() {
        sqlx::query(
            "INSERT INTO entity_type_parents (child_id, parent_id, is_primary)
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(child)
        .bind(p)
        // 第一个当主父：界面上说明了"画在第一个下面"，不再多一个控件
        .bind(i == 0)
        .execute(pool)
        .await?;
    }
    let _ = kb_id;
    Ok(())
}

pub async fn delete_entity_type(pool: &PgPool, kb_id: Uuid, id: Uuid) -> AppResult<()> {
    let (usage,): (i64,) = sqlx::query_as("SELECT count(*) FROM entities WHERE type_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    if usage > 0 {
        return Err(AppError::Conflict(format!(
            "Cannot delete: {usage} entities use this type"
        )));
    }
    // 只剩这一个 domain 的属性随类一起走：属性必须挂在类上，
    // 留一个没有 domain 的属性等于留一个不会出现在任何地方的死行。
    // 还挂在别的类上的则只掉一条关联（外键 CASCADE 负责）
    sqlx::query(
        "DELETE FROM relation_types r
         WHERE r.kind = 'attribute' AND r.kb_id = $1
           AND EXISTS (SELECT 1 FROM relation_type_domains d
                       WHERE d.relation_type_id = r.id AND d.entity_type_id = $2)
           AND NOT EXISTS (SELECT 1 FROM relation_type_domains d
                           WHERE d.relation_type_id = r.id AND d.entity_type_id <> $2)",
    )
    .bind(kb_id)
    .bind(id)
    .execute(pool)
    .await?;
    let res = sqlx::query("DELETE FROM entity_types WHERE id = $2 AND kb_id = $1 AND NOT builtin")
        .bind(kb_id)
        .bind(id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::Conflict(
            "Built-in types cannot be deleted".into(),
        ));
    }
    Ok(())
}

/// 属性字段校验：attribute 必须有所属类和合法 datatype；relation 三者强制为空。
fn validate_attribute_fields(
    kind: &str,
    domains: &[Uuid],
    datatype: Option<&str>,
) -> AppResult<()> {
    match kind {
        "relation" => Ok(()),
        "attribute" => {
            if domains.is_empty() {
                return Err(AppError::invalid(
                    "attr_needs_class",
                    "An attribute needs a class (domain)",
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
    domains: &[Uuid],
    ranges: &[Uuid],
    datatype: Option<&str>,
    unit: Option<&str>,
) -> AppResult<Uuid> {
    validate_key(key)?;
    if !matches!(temporal, "state" | "event" | "eternal") {
        return Err(AppError::Validation(
            "temporal must be state / event / eternal".into(),
        ));
    }
    validate_attribute_fields(kind, domains, datatype)?;
    let is_attr = kind == "attribute";
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal, functional, inverse_functional, description,
                                     kind, datatype, unit)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
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
    // attribute 不写 range：它的值域是字面量类型，落在 datatype 上
    set_domains_ranges(pool, id, domains, if is_attr { &[] } else { ranges }).await?;
    Ok(id)
}

/// 覆盖式写入 domain / range。**先删后插**，所以它既能用于新建也能用于重导入，
/// 且不会留下上一轮的残余。
async fn set_domains_ranges(
    pool: &PgPool,
    relation_type_id: Uuid,
    domains: &[Uuid],
    ranges: &[Uuid],
) -> AppResult<()> {
    for (table, ids) in [
        ("relation_type_domains", domains),
        ("relation_type_ranges", ranges),
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE relation_type_id = $1"))
            .bind(relation_type_id)
            .execute(pool)
            .await?;
        if ids.is_empty() {
            continue;
        }
        // unnest 一次插完，省去逐条往返
        sqlx::query(&format!(
            "INSERT INTO {table} (relation_type_id, entity_type_id)
             SELECT $1, x FROM unnest($2::uuid[]) AS x
             ON CONFLICT DO NOTHING"
        ))
        .bind(relation_type_id)
        .bind(ids)
        .execute(pool)
        .await?;
    }
    Ok(())
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
    // None = 不动。不管 domain 的调用方（属性表单）传 None，
    // 否则一次不相干的改名就会把属性的 domain 清空
    domains: Option<&[Uuid]>,
    ranges: Option<&[Uuid]>,
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
    // kind 不可变（改它会让存量事实语义错乱）。domain/range 可改——
    // 它们是签名不是身份，"这个属性也适用于承包商" 是个正当的编辑。
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
    if domains.is_some() || ranges.is_some() {
        let (kind,): (String,) = sqlx::query_as("SELECT kind FROM relation_types WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;
        let next_domains = domains.unwrap_or(&[]);
        if kind == "attribute" && next_domains.is_empty() {
            return Err(AppError::invalid(
                "attr_needs_class",
                "An attribute needs a class (domain)",
            ));
        }
        // 属性没有 range：它的值域是字面量类型，落在 datatype 上
        let next_ranges: &[Uuid] = if kind == "attribute" {
            &[]
        } else {
            ranges.unwrap_or(&[])
        };
        set_domains_ranges(pool, id, next_domains, next_ranges).await?;
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
        return Err(AppError::Conflict(
            "Built-in relations cannot be deleted".into(),
        ));
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
        // 已被用户拒绝的不再累加：否则计数会把"不要"重新顶成一个待处理信号
        "INSERT INTO ontology_misses (kb_id, kind, key, example)
         VALUES ($1, $2, left($3, 80), left($4, 200))
         ON CONFLICT (kb_id, kind, key)
         DO UPDATE SET count = ontology_misses.count + 1,
                       example = COALESCE(EXCLUDED.example, ontology_misses.example),
                       updated_at = now()
         WHERE ontology_misses.dismissed_at IS NULL",
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
         WHERE kb_id = $1 AND dismissed_at IS NULL
         ORDER BY count DESC, updated_at DESC LIMIT 50",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 用户说"不要这个"。**标记而非删除**——删掉的话下一次抽取遇到同一个词
/// 原样插回来，用户的拒绝活不过一轮抽取。自动扩展路径也据此绕开。
pub async fn dismiss_miss(pool: &PgPool, kb_id: Uuid, kind: &str, key: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE ontology_misses SET dismissed_at = now()
         WHERE kb_id = $1 AND kind = $2 AND key = $3 AND dismissed_at IS NULL",
    )
    .bind(kb_id)
    .bind(kind)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 本体已经覆盖了这个说法（采纳时调用）：与"用户拒绝"不同，这条真的可以清掉，
/// 下次抽取它会命中本体，不再是未匹配。
pub async fn clear_miss(pool: &PgPool, kb_id: Uuid, kind: &str, key: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM ontology_misses WHERE kb_id = $1 AND kind = $2 AND key = $3")
        .bind(kb_id)
        .bind(kind)
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/* ---- OWL 导入 ---- */

/// 建一个带 IRI 的类。IRI 是全局身份，重导入据它匹配（见 0001 P2）。
pub async fn create_entity_type_with_iri(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    label: &str,
    description: &str,
    iri: &str,
) -> AppResult<Uuid> {
    validate_key(key)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO entity_types (id, kb_id, key, label, color, shape, description, iri)
         VALUES ($1, $2, $3, $4, '#8ea5bd', 'circle', $5, $6)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(description)
    .bind(iri)
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

/// 重导入时按 IRI 更新标签与描述。**key 不动**——它可能已经被抽取出的实体
/// 和提示词引用，改它等于把已有数据的引用打断；上游改 label 是常态，
/// 而 IRI 才是身份。
pub async fn update_type_from_import(
    pool: &PgPool,
    kb_id: Uuid,
    iri: &str,
    label: &str,
    description: &str,
) -> AppResult<Option<Uuid>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE entity_types
         SET label = $3,
             -- 空描述不覆盖已有的：上游可能没写 rdfs:comment，而本地可能
             -- 已经被人按自己的语料调过，那份调整比空值有价值
             description = CASE WHEN $4 = '' THEN description ELSE $4 END
         WHERE kb_id = $1 AND iri = $2 RETURNING id",
    )
    .bind(kb_id)
    .bind(iri)
    .bind(label)
    .bind(description)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// 设父类。自环与已是该父类的情形静默跳过。
/// 从导入建一个属性（`kind='attribute'`），带 IRI。
///
/// 与 [`create_relation_type`] 的区别只在多了 `iri` 与 key 冲突的处置：
/// 导入按 IRI 认身份，key 撞了是"两个不同的东西争一个短标签"，
/// 由调用方在计划阶段报告并跳过，到这里不该再撞——所以冲突时返回 None
/// 而不是覆盖，让调用方把它计进"跳过"。
///
/// `temporal` 固定 `state`：属性是随时间变化的取值（薪资、人数），
/// 新值闭合旧值正是我们要的。OWL 里没有对应概念，猜 event 或 eternal 都更差。
#[allow(clippy::too_many_arguments)]
pub async fn create_attribute_with_iri(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    label: &str,
    description: &str,
    iri: &str,
    domains: &[Uuid],
    datatype: &str,
) -> AppResult<Option<Uuid>> {
    validate_key(key)?;
    let id = Uuid::now_v7();
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO relation_types
             (id, kb_id, key, label, temporal, functional, inverse_functional,
              description, kind, datatype, iri)
         VALUES ($1, $2, $3, $4, 'state', FALSE, FALSE, $5, 'attribute', $6, $7)
         ON CONFLICT (kb_id, key) DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(description)
    .bind(datatype)
    .bind(iri)
    .fetch_optional(pool)
    .await?;
    let Some((new_id,)) = row else {
        return Ok(None);
    };
    set_domains_ranges(pool, new_id, domains, &[]).await?;
    Ok(Some(new_id))
}

/// 从导入建一个关系（`kind='relation'`），带 IRI。
///
/// `temporal` 固定 `state`：OWL 没有对应概念，而 state（有区间）是三者里唯一
/// 不丢信息的——event 会把区间压成时点，eternal 会宣称它永不改变。
///
/// **`functional` / `inverse_functional` 照词汇表写**。它们驱动时态引擎自动闭合
/// 旧事实，猜错就成批造假冲突（`part_of` 那次 59 条）。预览已经把声明为函数性的
/// 关系单独列出来让人过目，所以这里不再自作主张改成 false。
#[allow(clippy::too_many_arguments)]
pub async fn create_relation_with_iri(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    label: &str,
    description: &str,
    iri: &str,
    functional: bool,
    inverse_functional: bool,
    domains: &[Uuid],
    ranges: &[Uuid],
) -> AppResult<Option<Uuid>> {
    validate_key(key)?;
    let id = Uuid::now_v7();
    let row: Option<(Uuid,)> = sqlx::query_as(
        "INSERT INTO relation_types
             (id, kb_id, key, label, temporal, functional, inverse_functional,
              description, kind, iri)
         VALUES ($1, $2, $3, $4, 'state', $5, $6, $7, 'relation', $8)
         ON CONFLICT (kb_id, key) DO NOTHING
         RETURNING id",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(functional)
    .bind(inverse_functional)
    .bind(description)
    .bind(iri)
    .fetch_optional(pool)
    .await?;
    let Some((new_id,)) = row else {
        return Ok(None);
    };
    set_domains_ranges(pool, new_id, domains, ranges).await?;
    Ok(Some(new_id))
}

/// 重导入时更新一个已按 IRI 认下的关系。
///
/// **key 不动**（它可能已被事实引用），**空描述不覆盖**（人写过的比上游的准），
/// **domain/range 整体重写**——它们是上游声明的结构，不是人调过的措辞。
pub async fn update_relation_from_import(
    pool: &PgPool,
    kb_id: Uuid,
    iri: &str,
    label: &str,
    description: &str,
    domains: &[Uuid],
    ranges: &[Uuid],
) -> AppResult<bool> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE relation_types
            SET label = $3,
                description = CASE WHEN $4 = '' THEN description ELSE $4 END
          WHERE kb_id = $1 AND iri = $2
          RETURNING id",
    )
    .bind(kb_id)
    .bind(iri)
    .bind(label)
    .bind(description)
    .fetch_optional(pool)
    .await?;
    let Some((id,)) = row else {
        return Ok(false);
    };
    set_domains_ranges(pool, id, domains, ranges).await?;
    Ok(true)
}

/// 记一次导入。原文已按内容寻址存进 blob，这里只记账。
#[allow(clippy::too_many_arguments)]
pub async fn record_import(
    pool: &PgPool,
    kb_id: Uuid,
    sha256: &str,
    filename: &str,
    format: &str,
    byte_size: i64,
    summary: &serde_json::Value,
    actor: Uuid,
) -> AppResult<Uuid> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO ontology_imports
            (id, kb_id, sha256, filename, format, byte_size, summary, imported_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(sha256)
    .bind(filename)
    .bind(format)
    .bind(byte_size)
    .bind(summary)
    .bind(actor)
    .execute(pool)
    .await?;
    Ok(id)
}

/// 一个库的导入历史（带导入人显示名；删号后为 NULL）。
pub async fn list_imports(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<OntologyImportView>> {
    Ok(sqlx::query_as(
        "SELECT i.id, i.filename, i.format, i.byte_size, i.summary, i.imported_at,
                u.display_name AS imported_by_name
         FROM ontology_imports i
         LEFT JOIN users u ON u.id = i.imported_by
         WHERE i.kb_id = $1 ORDER BY i.imported_at DESC LIMIT 50",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 一条待嵌入的本体行。`text` 是要送去嵌入的那段字，`kind` 决定回写哪张表。
#[derive(Debug, Clone)]
pub struct TypeToEmbed {
    pub id: Uuid,
    pub kind: TypeKind,
    pub text: String,
}

/// 本体行属于哪张表。类进 `entity_types`，关系与属性同住 `relation_types`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Entity,
    Relation,
}

/// 嵌入用的那段字：标签在前、描述在后。
///
/// **key 不进去。** key 是模型读写的令牌（`founding_date`），描述才是这个类型
/// 的意思所在。把 key 混进来，检索会被"两个 key 长得像"带偏——而
/// `position`（列表位次）与 `position`（职位）正是长得一模一样的两回事。
fn embed_text(label: &str, description: &str) -> String {
    let d = description.trim();
    if d.is_empty() {
        label.trim().to_string()
    } else {
        format!("{}\n{}", label.trim(), d)
    }
}

/// 哪些本体行的向量是陈的（没嵌过、描述改了、或换了嵌入模型）。
///
/// 判据是**比对当时嵌的原文与模型名**，不是看时间戳：描述改了、模型换了，
/// 时间戳一样看不出来。这样也不必在每个改描述的写入点挂钩子——漏一个就悄悄烂掉。
pub async fn types_needing_embedding(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
) -> AppResult<Vec<TypeToEmbed>> {
    let mut out = Vec::new();
    let ents: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, label, coalesce(description, '') FROM entity_types
         WHERE kb_id = $1
           AND (embedding IS NULL
                OR embedded_model IS DISTINCT FROM $2
                OR embedded_text IS DISTINCT FROM
                   CASE WHEN coalesce(btrim(description), '') = '' THEN btrim(label)
                        ELSE btrim(label) || E'\n' || btrim(description) END)",
    )
    .bind(kb_id)
    .bind(model)
    .fetch_all(pool)
    .await?;
    out.extend(ents.into_iter().map(|(id, label, desc)| TypeToEmbed {
        id,
        kind: TypeKind::Entity,
        text: embed_text(&label, &desc),
    }));
    let rels: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, label, coalesce(description, '') FROM relation_types
         WHERE kb_id = $1
           AND (embedding IS NULL
                OR embedded_model IS DISTINCT FROM $2
                OR embedded_text IS DISTINCT FROM
                   CASE WHEN coalesce(btrim(description), '') = '' THEN btrim(label)
                        ELSE btrim(label) || E'\n' || btrim(description) END)",
    )
    .bind(kb_id)
    .bind(model)
    .fetch_all(pool)
    .await?;
    out.extend(rels.into_iter().map(|(id, label, desc)| TypeToEmbed {
        id,
        kind: TypeKind::Relation,
        text: embed_text(&label, &desc),
    }));
    Ok(out)
}

/// 回写向量，连同"嵌的是哪段字、用的哪个模型"。三者必须同一次写入——
/// 只写向量而不写来源，下一轮就会认为它还是陈的，从此每轮重嵌。
pub async fn set_type_embeddings(
    pool: &PgPool,
    model: &str,
    items: &[(TypeToEmbed, Vec<f32>)],
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    for (item, emb) in items {
        let table = match item.kind {
            TypeKind::Entity => "entity_types",
            TypeKind::Relation => "relation_types",
        };
        sqlx::query(&format!(
            "UPDATE {table} SET embedding = $2, embedded_text = $3, embedded_model = $4
             WHERE id = $1"
        ))
        .bind(item.id)
        .bind(Vector::from(emb.clone()))
        .bind(&item.text)
        .bind(model)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 与给定向量最近的 k 个实体类型。
pub async fn nearest_entity_types(
    pool: &PgPool,
    kb_id: Uuid,
    embedding: &[f32],
    limit: i64,
) -> AppResult<Vec<TypeCandidate>> {
    let rows: Vec<(Uuid, String, String, String, f64)> = sqlx::query_as(
        "SELECT id, key, label, coalesce(description, ''), (embedding <=> $2)::float8
         FROM entity_types
         WHERE kb_id = $1 AND embedding IS NOT NULL
         ORDER BY embedding <=> $2
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(Vector::from(embedding.to_vec()))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, key, label, description, distance)| TypeCandidate {
            id,
            key,
            label,
            description,
            kind: None,
            distance: distance as f32,
        })
        .collect())
}

/// 与给定向量最近的 k 个关系/属性。
///
/// `only_kind` 分道：字面值宾语的事实要找的是属性，实体宾语的要找的是关系。
/// 不分道就会把 `founding_date` 这种属性推给一条关系事实，反过来也一样。
pub async fn nearest_relation_types(
    pool: &PgPool,
    kb_id: Uuid,
    embedding: &[f32],
    limit: i64,
    only_kind: Option<&str>,
) -> AppResult<Vec<TypeCandidate>> {
    let rows: Vec<(Uuid, String, String, String, String, f64)> = sqlx::query_as(
        "SELECT id, key, label, coalesce(description, ''), kind, (embedding <=> $2)::float8
         FROM relation_types
         WHERE kb_id = $1 AND embedding IS NOT NULL
           AND ($4::text IS NULL OR kind = $4)
         ORDER BY embedding <=> $2
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(Vector::from(embedding.to_vec()))
    .bind(limit)
    .bind(only_kind)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, key, label, description, kind, distance)| TypeCandidate {
                id,
                key,
                label,
                description,
                kind: Some(kind),
                distance: distance as f32,
            },
        )
        .collect())
}

/// 按 key 找关系/属性的 id。给"映射到已有类型"那条路用。
///
/// 不区分 kind：属性与关系同住一张表且共用 key 命名空间，调用方拿到 id 之后
/// 该怎么用它自己清楚（改写事实时谓词就是谓词）。
pub async fn relation_type_id_by_key(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
) -> AppResult<Option<Uuid>> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM relation_types WHERE kb_id = $1 AND key = $2")
            .bind(kb_id)
            .bind(key)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id,)| id))
}
