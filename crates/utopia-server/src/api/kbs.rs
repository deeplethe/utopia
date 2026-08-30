use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::{KnowledgeBase, Role, User};
use utopia_core::AppError;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateKbReq {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    /// 预置本体包的 id，按给定顺序装。空 = 只有十个种子。
    ///
    /// **顺序有意义**：第一个包的类会认领同名的种子类（它们没有 IRI），
    /// 后面的包撞名时查对齐表。schema.org 放第一个，别的包才对得上。
    #[serde(default)]
    pub ontology_packs: Vec<String>,
}

#[derive(Deserialize)]
pub struct UpdateKbReq {
    pub name: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<String>,
    /// 自动扩本体开关（缺省开；关掉不影响"留意"，只是变成你点一下的提案）
    #[serde(default)]
    pub auto_extend_ontology: Option<bool>,
    /// 本体语言（`en` | `zh`）：跟语料走，不跟界面走。见 docs/decisions/0004
    #[serde(default)]
    pub ontology_lang: Option<String>,
}

/// 用户可见的 KB 列表（restricted 库仅矩阵成员与系统管理员可见）。
pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<Vec<KnowledgeBase>>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Viewer)
        .await?;
    let list =
        utopia_store::kbs::list_visible(&state.pool, workspace_id, user.id, user.is_admin).await?;
    Ok(Json(list))
}

/// 建库：部署管理员（工作区 Admin+ 或系统管理员）。创建者自动进入矩阵为 admin。
pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateKbReq>,
) -> ApiResult<Json<KnowledgeBase>> {
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(AppError::invalid("bad_name", "Name must be 1-64 characters").into());
    }
    let kind = req.kind.as_deref().unwrap_or("knowledge");
    if !matches!(kind, "knowledge" | "memory") {
        return Err(AppError::Validation("kind must be 'knowledge' or 'memory'".into()).into());
    }
    // 建库是部署管理动作（入口在 System settings）：系统管理员或工作区 Admin+。
    // 用户自建库前端默认 restricted，不污染全员切换器；General 由系统初建保持 open
    let ws_role =
        utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Viewer)
            .await?;
    if !user.is_admin && ws_role < Role::Admin {
        return Err(AppError::Forbidden.into());
    }
    let kb = utopia_store::kbs::create(
        &state.pool,
        workspace_id,
        name,
        kind,
        req.description.as_deref(),
    )
    .await?;
    if let Some(v) = req.visibility.as_deref() {
        utopia_store::kbs::update(&state.pool, kb.id, None, None, Some(v), None, None).await?;
    }
    utopia_store::access::set_kb_member(&state.pool, kb.id, user.id, "admin", Some(user.id))
        .await?;
    install_packs(&state, kb.id, user.id, &req.ontology_packs).await?;
    let kb = utopia_store::kbs::get(&state.pool, kb.id).await?;
    Ok(Json(kb))
}

/// 我的知识库全景（账户层）：可见库 + 我的角色 + 加入信息 + 概览统计。
pub async fn my_kbs(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Viewer)
        .await?;
    let kbs =
        utopia_store::kbs::list_visible(&state.pool, workspace_id, user.id, user.is_admin).await?;
    let ids: Vec<Uuid> = kbs.iter().map(|k| k.id).collect();
    let infos = utopia_store::access::my_kb_infos(&state.pool, &ids, user.id).await?;
    let mut rows = Vec::with_capacity(kbs.len());
    for kb in &kbs {
        let info = infos.iter().find(|i| i.kb_id == kb.id);
        let role = utopia_store::access::kb_role(&state.pool, &user, kb).await?;
        rows.push(json!({
            "kb": kb,
            "my_role": role.map(|r| r.as_str()),
            "joined_at": info.and_then(|i| i.joined_at),
            "added_by_name": info.and_then(|i| i.added_by_name.clone()),
            "doc_count": info.map(|i| i.doc_count).unwrap_or(0),
            "member_count": info.map(|i| i.member_count).unwrap_or(0),
        }));
    }
    Ok(Json(json!({ "kbs": rows })))
}

async fn kb_with_role(
    state: &AppState,
    user: &User,
    kb_id: Uuid,
    min: Role,
) -> Result<KnowledgeBase, AppError> {
    utopia_store::access::require_kb(&state.pool, user, kb_id, min).await
}

/// 详情附带调用者在本库的角色：前端据此门控破坏性操作（重建/删除）的入口，
/// 不必让用户点到底才吃 403。
pub async fn get_one(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let kb = kb_with_role(&state, &user, id, Role::Viewer).await?;
    let role = utopia_store::access::kb_role(&state.pool, &user, &kb).await?;
    let mut body = serde_json::to_value(&kb).map_err(|e| AppError::Other(e.into()))?;
    body["my_role"] = json!(role.map(|r| r.as_str()));
    Ok(Json(body))
}

