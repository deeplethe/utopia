//! 告警中心（0005）。**跨库**：一个列表页 + 顶栏角标，不挂在某个 KB 下面。
//!
//! 可见性不在这里判——它在 `utopia_store::alerts` 的那条 SQL 里判且只判一次。
//! 路由层只负责取当前用户。

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::broadcast;
use utopia_core::models::AlertView;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

/// 一页几条。弹窗里放得下的量——再多就该翻页，而不是让人滚一屏。
const PAGE: i64 = 8;
/// 一页最多能要多少：防的是有人把 limit 写成 100000 让服务端去数全表
const MAX_PAGE: i64 = 50;

#[derive(Deserialize)]
pub struct ListQuery {
    /// 默认只看未解决的。自愈是这个功能的核心，已解决的默认不该占位置
    #[serde(default)]
    pub include_resolved: bool,
    /// 搜库名、对象详情、kind 代号。**搜不到界面上那句标题**——
    /// 措辞在客户端，服务端没有它（见 store 里 SEARCH 的注释）
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub items: Vec<AlertView>,
    pub total: i64,
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(PAGE).clamp(1, MAX_PAGE);
    let offset = q.offset.unwrap_or(0).max(0);
    let page = utopia_store::alerts::list_for_user(
        &state.pool,
        &user,
        q.include_resolved,
        q.q.as_deref(),
        limit,
        offset,
    )
    .await?;
    Ok(Json(ListResponse {
        items: page.items,
        total: page.total,
    }))
}

/// 角标。单独一条路由而不是从列表里数：这个每开一页都要。
pub async fn unread(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let n = utopia_store::alerts::unread_count(&state.pool, &user).await?;
    Ok(Json(json!({ "unread": n })))
}

/// 标记已读。**逐人**——读过不等于解决了，别人的未读列表不受影响。
///
/// 先查可见性：不能让人靠猜 id 把一条自己看不见的告警标成已读
///（本身无害，但那会在 alert_reads 里留下一条他本不该有的记录）。
pub async fn mark_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(alert_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    if !utopia_store::alerts::is_visible(&state.pool, &user, alert_id).await? {
        return Err(utopia_core::AppError::NotFound.into());
    }
    utopia_store::alerts::mark_read(&state.pool, alert_id, user.id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// 全部已读。只清**当前可见且未解决**的那些——已解决的本来就不在未读里。
pub async fn mark_all_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<serde_json::Value>> {
    let n = utopia_store::alerts::mark_all_read(&state.pool, &user).await?;
    Ok(Json(json!({ "marked": n })))
}

/// 全局事件流。KB 那条是 `/kbs/{id}/events`，按库过滤；角标是跨库的，
/// 挂不上去。
///
/// **这里不做任何权限过滤**：事件不带数据，收到的人一律回头重取列表，
/// 而列表那条查询会把不该看的挡掉。代价是没权限的人也被叫醒一次，
/// 换来的是推送这条路上一行权限逻辑都没有——不会出现"推送判得比列表松"。
pub async fn stream(
    State(state): State<AppState>,
    AuthUser(_user): AuthUser,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let mut rx = state.events.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.kind == "alert" => {
                    yield Ok(Event::default().event("alert").data("{}"));
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
