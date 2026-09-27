use super::*;
use axum::response::IntoResponse;
use std::sync::Arc;

#[tokio::test]
async fn reattachment_preserves_terminal_frames_on_both_sides_of_emit() {
    for (event, data) in [("done", r#"{"stopped":true}"#), ("error", "safe outcome")] {
        let registry = Arc::new(crate::live::Registry::default());
        let id = Uuid::now_v7();
        let handle = registry.begin(id).await.unwrap();
        handle.emit(delta_event("partial")).await;
        let before = sse_from(registry.attach(id).await);
        handle.emit(Frame::new(event, data.into())).await;
        // 释放把手会关闭旧实现中的迟到接收端，
        // 即使丢失终态，这个反例也能结束而不必等超时。
        let after = sse_from(registry.attach(id).await);
        drop(handle);
        for response in [before, after] {
            let body = axum::body::to_bytes(response.into_response().into_body(), 65536)
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&body);
            assert_eq!(
                text.matches(&format!("event: {event}")).count(),
                1,
                "{text}"
            );
            assert!(text.contains("partial") && text.contains(data));
        }
        let idle = axum::body::to_bytes(
            sse_from(registry.attach(id).await)
                .into_response()
                .into_body(),
            65536,
        )
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&idle).contains("event: idle"));
    }
}

#[tokio::test]
async fn lagged_subscribers_receive_an_error_not_done() {
    let registry = Arc::new(crate::live::Registry::default());
    let id = Uuid::now_v7();
    let handle = registry.begin(id).await.unwrap();
    let stream = sse_from(registry.attach(id).await);
    for _ in 0..300 {
        handle.emit(delta_event("x")).await;
    }
    drop(handle);
    let body = axum::body::to_bytes(stream.into_response().into_body(), 65536)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: error") && !text.contains("event: done"));
}

#[tokio::test]
async fn producer_disappearing_without_an_outcome_ends_in_one_error() {
    let registry = Arc::new(crate::live::Registry::default());
    let id = Uuid::now_v7();
    let handle = registry.begin(id).await.unwrap();
    let stream = sse_from(registry.attach(id).await);
    handle.emit(delta_event("partial")).await;
    drop(handle);
    let body = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        axum::body::to_bytes(stream.into_response().into_body(), 65536),
    )
    .await
    .expect("closed producer must end the stream")
    .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert_eq!(text.matches("event: error").count(), 1, "{text}");
    assert!(!text.contains("event: done"), "{text}");
    assert!(text.contains("Answer stream ended unexpectedly"), "{text}");
    assert!(text.contains(r#""code":"stream_ended""#), "{text}");
}

#[tokio::test]
async fn first_terminal_freezes_the_snapshot_and_broadcast() {
    let registry = Arc::new(crate::live::Registry::default());
    let id = Uuid::now_v7();
    let handle = registry.begin(id).await.unwrap();
    handle.emit(delta_event("kept")).await;
    handle
        .emit(error_event("answer_failed", "original error"))
        .await;
    handle.emit(delta_event("discarded")).await;
    handle.emit(Frame::new("done", "{}".into())).await;
    let (snapshot, _) = registry.attach(id).await.unwrap();
    assert_eq!(snapshot.content, "kept");
    assert_eq!(snapshot.terminal().unwrap().event, "error");
    assert!(!snapshot.to_frame().data.contains("terminal"));
    drop(handle);
}