/// 库设置（名称/描述/可见性）：库 admin 起步。
pub async fn update(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateKbReq>,
) -> ApiResult<Json<KnowledgeBase>> {
    kb_with_role(&state, &user, id, Role::Admin).await?;
    let kb = utopia_store::kbs::update(
        &state.pool,
        id,
        req.name.as_deref().map(str::trim),
        req.description.as_deref(),
        req.visibility.as_deref(),
        req.auto_extend_ontology,
        req.ontology_lang.as_deref(),
    )
    .await?;
    // 审计只记不阻断
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(id),
        user.id,
        "kb.updated",
        "kb",
        Some(id),
        json!({ "name": req.name, "visibility": req.visibility }),
    )
    .await;
    Ok(Json(kb))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let kb = kb_with_role(&state, &user, id, Role::Admin).await?;
    utopia_store::kbs::delete(&state.pool, id).await?;
    // kb_id 置 NULL：库已级联删除，事件留在部署层（actor 与库名在 detail）
    let _ = utopia_store::audit::record(
        &state.pool,
        None,
        user.id,
        "kb.deleted",
        "kb",
        Some(id),
        json!({ "name": kb.name }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// KB 成员矩阵（库自己的 Settings → Members）
// ---------------------------------------------------------------------------

pub async fn members(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    kb_with_role(&state, &user, id, Role::Admin).await?;
    let members = utopia_store::access::kb_members(&state.pool, id).await?;
    Ok(Json(json!({ "members": members })))
}

#[derive(Deserialize)]
pub struct SetMemberReq {
    /// viewer | editor | admin
    pub role: String,
}

pub async fn set_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SetMemberReq>,
) -> ApiResult<Json<serde_json::Value>> {
    kb_with_role(&state, &user, id, Role::Admin).await?;
    utopia_store::access::set_kb_member(&state.pool, id, user_id, &req.role, Some(user.id)).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(id),
        user.id,
        "kb.member_set",
        "user",
        Some(user_id),
        json!({ "role": req.role }),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn remove_member(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    kb_with_role(&state, &user, id, Role::Admin).await?;
    utopia_store::access::remove_kb_member(&state.pool, id, user_id).await?;
    let _ = utopia_store::audit::record(
        &state.pool,
        Some(id),
        user.id,
        "kb.member_removed",
        "user",
        Some(user_id),
        json!({}),
    )
    .await;
    Ok(Json(json!({ "ok": true })))
}

/// 库级审计日志（Admin 起步；纯审计展示）。
pub async fn audit_log(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    kb_with_role(&state, &user, id, Role::Admin).await?;
    let events = utopia_store::audit::list_for_kb(&state.pool, id, 100).await?;
    Ok(Json(json!({ "events": events })))
}
/// 建库时装选中的本体包。
///
/// 从前这里要先跑一次 `ensure_default_ontology`：包里的类要认领同名的种子类
/// （`schema:Organization` 接管 `organization`），而认领的前提是那一行已经存在。
/// **现在没有种子可认领了**——0009 删掉内置实体类、0010 与 `#125` 删掉种子关系、
/// 0011 把 `mapped_to` 搬去语义层之后，播种函数本身也退场了。包直接落进空库。
///
/// **一个包失败不回滚已装的**：本体是加法，装了一半的库仍然可用，
/// 而回滚要撤已经建好的类——那正是 0008 决定不做导入撤销的理由。
/// 失败信息里带上是哪个包，让人知道从哪补。
async fn install_packs(
    state: &AppState,
    kb_id: Uuid,
    actor: Uuid,
    pack_ids: &[String],
) -> ApiResult<()> {
    if pack_ids.is_empty() {
        return Ok(());
    }
    for id in pack_ids {
        let pack = crate::ontology_packs::get(id)
            .ok_or_else(|| AppError::invalid("unknown_pack", format!("未知的本体包：{id}")))?;
        let bytes = crate::ontology_packs::bytes(pack)?;
        crate::owl_import::apply(state, kb_id, actor, pack.filename, &bytes)
            .await
            .map_err(|e| AppError::Other(anyhow::anyhow!("装本体包 {} 失败：{e}", pack.id)))?;
    }
    Ok(())
}

/// 可选的本体包清单，给建库界面。不需要登录之外的权限——它是静态数据。
pub async fn list_packs(AuthUser(_): AuthUser) -> ApiResult<Json<serde_json::Value>> {
    let packs: Vec<_> = crate::ontology_packs::PACKS
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "summary": p.summary,
                "classes": p.classes,
                "properties": p.properties,
            })
        })
        .collect();
    Ok(Json(json!({ "packs": packs })))
}
