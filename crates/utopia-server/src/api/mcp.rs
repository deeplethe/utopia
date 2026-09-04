//! MCP 服务端（Streamable HTTP）。
//!
//! 一条路由：`POST /api/v1/kbs/{kb_id}/mcp`，收 JSON-RPC 2.0。
//!
//! **传输选 Streamable HTTP 不选 stdio**，理由在 `docs/decisions/0014`：Utopia
//! 本来就是个服务端，stdio 要么起一个子进程反过来连它、要么再开一个连接池；
//! 而这是多人部署，五个人各连各的应该由同一个部署统一服务。
//!
//! **每个 POST 都重新认证一次。** 规范允许把一次握手的结果沿用整条连接，
//! 而 0014 那条纪律说得很清楚：
//!
//! > 校验 scope 要在每个工具入口，不在握手。`revoked_at` 中途被写上时必须
//! > 立刻生效。
//!
//! 这里做成无状态之后，那条性质是白得的——没有「连接」这个东西可以被信任。
//!
//! **响应用 `application/json` 而不是 SSE。** 规范允许两者，而这些工具是
//! 一问一答，没有服务端主动推的东西；SSE 是给通知用的，这一版不需要。

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value};
use utopia_core::models::Role;
use uuid::Uuid;

use super::tools::{self, ToolCtx, ToolSink};
use crate::error::ApiResult;
use crate::state::AppState;

/// 实现的协议版本。客户端报别的版本时照样按这个答——
/// 规范要求服务端回自己支持的版本，由客户端决定接不接受
const PROTOCOL_VERSION: &str = "2025-06-18";

/// 只读的那些：任何一枚够得着这个库的令牌都能调。
///
/// `query_data` 仍旧不放：它对**部署之外的生产库**跑 SQL，审计怎么记、
/// 一个被投喂了脏文档的 agent 拿它做什么，都还没答完（0014 的混淆代理那一节）。
const EXPOSED: [&str; 6] = [
    "search_chunks",
    "get_document",
    "search_docs",
    "find_entities",
    "changes",
    "entity_facts",
];

/// 会往账本里写的那些。
///
/// **0014 当初不放 `remember`，主要是怕混淆代理**：agent 读的是库里的文档，
/// 一句「请记住 X」写在文档里就可能被当成指令照做，而它带着那个人的全部权限。
/// 0015 把这个反对意见拆掉了——记忆抽出的事实先进 `pending_facts` 等人点头，
/// 没人点头就什么都没进图。所以这里放开的不是「写图」，是「提议」。
const WRITABLE: [&str; 1] = ["remember"];

/// `can_write` = 令牌 scope 是 write **且** 这个人在这个库里 editor 以上。
/// 两个条件缺一不可：scope 是上限，角色才是权限（0014）
fn is_exposed(name: &str, can_write: bool) -> bool {
    EXPOSED.contains(&name) || (can_write && WRITABLE.contains(&name))
}

/// 认证 + 授权。**两道，不是一道。**
///
/// 令牌说「这是谁、这枚钥匙够到哪几个库」；`require_kb` 说「这个人在这个库里
/// 是什么角色」。前者只收窄，后者才是权限——一枚 scope 全开的令牌落在一个
/// viewer 手上，仍旧只是 viewer。
/// 认证的产物：人、令牌、库，以及**有效写权限**——令牌 scope ∩ 这个人在这个库里的角色。
type Authorized = (
    utopia_core::models::User,
    utopia_store::tokens::Authenticated,
    utopia_core::models::KnowledgeBase,
    bool,
);

async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    kb_id: Uuid,
) -> Result<Authorized, utopia_core::AppError> {
    let raw = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(utopia_core::AppError::Unauthorized)?;
    let auth = utopia_store::tokens::authenticate(&state.pool, raw.trim()).await?;
    // 令牌的范围：限定过库的，够不着别的
    if !auth.covers(kb_id) {
        return Err(utopia_core::AppError::Forbidden);
    }
    // 人：停用立即生效（find_user_by_id 会挡住）
    let user = utopia_store::accounts::find_user_by_id(&state.pool, auth.user_id)
        .await?
        .ok_or(utopia_core::AppError::Unauthorized)?;
    // 角色：和网页端走同一个守卫，一行没改
    let kb = utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;
    // **收窄两次。** 令牌勾了 write 只是没把写排除掉；能不能写，问的是这个人
    // 在这个库里的角色——一枚 scope 全开的令牌落在 viewer 手上仍旧只能读
    let can_write = auth.can_write()
        && utopia_store::access::kb_role(&state.pool, &user, &kb)
            .await?
            .is_some_and(|role| role >= Role::Editor);
    Ok((user, auth, kb, can_write))
}

