//! KB 事件流（SSE）：文档摄入/抽取状态与审核队列变化的实时推送。
//! 前端收到事件只做 react-query 失效重取——事件本身不带业务数据，天然幂等。

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use utopia_core::models::Role;
use std::convert::Infallible;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::AppState;

pub async fn kb_events(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb_id): Path<Uuid>,
) -> ApiResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    utopia_store::access::require_kb(&state.pool, &user, kb_id, Role::Viewer).await?;

    let mut rx = state.events.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.kb_id == kb_id => {
                    yield Ok(Event::default()
                        .event(ev.kind)
                        .data(serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into())));
                }
                Ok(_) => continue,
                // 消费落后被跳帧：无所谓，事件只是"该刷新了"的信号
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
