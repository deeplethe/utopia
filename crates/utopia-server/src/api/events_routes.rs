//! KB 事件流（SSE）：文档摄入/抽取状态与审核队列变化的实时推送。
//! 前端收到事件只做 react-query 失效重取——事件本身不带业务数据，天然幂等。
//!
//! **告警也从这条流走**（#1028）。浏览器给一个源的 HTTP/1.1 连接只有六条，事件流占着
//! 不放：一页开两条通知流、再加一条正在生成的回答，两个标签页就占满，Stop 发不出去。
//! 所以打开着一个库的页只订这一条，全局那条 `/alerts/events` 留给没有库的页。

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::Stream;
use std::convert::Infallible;
use tokio::sync::broadcast;
use utopia_core::models::Role;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::state::{AppEvent, AppState};

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
                Ok(ev) => match relay(&ev, kb_id) {
                    Some(Relay::Alert) => yield Ok(Event::default().event("alert").data("{}")),
                    Some(Relay::Kb) => yield Ok(Event::default()
                        .event(ev.kind)
                        .data(serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into()))),
                    None => continue,
                },
                // 消费落后被跳帧：无所谓，事件只是"该刷新了"的信号
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Debug, PartialEq)]
enum Relay {
    /// 告警：不按库过滤、不带数据，和 `alerts_routes::stream` 同一个约定——收到的人
    /// 回头重取列表，谁能看见什么由列表查询说了算
    Alert,
    /// 这个库自己的事件，原样送出
    Kb,
}

fn relay(ev: &AppEvent, kb_id: Uuid) -> Option<Relay> {
    if ev.kind == "alert" {
        Some(Relay::Alert)
    } else if ev.kb_id == Some(kb_id) {
        Some(Relay::Kb)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kb_stream_carries_its_own_events_and_every_alert() {
        let (here, elsewhere) = (Uuid::now_v7(), Uuid::now_v7());
        let event = |kb_id, kind| AppEvent {
            kb_id,
            kind,
            document_id: None,
        };
        assert_eq!(relay(&event(Some(here), "document"), here), Some(Relay::Kb));
        assert_eq!(relay(&event(Some(elsewhere), "document"), here), None);
        // 系统级告警没有库，别的库的告警也要叫醒角标：都送，都不带数据
        for kb_id in [None, Some(here), Some(elsewhere)] {
            assert_eq!(relay(&event(kb_id, "alert"), here), Some(Relay::Alert));
        }
    }
}