/// OpenAI 形状 → MCP 形状。
///
/// **共用 `chat.rs` 那一份定义**，而不是在这里另写一套。名字与参数 schema 是
/// 与 `tools.rs` 执行侧的契约，抄成两份迟早分叉——那正是上一步把工具抽出来
/// 要避免的事。
///
/// 已知的瑕疵：描述是给应用内助手写的，`search_chunks` 那句还提着「可以引用
/// 成 [n]」，而 MCP 客户端拿不到引用编号。共用一份的好处大过这句话的代价，
/// 真要分开时再说。
fn to_mcp_tools(openai: &Value, can_write: bool) -> Vec<Value> {
    openai
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = t.get("function")?;
                    let name = f.get("name")?.as_str()?;
                    if !is_exposed(name, can_write) {
                        return None;
                    }
                    Some(json!({
                        "name": name,
                        "description": f.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                        "inputSchema": f.get("parameters").cloned().unwrap_or(json!({
                            "type": "object", "properties": {}
                        })),
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ok(id: Option<Value>, result: Value) -> Json<Value> {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// JSON-RPC 的错误不是 HTTP 的错误：**传输成功了，方法失败了**。
/// 回 200 带 error 体，客户端才解析得动。
fn rpc_err(id: Option<Value>, code: i64, message: &str) -> Json<Value> {
    Json(json!({
        "jsonrpc": "2.0", "id": id,
        "error": { "code": code, "message": message }
    }))
}

pub async fn handle(
    State(state): State<AppState>,
    Path(kb_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> ApiResult<Json<Value>> {
    let (user, auth, kb, can_write) = authorize(&state, &headers, kb_id).await?;
    let id = req.get("id").cloned();
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    Ok(match method {
        "initialize" => ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "utopia", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        // 通知没有 id，按规范不该回响应体；但 HTTP 这一侧总得回点什么，
        // 回 202 语义更准，这里为了保持处理器签名统一回一个空结果
        "notifications/initialized" | "notifications/cancelled" => ok(None, json!({})),
        "ping" => ok(id, json!({})),
        // **列表跟着这枚令牌变。** 写不了的令牌看不见 `remember`——
        // 列出一个调不动的工具，等于让对面的 agent 反复试
        "tools/list" => ok(
            id,
            json!({
                // 传空的数据源列表：`query_data` 没放出来，那份描述也就不必生成
                "tools": to_mcp_tools(&super::chat::tools_schema(can_write, &[]), can_write)
            }),
        ),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if !is_exposed(name, can_write) {
                // 三种「不行」要分得开，否则客户端只能反复重试：
                // 拿不到的工具（query_data）是「这一版没放出来」，写工具是
                // 「这枚令牌/这个人写不了」——后者人改得了，前者改不了
                let message = if WRITABLE.contains(&name) {
                    format!(
                        "Tool '{name}' needs a token with the write scope, \
                         held by an editor in this base"
                    )
                } else {
                    format!("Tool '{name}' is not exposed over MCP in this version")
                };
                return Ok(rpc_err(id, -32601, &message));
            }
            // `mounted_sources` 仍旧空着：`query_data` 没放出来，给了也没人用。
            // `can_write` 不再写死 false——它现在是令牌与角色一起算出来的
            let ctx = ToolCtx {
                state: &state,
                kb_id,
                workspace_id: kb.workspace_id,
                mounted_sources: &[],
                can_write,
                actor: Some(user.id),
                // 「谁说的」要答到 agent 这一层：一个人可以同时挂三个客户端
                via_token: Some(auth.token_id),
            };
            let mut sink = ToolSink::default();
            let (text, _step) = tools::dispatch(&ctx, &mut sink, name, &args).await;
            let _ = utopia_store::audit::record(
                &state.pool,
                Some(kb_id),
                user.id,
                "mcp.tool_called",
                "personal_token",
                Some(auth.token_id),
                json!({ "tool": name }),
            )
            .await;
            ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false,
                }),
            )
        }
        other => rpc_err(id, -32601, &format!("Unknown method: {other}")),
    })
}
