use super::*;
use axum::response::IntoResponse;
use std::sync::Arc;

#[tokio::test]
async fn reattachment_preserves_terminal_frames_on_both_sides_of_emit() {
    for event in ["done", "error"] {
        let registry = Arc::new(crate::live::Registry::default());
        let id = Uuid::now_v7();
        let handle = registry.begin(id).await;
        handle.emit(delta_event("partial")).await;
        let before = sse_from(registry.attach(id).await);
        handle.emit(Frame::new(event, "safe outcome".into())).await;
        // Finish closes the late receiver on the old implementation, so this
        // counterexample completes without timing out even when terminal is lost.
        let after = sse_from(registry.attach(id).await);
        handle.finish().await;
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
            assert!(text.contains("partial") && text.contains("safe outcome"));
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
    let handle = registry.begin(id).await;
    let stream = sse_from(registry.attach(id).await);
    for _ in 0..300 {
        handle.emit(delta_event("x")).await;
    }
    handle.finish().await;
    let body = axum::body::to_bytes(stream.into_response().into_body(), 65536)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("event: error") && !text.contains("event: done"));
}

#[tokio::test]
async fn first_terminal_freezes_the_snapshot_and_broadcast() {
    let registry = Arc::new(crate::live::Registry::default());
    let id = Uuid::now_v7();
    let handle = registry.begin(id).await;
    handle.emit(delta_event("kept")).await;
    handle.emit(error_event("original error")).await;
    handle.emit(delta_event("discarded")).await;
    handle.emit(done_event()).await;
    let (snapshot, _) = registry.attach(id).await.unwrap();
    assert_eq!(snapshot.content, "kept");
    assert_eq!(snapshot.terminal().unwrap().event, "error");
    assert!(!snapshot.to_frame().data.contains("terminal"));
    handle.finish().await;
}
