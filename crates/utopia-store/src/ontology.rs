//! 本体编辑器仓储：类型/关系 CRUD（带使用量与删除保护）+ 未匹配统计。

use pgvector::Vector;
use sqlx::PgPool;
use utopia_core::models::{
    EntityInstance, EntityTypeView, OntologyImportView, OntologyMiss, RelationAxioms,
    RelationTypeView, TypeCandidate,
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
                ARRAY(SELECT d.b_id FROM entity_type_disjoint d
                      WHERE d.kb_id = t.kb_id AND d.a_id = t.id) AS disjoint,
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
        "SELECT r.id, r.key, r.label, r.temporal, r.functional, r.inverse_functional,
                r.is_transitive, r.is_symmetric, r.is_asymmetric, r.is_irreflexive,
                r.inverse_of, r.sub_property_of,
                r.builtin, r.description,
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
/// 改一个实体类。
///
/// **`color: None` = 保持原色**，不是重置。从前这里是 `&str`，调用方不给就
/// 写死一个默认灰蓝，于是任何一次不带颜色的改名都会抹掉用户挑过的颜色。
pub async fn update_entity_type(
    pool: &PgPool,
    kb_id: Uuid,
    id: Uuid,
    label: &str,
    color: Option<&str>,
    shape: &str,
    parents: &[Uuid],
    description: &str,
) -> AppResult<()> {
    validate_shape(shape)?;
    let res = sqlx::query(
        "UPDATE entity_types SET label = $3, color = COALESCE($4, color), shape = $5,
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

/// 两条指向别的关系的公理，落库前必须过这里。
///
/// **最要紧的一条是同库。** 列上的外键是 `REFERENCES relation_types(id)`，
/// 它不认知识库——数据库层面，A 库的关系可以指向 B 库的关系。RDF 导入那条路
/// 天然过不去（按 IRI 在本库里查），而接口收的是裸 UUID：不在这里挡，
/// 拿到任意一个 UUID 就能让推理机跨库读公理。**不能靠前端只列本库选项**，
/// 那是界面礼貌，不是边界。
///
/// 另外两条：属性没有逆（它的宾语是字面值，反过来无从谈起），
/// 子属性不能是自己（DB 有 CHECK，但撞上去是 500，得在这里给出人话）。
/// 而**逆是自己允许**——那等于对称，R0 会提示改用 `symmetric` 更直白，不算错。
async fn validate_property_links(
    pool: &PgPool,
    kb_id: Uuid,
    self_id: Option<Uuid>,
    kind: &str,
    ax: RelationAxioms,
) -> AppResult<()> {
    let links = [ax.inverse_of, ax.sub_property_of];
    if links.iter().all(|l| l.is_none()) {
        return Ok(());
    }
    if kind == "attribute" {
        return Err(AppError::invalid(
            "attr_has_no_link",
            "An attribute cannot have an inverse or a super-property",
        ));
    }
    if self_id.is_some() && ax.sub_property_of == self_id {
        return Err(AppError::invalid(
            "sub_property_self",
            "A relation cannot be its own super-property",
        ));
    }
    for target in links.into_iter().flatten() {
        let ok: Option<(String,)> =
            sqlx::query_as("SELECT kind FROM relation_types WHERE id = $1 AND kb_id = $2")
                .bind(target)
                .bind(kb_id)
                .fetch_optional(pool)
                .await?;
        match ok {
            // 不区分「不存在」与「在别的库」：能问出哪个 UUID 存在于别处，
            // 本身就是一点不该给的信息
            None => {
                return Err(AppError::invalid(
                    "unknown_relation",
                    "That relation is not in this knowledge base",
                ));
            }
            Some((k,)) if k != "relation" => {
                return Err(AppError::invalid(
                    "link_target_is_attr",
                    "An attribute cannot be an inverse or a super-property",
                ));
            }
            Some(_) => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn create_relation_type(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    label: &str,
    temporal: &str,
    ax: RelationAxioms,
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
    // 新建的行 id 还不存在，指向自己无从谈起——所以 self_id 传 None
    validate_property_links(pool, kb_id, None, kind, ax).await?;
    let is_attr = kind == "attribute";
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO relation_types (id, kb_id, key, label, temporal,
                                     functional, inverse_functional, description,
                                     kind, datatype, unit,
                                     is_transitive, is_symmetric,
                                     is_asymmetric, is_irreflexive,
                                     inverse_of, sub_property_of)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                 $16, $17)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(temporal)
    .bind(ax.functional)
    .bind(ax.inverse_functional)
    .bind(description)
    .bind(kind)
    .bind(is_attr.then_some(datatype).flatten())
    .bind(is_attr.then_some(unit).flatten())
    .bind(ax.transitive)
    .bind(ax.symmetric)
    .bind(ax.asymmetric)
    .bind(ax.irreflexive)
    // 属性不带这两条（上面已经拦了非空的情况，这里是兜底）
    .bind(if is_attr { None } else { ax.inverse_of })
    .bind(if is_attr { None } else { ax.sub_property_of })
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
    ax: RelationAxioms,
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
    // 指向别的关系的两条要先问过库：目标在不在本库、是不是关系。
    // 只在真的填了的时候查——清空（两个都 None）没有目标可验
    if ax.inverse_of.is_some() || ax.sub_property_of.is_some() {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT kind FROM relation_types WHERE id = $1 AND kb_id = $2")
                .bind(id)
                .bind(kb_id)
                .fetch_optional(pool)
                .await?;
        let (kind,) = row.ok_or(AppError::NotFound)?;
        validate_property_links(pool, kb_id, Some(id), &kind, ax).await?;
    }
    // kind 不可变（改它会让存量事实语义错乱）。domain/range 可改——
    // 它们是签名不是身份，"这个属性也适用于承包商" 是个正当的编辑。
    // datatype/unit 只对 attribute 行生效，datatype 缺省保持原值
    //
    // 两条链**跟着六位公理一起覆盖式写**：缺省 = 清空，不是「不动」。
    // 与 `is_transitive` 那几位同一条规矩——它们是同一个表单里同时提交的
    // 一组声明，一半覆盖一半保留才是真正会出事的语义
    let res = sqlx::query(
        "UPDATE relation_types
            SET label = $3, temporal = $4,
                functional = $5, inverse_functional = $6, description = $7,
                datatype = CASE WHEN kind = 'attribute' AND $8 IS NOT NULL THEN $8 ELSE datatype END,
                unit = CASE WHEN kind = 'attribute' THEN $9 ELSE unit END,
                is_transitive = $10, is_symmetric = $11,
                is_asymmetric = $12, is_irreflexive = $13,
                inverse_of = $14, sub_property_of = $15
         WHERE id = $2 AND kb_id = $1",
    )
    .bind(kb_id)
    .bind(id)
    .bind(label)
    .bind(temporal)
    .bind(ax.functional)
    .bind(ax.inverse_functional)
    .bind(description)
    .bind(datatype)
    .bind(unit)
    .bind(ax.transitive)
    .bind(ax.symmetric)
    .bind(ax.asymmetric)
    .bind(ax.irreflexive)
    .bind(ax.inverse_of)
    .bind(ax.sub_property_of)
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
        // **被拒绝过的照样累加。**
        //
        // 从前这里带着 `WHERE dismissed_at IS NULL`，理由是"否则计数会把'不要'
        // 重新顶成一个待处理信号"。那个理由针对的是**呈现**，用的手段却是
        // **停止计数**——两件事被绑在一起了，代价是一次点击变成永久失明：
        // 第一篇里出现一次的说法被忽略掉，后面二十篇都在用它，计数仍停在 1，
        // 谁也不知道当初那个判断已经不成立，那批事实永远没有谓词。
        //
        // 用户是对**当时看得见的证据**做的判断，不是对所有时间。所以计数照记，
        // 抑制交给读取侧：`list_misses` 仍然只返回未忽略的，提案与自动扩本体
        // 一步没变；已忽略的连同更新后的计数走 `list_dismissed_misses`，
        // 在面板上单列一处，人看见涨到 40 了可以自己撤回
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
         WHERE kb_id = $1 AND dismissed_at IS NULL
         ORDER BY count DESC, updated_at DESC LIMIT 50",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 已被忽略的说法，连同**它此后继续累积的计数**。
///
/// 存在的理由是忽略这个动作曾经是单向门：点下去之后既不再呈现、也不再计数，
/// 于是"当时只出现过一次"这个判断依据一旦过期，没有任何人看得见。
/// 这个列表是那扇门上的窗——抑制照旧，但看得见抑制掉的是什么、现在有多重。
pub async fn list_dismissed_misses(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<OntologyMiss>> {
    Ok(sqlx::query_as(
        "SELECT kind, key, example, count FROM ontology_misses
         WHERE kb_id = $1 AND dismissed_at IS NOT NULL
         ORDER BY count DESC, updated_at DESC LIMIT 50",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 撤回一次忽略：这个说法重新进入提案与自动扩本体。
pub async fn restore_miss(pool: &PgPool, kb_id: Uuid, kind: &str, key: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE ontology_misses SET dismissed_at = NULL, updated_at = now()
         WHERE kb_id = $1 AND kind = $2 AND key = $3 AND dismissed_at IS NOT NULL",
    )
    .bind(kb_id)
    .bind(kind)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

/// 用户说"不要这个"。**标记而非删除**——删掉的话下一次抽取遇到同一个词
/// 原样插回来，用户的拒绝活不过一轮抽取。自动扩展路径也据此绕开。
///
/// 可撤回（见 [`restore_miss`]），且撤回之后计数是连续的——忽略期间照样在记。
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
         VALUES ($1, $2, $3, $4, $7, $8, $5, $6)",
    )
    .bind(id)
    .bind(kb_id)
    .bind(key)
    .bind(label)
    .bind(description)
    .bind(iri)
    // 按 key 取色而不是所有类一个灰蓝——导入一个大本体进来才有得看
    .bind(crate::palette::color_for_key(key))
    // 形状说明来历：这条路带 IRI，就是词表声明的
    .bind(crate::palette::shape_for(iri))
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
    /// 写进哪一组列。类有两份向量：整段（label + 描述）与只有 label 的那份，
    /// 分别服务长画像与短说法两种查询（见 `entity_types.label_embedding`）
    pub field: EmbedField,
}

/// 同一行的两份向量。**短查询比 Label，长画像比 Full**——查询分了两种形状，
/// 文档也得分两种，否则短查询会被同义反复的类接管（`Map\nA map.` 那一类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedField {
    Full,
    Label,
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
///
/// `only` 限定只补一半。类型消解只用类，让它等 1633 个关系嵌完是白等六分钟；
/// 而后台那个补齐任务不限，两边补的是同一批行，谁先跑到都算数。
pub async fn types_needing_embedding(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
    only: Option<TypeKind>,
) -> AppResult<Vec<TypeToEmbed>> {
    let mut out = Vec::new();
    if only == Some(TypeKind::Relation) {
        // 类型消解只用类，等 1633 个关系嵌完是白等六分钟
        return relations_needing_embedding(pool, kb_id, model).await;
    }
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
        field: EmbedField::Full,
    }));
    // 只嵌 label 的那一份（见 `entity_types.label_embedding`）。短查询走这个索引——查询分了两种形状，
    // 文档也得分两种，否则短查询被同义反复的类接管
    let labels: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, label FROM entity_types
         WHERE kb_id = $1
           AND (label_embedding IS NULL
                OR label_embedded_model IS DISTINCT FROM $2
                OR label_embedded_text IS DISTINCT FROM btrim(label))",
    )
    .bind(kb_id)
    .bind(model)
    .fetch_all(pool)
    .await?;
    out.extend(labels.into_iter().map(|(id, label)| TypeToEmbed {
        id,
        kind: TypeKind::Entity,
        text: label.trim().to_string(),
        field: EmbedField::Label,
    }));
    if only == Some(TypeKind::Entity) {
        return Ok(out);
    }
    out.extend(relations_needing_embedding(pool, kb_id, model).await?);
    Ok(out)
}

async fn relations_needing_embedding(
    pool: &PgPool,
    kb_id: Uuid,
    model: &str,
) -> AppResult<Vec<TypeToEmbed>> {
    let mut out = Vec::new();
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
        // 关系没有短查询那一路,只有整段这一份
        field: EmbedField::Full,
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
        // 两份向量各写各的列（见 `entity_types.label_embedding`）。列名前缀不同，其余一模一样
        let (vec_col, text_col, model_col) = match item.field {
            EmbedField::Full => ("embedding", "embedded_text", "embedded_model"),
            EmbedField::Label => (
                "label_embedding",
                "label_embedded_text",
                "label_embedded_model",
            ),
        };
        sqlx::query(&format!(
            "UPDATE {table} SET {vec_col} = $2, {text_col} = $3, {model_col} = $4
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
///
/// **既没有描述、又不在分类树上的类不参加。** 导入一份词表时，凡被
/// domainIncludes / rangeIncludes / equivalentClass 引用到的外部 IRI 都会建成一行
/// （OMG、UNECE、GS1…），它们没有标签正文、没有父也没有子——不是词汇，是悬空引用。
///
/// 它们偏偏很能赢：`embed_text` 在描述为空时退化成只嵌标签，于是这一行嵌的
/// 就是 "Location" 一个词。短的那一侧距离系统性地更小（同一条规律在这个文件
/// 和类型消解里已经栽过三次），于是一个空壳压过了带 83 字定义的
/// `administrative_area`——实测 `杭州拱墅区` 正是这么丢的。
///
/// 而且就算端上去也判不了：裁决看到的是 `- location (location)`，没有定义可依。
/// 装 schema.org 的库里这样的行有 50 个，43 个连一条继承边都没有。
///
/// **只是不当候选，不删行**：domain / range 仍然指着它们，删了会断引用。
pub async fn nearest_entity_types(
    pool: &PgPool,
    kb_id: Uuid,
    embedding: &[f32],
    limit: i64,
    // true = 比只有 label 的那份向量（见 `entity_types.label_embedding`）。短说法走这一路：**短对短**，
    // 否则 `district. place` 会被 `Map\nA map.` 这类同义反复的一行话赢过去
    by_label: bool,
) -> AppResult<Vec<TypeCandidate>> {
    let col = if by_label {
        "label_embedding"
    } else {
        "embedding"
    };
    let rows: Vec<(Uuid, String, String, String, f64)> = sqlx::query_as(&format!(
        "SELECT id, key, label, coalesce(description, ''), ({col} <=> $2)::float8
         FROM entity_types t
         WHERE t.kb_id = $1 AND t.{col} IS NOT NULL
           AND (coalesce(btrim(t.description), '') <> ''
                OR EXISTS (SELECT 1 FROM entity_type_parents p
                           WHERE p.child_id = t.id OR p.parent_id = t.id))
         ORDER BY {col} <=> $2
         LIMIT $3"
    ))
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

/// 一个属性声明的 datatype。改写字面值事实时要按它换算。
///
/// 以**库里这一条**为准而不是以请求为准：指向已有属性时请求里根本没有
/// datatype，而即便有，本体说了算。
pub async fn relation_type_datatype(pool: &PgPool, id: Uuid) -> AppResult<Option<String>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT datatype FROM relation_types WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(d,)| d))
}

/// 把一个 IRI 认到已有的**本地**类上（原本没有 IRI 的那种）。
///
/// **只写 IRI 与形状，不动 label、description、颜色。** 认领要解决的是
/// "这棵树是断的"，不是"用词汇表的说法覆盖用户的说法"：种子类的描述是照着
/// 抽取调过的、且跟库的语言走，而 schema.org 的描述是英文样板。覆盖它等于
/// 悄悄换掉抽取提示词里最承重的那一句。
///
/// **形状要跟着改，因为形状说的就是来历**（方=词表声明的，圆=语料里长的）。
/// 一个类被认领成"词表声明的"却还画成圆，画面就在说谎。实测过这个缝：
/// 往一个已有 `person` / `organization` 的库里导 schema.org，
/// 那几个类拿到了 IRI 却仍是圆的——有 IRI 却是圆的，自相矛盾。
///
/// 颜色不动：颜色是**身份**（同一个 key 永远同一个色），认领不改变它是谁。
/// 形状是**来历**，认领恰恰改变了这一点。
///
/// 只在 `iri IS NULL` 时写，所以重复导入是幂等的，也绝不会抢走另一个
/// 词汇表已经认领的类。
pub async fn adopt_iri_onto_key(
    pool: &PgPool,
    kb_id: Uuid,
    key: &str,
    iri: &str,
) -> AppResult<Option<Uuid>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "UPDATE entity_types SET iri = $3, shape = 'square'
         WHERE kb_id = $1 AND key = $2 AND iri IS NULL
         RETURNING id",
    )
    .bind(kb_id)
    .bind(key)
    .bind(iri)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// 与给定向量最近的若干**类 id**（只回 id，调用方手里已有类的全量数据）。
///
/// 给抽取用：分块向量在抽取循环里本来就有（实体消解在用），拿它检索出这一块
/// 可能用得上的类，只把这些铺进提示词。
pub async fn nearest_entity_type_ids(
    pool: &PgPool,
    kb_id: Uuid,
    embedding: &[f32],
    limit: i64,
) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM entity_types
         WHERE kb_id = $1 AND embedding IS NOT NULL
         ORDER BY embedding <=> $2
         LIMIT $3",
    )
    .bind(kb_id)
    .bind(Vector::from(embedding.to_vec()))
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 同上，关系与属性。`only_kind` 分道：关系清单与属性清单在提示词里是两段。
pub async fn nearest_relation_type_ids(
    pool: &PgPool,
    kb_id: Uuid,
    embedding: &[f32],
    limit: i64,
    only_kind: Option<&str>,
) -> AppResult<Vec<Uuid>> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM relation_types
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
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 一次插完一批类，返回 key → id。
///
/// **存在的理由是 fsync。** 逐条 `execute(pool)` 每条各自提交，导入 schema.org
/// 那种量级（968 个类加约 1500 个属性）是五千次 fsync，实测 45 秒；同样的行数
/// 放进一条语句是 536 毫秒。差的不是往返，是提交。
///
/// `ON CONFLICT DO NOTHING` 而不是报错：撞 key 的处置在计划阶段已经判过了
/// （[`crate::ontology::create_entity_type_with_iri`] 的注释讲了为什么不覆盖），
/// 这里只是把计划落地，撞上说明计划与库不同步，跳过并让调用方从返回的 map 里
/// 发现少了谁。
pub async fn create_entity_types_bulk(
    pool: &PgPool,
    kb_id: Uuid,
    rows: &[(String, String, String, String)],
) -> AppResult<std::collections::HashMap<String, Uuid>> {
    if rows.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    for (key, ..) in rows {
        validate_key(key)?;
    }
    let keys: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    let labels: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    let descs: Vec<&str> = rows.iter().map(|r| r.2.as_str()).collect();
    let iris: Vec<&str> = rows.iter().map(|r| r.3.as_str()).collect();
    // 颜色在 Rust 侧按 key 算好，跟着 UNNEST 一起进去——
    // SQL 里调不到 Rust 函数，而这一批正是导入大本体走的路
    let colours: Vec<&str> = keys
        .iter()
        .map(|k| crate::palette::color_for_key(k))
        .collect();
    let shapes: Vec<&str> = iris.iter().map(|i| crate::palette::shape_for(i)).collect();
    let out: Vec<(Uuid, String)> = sqlx::query_as(
        "INSERT INTO entity_types (id, kb_id, key, label, color, shape, description, iri)
         SELECT gen_random_uuid(), $1, k, l, c, s, d, i
         FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[])
              AS t(k, l, d, i, c, s)
         ON CONFLICT (kb_id, key) DO NOTHING
         RETURNING id, key",
    )
    .bind(kb_id)
    .bind(&keys)
    .bind(&labels)
    .bind(&descs)
    .bind(&iris)
    .bind(&colours)
    .bind(&shapes)
    .fetch_all(pool)
    .await?;
    Ok(out.into_iter().map(|(id, k)| (k, id)).collect())
}

/// 批量建关系/属性时的一行。
///
/// **`functional` / `inverse_functional` 必须照词汇表写下去，不能默认 false。**
/// 它们是时态引擎自动闭合事实的依据，猜错会成批造假冲突——`part_of` 被误标成
/// functional 那次积了 59 条。
pub struct BulkRelation {
    pub key: String,
    pub label: String,
    pub description: String,
    pub iri: String,
    /// `relation` 或 `attribute`
    pub kind: &'static str,
    /// 只对属性有意义，关系传 `None`
    pub datatype: Option<String>,
    pub functional: bool,
    pub inverse_functional: bool,
    /// OWL 属性公理,一致性检查的判定依据（0002 R0）。
    /// 同样必须照词汇表写下去——`alias_of` 双向是对的、`produces` 双向是错的,
    /// 分开这两者的只能是本体
    pub transitive: bool,
    pub symmetric: bool,
    pub asymmetric: bool,
    pub irreflexive: bool,
}

/// 一次插完一批关系或属性，返回 key → id。语义同 [`create_entity_types_bulk`]。
///
/// `kind` 决定走关系通道还是属性通道；`datatype` 只对属性有意义，关系传 `None`。
/// `temporal` 固定 `state`——OWL 里没有对应概念，猜 event 或 eternal 都更差
/// （与单条版的判断一致）。
pub async fn create_relation_types_bulk(
    pool: &PgPool,
    kb_id: Uuid,
    rows: &[BulkRelation],
) -> AppResult<std::collections::HashMap<String, Uuid>> {
    if rows.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    for r in rows {
        validate_key(&r.key)?;
    }
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
    let descs: Vec<&str> = rows.iter().map(|r| r.description.as_str()).collect();
    let iris: Vec<&str> = rows.iter().map(|r| r.iri.as_str()).collect();
    let kinds: Vec<&str> = rows.iter().map(|r| r.kind).collect();
    let dts: Vec<Option<&str>> = rows.iter().map(|r| r.datatype.as_deref()).collect();
    let funcs: Vec<bool> = rows.iter().map(|r| r.functional).collect();
    let invs: Vec<bool> = rows.iter().map(|r| r.inverse_functional).collect();
    let trans: Vec<bool> = rows.iter().map(|r| r.transitive).collect();
    let syms: Vec<bool> = rows.iter().map(|r| r.symmetric).collect();
    let asyms: Vec<bool> = rows.iter().map(|r| r.asymmetric).collect();
    let irrefs: Vec<bool> = rows.iter().map(|r| r.irreflexive).collect();
    let out: Vec<(Uuid, String)> = sqlx::query_as(
        "INSERT INTO relation_types
             (id, kb_id, key, label, temporal, functional, inverse_functional,
              description, kind, datatype, iri,
              is_transitive, is_symmetric, is_asymmetric, is_irreflexive)
         SELECT gen_random_uuid(), $1, k, l, 'state', fu, iv, d, kind, dt, i,
                tr, sy, asym, irr
         FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[],
                     $8::bool[], $9::bool[], $10::bool[], $11::bool[], $12::bool[], $13::bool[])
              AS t(k, l, d, i, kind, dt, fu, iv, tr, sy, asym, irr)
         ON CONFLICT (kb_id, key) DO NOTHING
         RETURNING id, key",
    )
    .bind(kb_id)
    .bind(&keys)
    .bind(&labels)
    .bind(&descs)
    .bind(&iris)
    .bind(&kinds)
    .bind(&dts)
    .bind(&funcs)
    .bind(&invs)
    .bind(&trans)
    .bind(&syms)
    .bind(&asyms)
    .bind(&irrefs)
    .fetch_all(pool)
    .await?;
    Ok(out.into_iter().map(|(id, k)| (k, id)).collect())
}

/// 一次写完一批关系的 domain / range。
///
/// 单条版 [`set_domains_ranges`] 内部已经用 unnest 省了往返，但它对**每条关系**
/// 都要跑 4 条语句（两张表各一次 DELETE 一次 INSERT）。1500 条关系就是 6000 次
/// 独立提交，而提交才是代价。这里把所有关系的关联行摊平成两条语句。
///
/// **不 DELETE**：调用方是刚建出来的新关系，关联表上不可能有旧行。
/// 更新既有关系仍然走单条版。
pub async fn link_domains_ranges_bulk(
    pool: &PgPool,
    domains: &[(Uuid, Uuid)],
    ranges: &[(Uuid, Uuid)],
) -> AppResult<()> {
    for (table, pairs) in [
        ("relation_type_domains", domains),
        ("relation_type_ranges", ranges),
    ] {
        if pairs.is_empty() {
            continue;
        }
        let rels: Vec<Uuid> = pairs.iter().map(|p| p.0).collect();
        let types: Vec<Uuid> = pairs.iter().map(|p| p.1).collect();
        sqlx::query(&format!(
            "INSERT INTO {table} (relation_type_id, entity_type_id)
             SELECT r, t FROM UNNEST($1::uuid[], $2::uuid[]) AS x(r, t)
             ON CONFLICT DO NOTHING"
        ))
        .bind(&rels)
        .bind(&types)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 一次设完一批类的父类。语义同 [`set_parents`]，但**不查环**。
///
/// 单条版每次都要跑一个递归 CTE 查环，一千个类就是一千次递归查询。
/// 这里只用于导入新建的类：环是上游词汇表的问题，而 `set_parents` 对环的处置
/// 本来也只是跳过那一条（`let _ =`），不是中断导入。导入完之后本体页
/// 仍然能发现并让人处理。
pub async fn set_parents_bulk(pool: &PgPool, pairs: &[(Uuid, Uuid)]) -> AppResult<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let children: Vec<Uuid> = pairs.iter().map(|p| p.0).collect();
    let parents: Vec<Uuid> = pairs.iter().map(|p| p.1).collect();
    sqlx::query(
        "INSERT INTO entity_type_parents (child_id, parent_id)
         SELECT c, p FROM UNNEST($1::uuid[], $2::uuid[]) AS x(c, p)
         WHERE c <> p
         ON CONFLICT DO NOTHING",
    )
    .bind(&children)
    .bind(&parents)
    .execute(pool)
    .await?;
    Ok(())
}

/// 类互斥落库（见 `relation_types` 的公理列）。语义同 [`set_parents_bulk`]。
///
/// **两个方向都写。** 解析侧已经把 `owl:disjointWith` 的对称性展开成两条,
/// 这里照写即可——查"A 与 B 互斥吗"因此不必关心从哪一头问。
///
/// `a <> b` 挡自指:自己跟自己互斥是无意义的声明，而它会让一致性检查
/// 把每个实体都报成矛盾。表上也有同样的 CHECK，两道都留着——
/// 约束是最后一道，过滤在这里是为了不让一整批插入因为一条脏数据整个失败。
pub async fn set_disjoint_bulk(
    pool: &PgPool,
    kb_id: Uuid,
    pairs: &[(Uuid, Uuid)],
) -> AppResult<()> {
    if pairs.is_empty() {
        return Ok(());
    }
    let a: Vec<Uuid> = pairs.iter().map(|p| p.0).collect();
    let b: Vec<Uuid> = pairs.iter().map(|p| p.1).collect();
    sqlx::query(
        "INSERT INTO entity_type_disjoint (kb_id, a_id, b_id)
         SELECT $1, x, y FROM UNNEST($2::uuid[], $3::uuid[]) AS t(x, y)
         WHERE x <> y
         ON CONFLICT DO NOTHING",
    )
    .bind(kb_id)
    .bind(&a)
    .bind(&b)
    .execute(pool)
    .await?;
    Ok(())
}

/// 把 `others` 设成这个类的**全部**互斥对象（不在其中的解除）。
///
/// 与 [`set_disjoint_bulk`] 的分工：那个是导入用的「只增不减」，这个是编辑用的
/// 「这就是全部」。编辑必须能取消——否则界面上取消勾选没有任何效果，
/// 而用户会以为自己改了。
///
/// 两个方向各写一行，与导入侧一致：查「A 与 B 互斥吗」因此不必关心从哪头问。
pub async fn set_disjoint_for(
    pool: &PgPool,
    kb_id: Uuid,
    class: Uuid,
    others: &[Uuid],
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    // 先清掉这个类参与的全部互斥边——两个方向都要清，因为它两边都可能出现
    sqlx::query(
        "DELETE FROM entity_type_disjoint
          WHERE kb_id = $1 AND (a_id = $2 OR b_id = $2)",
    )
    .bind(kb_id)
    .bind(class)
    .execute(&mut *tx)
    .await?;
    for other in others {
        if *other == class {
            continue;
        }
        sqlx::query(
            "INSERT INTO entity_type_disjoint (kb_id, a_id, b_id)
             VALUES ($1, $2, $3), ($1, $3, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(kb_id)
        .bind(class)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 本体提案（见 `ontology_proposals`）
// ---------------------------------------------------------------------------

/// 一条落库的提案。`payload` 是接口原样返回的那一条。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredProposal {
    pub section: String,
    pub key: String,
    pub payload: serde_json::Value,
}

/// 把一轮 Suggest 的结果写下来。
///
/// **已经有人表过态的不动。** `WHERE status = 'open'` 那一句是这个函数的全部要点：
/// 重跑 Suggest 会再次算出被拒绝过的那条提案（原材料还在 `ontology_misses` 里），
/// 不加这句它就会被刷回 open——等于每跑一次都把人的否决抹掉一次。
pub async fn save_proposals(
    pool: &PgPool,
    kb_id: Uuid,
    items: &[(String, String, serde_json::Value)],
) -> AppResult<()> {
    for (section, key, payload) in items {
        sqlx::query(
            "INSERT INTO ontology_proposals (id, kb_id, section, key, payload)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (kb_id, section, key) DO UPDATE
               SET payload = EXCLUDED.payload, created_at = now()
               WHERE ontology_proposals.status = 'open'",
        )
        .bind(Uuid::now_v7())
        .bind(kb_id)
        .bind(section)
        .bind(key)
        .bind(payload)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// 还等着人看的提案。新的排前面——旧的那批已经被看过好几眼了。
pub async fn open_proposals(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<StoredProposal>> {
    Ok(sqlx::query_as(
        "SELECT section, key, payload FROM ontology_proposals
         WHERE kb_id = $1 AND status = 'open'
         ORDER BY created_at DESC, key",
    )
    .bind(kb_id)
    .fetch_all(pool)
    .await?)
}

/// 一条提案被采纳或拒绝了。
///
/// **改状态而不是删行**：采纳发生过、拒绝也发生过。跟 `fact_adoptions`、
/// `entity_retypes` 同一条路。拒绝留痕还有个当下就用得着的作用——下一轮
/// Suggest 不会把它刷回待看。
pub async fn decide_proposal(
    pool: &PgPool,
    kb_id: Uuid,
    section: &str,
    key: &str,
    status: &str,
    actor: Uuid,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE ontology_proposals
            SET status = $4, decided_by = $5, decided_at = now()
          WHERE kb_id = $1 AND section = $2 AND key = $3 AND status = 'open'",
    )
    .bind(kb_id)
    .bind(section)
    .bind(key)
    .bind(status)
    .bind(actor)
    .execute(pool)
    .await?;
    Ok(())
}

/// 还有多少条等着看。0003 的缺口：关掉自动扩展开关之后没有「自上次以来有 N 条」
/// 的提醒，信号在面板里但没人主动看——有了这张表，提醒就是这一句。
pub async fn open_proposal_count(pool: &PgPool, kb_id: Uuid) -> AppResult<i64> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM ontology_proposals WHERE kb_id = $1 AND status = 'open'",
    )
    .bind(kb_id)
    .fetch_one(pool)
    .await?;
    Ok(n)
}

/// 主语的类型合不合这个关系声明的 domain。沿继承链往上找。
///
/// **只回答，不动数据。** 0001 已经判过：签名是提示不是闸门，
/// 「用可能错的声明驱动自动动作风险高」。而实体类型本身也是模型判出来的
/// （实测里 Elon Musk 被判成 `researcher`），拿它去翻转事实方向，
/// 是两层不确定叠在一起还静默改写。所以这里的结果只用来落一条信号。
///
/// 三种回答，别混成两种：
/// - `Some(true)`  合
/// - `Some(false)` 不合——**这才是信号**
/// - `None`        没得判（关系没声明 domain，或实体还没有类型）
pub async fn subject_fits_domain(
    pool: &PgPool,
    relation_type_id: Uuid,
    subject_type_id: Option<Uuid>,
) -> AppResult<Option<bool>> {
    let Some(subject_type_id) = subject_type_id else {
        return Ok(None);
    };
    let (declared, ok): (i64, i64) = sqlx::query_as(
        "WITH RECURSIVE up(id) AS (
             SELECT $2::uuid
             UNION
             SELECT p.parent_id FROM entity_type_parents p JOIN up ON p.child_id = up.id
         )
         SELECT (SELECT count(*) FROM relation_type_domains WHERE relation_type_id = $1),
                (SELECT count(*) FROM relation_type_domains d
                   JOIN up ON up.id = d.entity_type_id
                  WHERE d.relation_type_id = $1)",
    )
    .bind(relation_type_id)
    .bind(subject_type_id)
    .fetch_one(pool)
    .await?;
    Ok((declared > 0).then_some(ok > 0))
}

/// 这些类的全部祖先（不含自己）。沿 `subClassOf` 上溯，多继承与菱形都走得通。
///
/// 给按块检索的候选补地板用：向量检索偏爱字面出现在正文里的叶子类，
/// 泛化基类排得很后（实测 `person` 在 976 个类里排第 359），于是提示词里
/// 没有它们——实体无处落脚，关系签名也退化成 `*`。祖先是本体自己声明的
/// 泛化关系，拿它补比维护一张「通用类」清单可靠。
pub async fn ancestors_of(pool: &PgPool, ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // 递归项里 UNION 去重，菱形继承不会把同一个祖先展开两次
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "WITH RECURSIVE up(id) AS (
             SELECT unnest($1::uuid[])
             UNION
             SELECT p.parent_id FROM entity_type_parents p JOIN up ON p.child_id = up.id
         )
         SELECT id FROM up WHERE id <> ALL($1::uuid[])",
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// 同 [`subject_fits_domain`]，但类型**从库里的实体读**，不靠调用方手上那份。
///
/// 抽取器手上的 `entity_type_of` 只覆盖模型在这一块里声明过的实体；宾语常常
/// 是别处已经存在的实体，这一块没重新声明它的类型，于是查不到、判不了。
/// 而消解已经把它连到了库里那一行——那里有类型，用它才判得全。
pub async fn entity_fits_domain(
    pool: &PgPool,
    relation_type_id: Uuid,
    entity_id: Uuid,
) -> AppResult<Option<bool>> {
    let (declared, ok): (i64, i64) = sqlx::query_as(
        "WITH RECURSIVE up(id) AS (
             SELECT type_id FROM entities WHERE id = $2
             UNION
             SELECT p.parent_id FROM entity_type_parents p JOIN up ON p.child_id = up.id
         )
         SELECT (SELECT count(*) FROM relation_type_domains WHERE relation_type_id = $1),
                (SELECT count(*) FROM relation_type_domains d
                   JOIN up ON up.id = d.entity_type_id
                  WHERE d.relation_type_id = $1)",
    )
    .bind(relation_type_id)
    .bind(entity_id)
    .fetch_one(pool)
    .await?;
    Ok((declared > 0).then_some(ok > 0))
}

/// 一条 (主语, 谓词, 宾语) 对着谓词的 domain 签名该怎么落。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// 谓词没声明 domain——没有判据，照原样落
    Unchecked,
    /// 主语符合
    Keep,
    /// 主语不符合、宾语符合：按签名对调主宾
    Swap,
    /// 两边都不符合：这个关系不适用于这对实体，谓词该留空
    Neither,
}

/// **三条写谓词的路共用的那一道判断**（#190 / #196）：抽取落新事实、采纳把谓词
/// 挂回旧事实、合并换掉主语——从前只有抽取查，另外两条各自绕了过去。
///
/// 判据刻意窄（0012）：只看 domain，只在**主语违反且宾语符合**时对调，两边都对不上
/// 就留空谓词。参数顺序不是关于世界的断言，是这个 key 的编码约定，所以本体在这一处
/// 是执法的；哪些类型能参与仍是引导，不在这里裁。
pub async fn judge_direction(
    pool: &PgPool,
    relation_type_id: Uuid,
    subject_id: Uuid,
    object_id: Uuid,
) -> AppResult<Fit> {
    match entity_fits_domain(pool, relation_type_id, subject_id).await? {
        None => Ok(Fit::Unchecked),
        Some(true) => Ok(Fit::Keep),
        Some(false) => match entity_fits_domain(pool, relation_type_id, object_id).await? {
            Some(true) => Ok(Fit::Swap),
            _ => Ok(Fit::Neither),
        },
    }
}

/// 把 `owl:inverseOf` / `rdfs:subPropertyOf` 从 IRI 解析成 id。
///
/// **必须是第二遍。** 这两条指的是另一个关系类型，而 id 要等全部插完才有——
/// 一遍过的写法只能处理「父属性恰好排在前面」的文件，而 RDF 三元组没有顺序。
///
/// 按 IRI 配对而不是按 key：key 会因为撞名加后缀（`part_of_2`），
/// 而 IRI 是这份本体里的身份。
pub async fn link_property_axioms_bulk(
    pool: &PgPool,
    kb_id: Uuid,
    inverse: &[(String, String)],
    sub_property: &[(String, String)],
) -> AppResult<(u64, u64)> {
    let run = |column: &'static str, pairs: &[(String, String)]| {
        let src: Vec<String> = pairs.iter().map(|(s, _)| s.clone()).collect();
        let dst: Vec<String> = pairs.iter().map(|(_, d)| d.clone()).collect();
        async move {
            if src.is_empty() {
                return AppResult::Ok(0);
            }
            // 目标 IRI 在这个库里找不到就跳过这一条——**部分导入是常态**
            // （引用了外部词汇表里的属性），一条连不上不该让整次导入失败
            let sql = format!(
                "UPDATE relation_types r SET {column} = t.id
                   FROM UNNEST($2::text[], $3::text[]) AS p(src, dst)
                   JOIN relation_types t ON t.kb_id = $1 AND t.iri = p.dst
                  WHERE r.kb_id = $1 AND r.iri = p.src AND r.id <> t.id"
            );
            let n = sqlx::query(&sql)
                .bind(kb_id)
                .bind(&src)
                .bind(&dst)
                .execute(pool)
                .await?
                .rows_affected();
            AppResult::Ok(n)
        }
    };
    let inv = run("inverse_of", inverse).await?;
    let sub = run("sub_property_of", sub_property).await?;
    Ok((inv, sub))
}
