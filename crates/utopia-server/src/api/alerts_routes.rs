//! 告警中心（0005）。**跨库**：一个顶栏面板，不挂在某个 KB 下面。
//!
//! 可见性不在这里判——它在 `utopia_store::alerts` 的那几条 SQL 里判且只判一次。
//! 路由层只负责取当前用户。

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use chrono::{DateTime, Utc};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

/// 一页几组。弹窗里放得下的量——再多就该翻页，而不是让人滚一屏。
const PAGE: i64 = 8;
/// 一页最多能要多少：防的是有人把 limit 写成 100000 让服务端去数全表
const MAX_PAGE: i64 = 50;

#[derive(Deserialize)]
pub struct ListQuery {
    /// 搜库名、对象详情、kind 代号。**搜不到界面上那句标题**——
    /// 措辞在客户端，服务端没有它（见 store 里 SEARCH 的注释）
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

/// 一组连着的同类故障。折叠是**读**出来的，存储那边仍是一次故障一行。
#[derive(Serialize)]
pub struct GroupView {
    pub kb_id: Option<Uuid>,
    pub kb_name: Option<String>,
    pub kind: String,
    pub severity: String,
    pub count: i64,
    pub unread: i64,
    pub latest_at: DateTime<Utc>,
    /// 跟 `latest_at` 一起圈出这一组，标已读时原样发回来
    pub earliest_at: DateTime<Utc>,
    /// 明细，最多几条，新的在前
    pub lines: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub struct ListResponse {
    pub items: Vec<GroupView>,
    /// 总**组**数——翻页控件数的是组，不是行
    pub total: i64,
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<ListResponse>> {
    let limit = q.limit.unwrap_or(PAGE).clamp(1, MAX_PAGE);
    let offset = q.offset.unwrap_or(0).max(0);
    let page = utopia_store::alerts::list_groups(&state.pool, &user, q.q.as_deref(), limit, offset)
        .await?;
    Ok(Json(ListResponse {
        items: page
            .items
            .into_iter()
            .map(|g| GroupView {
                kb_id: g.kb_id,
                kb_name: g.kb_name,
                kind: g.kind,
                severity: g.severity,
                count: g.count,
                unread: g.unread,
                latest_at: g.latest_at,
                earliest_at: g.earliest_at,
                lines: g.lines,
            })
            .collect(),
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

#[derive(Deserialize)]
pub struct ReadGroupBody {
    pub kb_id: Option<Uuid>,
    pub kind: String,
    /// 组的时间区间，原样来自列表返回的 `earliest_at` / `latest_at`
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// 把一整组标已读。**逐人**——读过不等于问题没了，别人的未读不受影响。
///
/// 按时间区间圈而不是发 id 列表：一组可能有几百条。可见性在 store 里照查，
/// 所以猜一个 kind 也标不掉自己看不见的东西。
pub async fn mark_group_read(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(b): Json<ReadGroupBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let n =
        utopia_store::alerts::mark_group_read(&state.pool, &user, b.kb_id, &b.kind, b.from, b.to)
            .await?;
    Ok(Json(json!({ "marked": n })))
}

/// 全部已读。
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
