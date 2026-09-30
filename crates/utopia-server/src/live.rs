//! 正在生成的回答，能被重新接上。
//!
//! **一条 SSE 流绑在一个 HTTP 请求上，而回答比请求活得长。** 生成已经搬进
//! 独立任务（`api::chat`），所以刷新页面不会再丢答案；但那条流断了就是断了，
//! 刷新之后只能等它落库，中间那段看不见。前端把进行中的那一次搬出组件，
//! 解决的是同一个标签页里切来切去；**刷新、换标签页、换设备都不在其中**。
//!
//! 这里补上最后一段：生成期间把它登记下来，谁都可以再接上。
//!
//! **接上时先给一份快照，不是重放事件。** 事件流会无限长，缓冲它等于把
//! 一次对话的全部增量都留在内存里；而快照的大小就是那个回答本身的大小，
//! 有天然上限。客户端那边也更简单：拿快照覆盖当前状态，然后照常收增量，
//! 不必去想"我重放到哪一条了"。
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{broadcast, watch, RwLock};
use utopia_core::{AppError, AppResult};
use uuid::Uuid;

/// 一个 SSE 事件：事件名 + 已经序列化好的 data。
///
/// 不用 `axum::response::sse::Event`——它没有读回内容的办法，而这里既要
/// 广播出去，又要拿它更新快照。
#[derive(Clone, Debug)]
pub struct Frame {
    pub event: &'static str,
    pub data: String,
}

impl Frame {
    pub fn new(event: &'static str, data: String) -> Self {
        Self { event, data }
    }
}

/// 到此刻为止这个回答长什么样。接上的人先拿到它。
#[derive(Clone, Default, Debug)]
pub struct Snapshot {
    pub generation_id: Uuid,
    pub content: String,
    pub steps: Vec<serde_json::Value>,
    pub sources: Vec<serde_json::Value>,
    terminal: Option<Frame>,
}

impl Snapshot {
    /// **快照由事件本身推出来，不另设一套写入口。** 两套写法迟早对不上——
    /// 那正是这个仓库反复踩到的形状（一处认得新字段，另一处不认）
    fn apply(&mut self, f: &Frame) {
        match f.event {
            "delta" => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&f.data) {
                    if let Some(t) = v["text"].as_str() {
                        self.content.push_str(t);
                    }
                }
            }
            "step" => {
                if let Ok(v) = serde_json::from_str(&f.data) {
                    self.steps.push(v);
                }
            }
            // sources 是全量重发，不是追加
            "sources" => {
                if let Ok(serde_json::Value::Array(a)) = serde_json::from_str(&f.data) {
                    self.sources = a;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn terminal(&self) -> Option<Frame> {
        self.terminal.clone()
    }

    pub fn to_frame(&self) -> Frame {
        Frame::new(
            "snapshot",
            json!({
                "generation_id": self.generation_id,
                "content": self.content,
                "steps": self.steps,
                "sources": self.sources,
            })
            .to_string(),
        )
    }
}

struct Entry {
    generation_id: Uuid,
    tx: broadcast::Sender<Frame>,
    snap: Arc<RwLock<Snapshot>>,
    cancellation: Cancellation,
    ready: watch::Sender<bool>,
}

/// 保留取消信号：Stop 可能早于生成器开始等待。
#[derive(Clone)]
pub struct Cancellation(watch::Sender<bool>);

impl Default for Cancellation {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl Cancellation {
    pub fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }
}

/// 按会话登记进行中的生成，在写问题前占位。注册表锁不跨越 await；
/// 使用同步锁，让准备失败时的 Drop 也能释放占位。
#[derive(Default)]
pub struct Registry(Mutex<HashMap<Uuid, Entry>>);

/// 一次生成期间握着的把手。发事件、结束时注销。
pub struct Handle {
    conversation_id: Uuid,
    generation_id: Uuid,
    tx: broadcast::Sender<Frame>,
    snap: Arc<RwLock<Snapshot>>,
    cancellation: Cancellation,
    ready: watch::Sender<bool>,
    registry: Arc<Registry>,
}

impl Handle {
    pub fn generation_id(&self) -> Uuid {
        self.generation_id
    }

    pub fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.snap.read().await.clone()
    }

    /// 准备成功后才开放订阅；被拒的重试只释放占位，不会留下等待终态的客户端。
    pub async fn start(&self) {
        self.ready.send_replace(true);
    }

    /// 发一个事件：记进快照，然后广播。
    ///
    /// **广播时仍然握着快照的写锁**，这一点是必需的。只保证「先写后发」
    /// 挡不住重复：接上的人在两步之间订阅，就会既在快照里看到这一段、
    /// 又从广播里再收一次。握着锁发，`attach` 那边握着读锁订阅，两者互斥——
    /// 于是接上的时刻要么整个在这次 emit 之前，要么整个在它之后
    pub async fn emit(&self, frame: Frame) {
        let mut snap = self.snap.write().await;
        // 终态也必须记进快照并遵守订阅边界：
        // 广播后才接入的订阅者仍需要收到同一个结果。
        if snap.terminal.is_some() {
            return;
        }
        snap.apply(&frame);
        if matches!(frame.event, "done" | "error") {
            snap.terminal = Some(frame.clone());
        }
        // 没有订阅者是常态（人走了），不是错
        let _ = self.tx.send(frame);
    }

    /// 持久化完成后，先释放会话，再通知订阅者可以继续提问；
    /// 快照和订阅仍保持同一个边界。
    pub async fn complete(self, terminal: Frame) {
        let mut snap = self.snap.write().await;
        self.retire();
        if snap.terminal.is_some() {
            return;
        }
        snap.terminal = Some(terminal.clone());
        let _ = self.tx.send(terminal);
    }

    fn retire(&self) {
        let mut entries = self.registry.0.lock().expect("live registry lock");
        if entries
            .get(&self.conversation_id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.snap, &self.snap))
        {
            entries.remove(&self.conversation_id);
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.retire();
    }
}

impl Registry {
    /// 检查和占位在同一把锁下，新问题与重试都只允许一个生成者。
    pub async fn begin(self: &Arc<Self>, conversation_id: Uuid) -> AppResult<Handle> {
        let mut entries = self.0.lock().expect("live registry lock");
        if entries.contains_key(&conversation_id) {
            return Err(AppError::CodedConflict {
                code: "answer_running",
                message: "This conversation is still generating an answer.".into(),
            });
        }
        let generation_id = Uuid::now_v7();
        let (tx, _) = broadcast::channel(256);
        let snap = Arc::new(RwLock::new(Snapshot {
            generation_id,
            ..Snapshot::default()
        }));
        let cancellation = Cancellation::default();
        let (ready, _) = watch::channel(false);
        entries.insert(
            conversation_id,
            Entry {
                generation_id,
                tx: tx.clone(),
                snap: snap.clone(),
                cancellation: cancellation.clone(),
                ready: ready.clone(),
            },
        );
        Ok(Handle {
            conversation_id,
            generation_id,
            tx,
            snap,
            cancellation,
            ready,
            registry: self.clone(),
        })
    }

    /// 延迟或重复的 Stop 不能取消下一轮生成。
    pub async fn stop(&self, conversation_id: Uuid, generation_id: Uuid) {
        let entries = self.0.lock().expect("live registry lock");
        let Some(entry) = entries.get(&conversation_id) else {
            return;
        };
        if entry.generation_id == generation_id {
            entry.cancellation.cancel();
        }
    }

    /// 接上一次正在跑的生成：拿到此刻的快照，以及之后的增量。
    ///
    /// 返回 `None` = 这个会话没有在跑的生成。**那不是错**，是最常见的情况
    pub async fn attach(
        &self,
        conversation_id: Uuid,
    ) -> Option<(Snapshot, broadcast::Receiver<Frame>)> {
        let (snap, tx, mut ready) = {
            let map = self.0.lock().expect("live registry lock");
            let entry = map.get(&conversation_id)?;
            (
                entry.snap.clone(),
                entry.tx.clone(),
                entry.ready.subscribe(),
            )
        };
        // 不持有 ready 的发送端：准备失败释放占位时，等待者会醒来并回到 idle。
        // 准备成功则接上同一轮，避免过早返回 idle 后错过整个回答。
        ready.wait_for(|started| *started).await.ok()?;
        // **握着快照的读锁再订阅。** `emit` 是握着写锁广播的，所以这一段
        // 与任何一次 emit 互斥：拿到的快照与订阅起点严丝合缝，
        // 中间那一小段既不会漏、也不会重
        let guard = snap.read().await;
        let rx = tx.subscribe();
        Some((guard.clone(), rx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> Frame {
        Frame::new("delta", json!({"text": text}).to_string())
    }

    #[tokio::test]
    async fn retired_handle_cannot_unregister_or_write_into_current_generation() {
        let registry = Arc::new(Registry::default());
        let id = Uuid::now_v7();
        let old = registry.begin(id).await.unwrap();
        old.start().await;
        old.emit(delta("old")).await;
        old.retire();
        let current = registry.begin(id).await.unwrap();
        current.start().await;
        current.emit(delta("new")).await;
        let (snapshot, mut rx) = registry.attach(id).await.unwrap();
        assert_eq!(snapshot.content, "new");
        old.emit(delta("late old text")).await;
        drop(old);
        let (snapshot, _) = registry
            .attach(id)
            .await
            .expect("new generation still running");
        assert_eq!(snapshot.content, "new");
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
        current.emit(delta(" answer")).await;
        assert_eq!(rx.recv().await.unwrap().data, delta(" answer").data);
        drop(current);
        assert!(registry.attach(id).await.is_none());
    }

    #[tokio::test]
    async fn only_current_owner_can_remove_entry_in_any_finish_order() {
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let registry = Arc::new(Registry::default());
            let id = Uuid::now_v7();
            let other_id = Uuid::now_v7();
            let other = registry.begin(other_id).await.unwrap();
            other.start().await;
            other.emit(delta("unrelated")).await;
            let mut handles = Vec::new();
            for index in 0..3 {
                let handle = registry.begin(id).await.unwrap();
                handle.start().await;
                if index < 2 {
                    handle.retire();
                }
                handles.push(Some(handle));
            }
            let mut current_finished = false;
            for index in order {
                drop(handles[index].take().unwrap());
                current_finished |= index == 2;
                assert_eq!(
                    registry.attach(id).await.is_none(),
                    current_finished,
                    "{order:?}"
                );
                assert_eq!(
                    registry.attach(other_id).await.unwrap().0.content,
                    "unrelated"
                );
            }
            drop(other);
            assert!(registry.attach(other_id).await.is_none());
        }
    }

    #[tokio::test]
    async fn snapshot_and_subscription_partition_concurrent_emission() {
        let registry = Arc::new(Registry::default());
        let id = Uuid::now_v7();
        let handle = registry.begin(id).await.unwrap();
        handle.start().await;
        // 两种拿锁顺序都合法：每个增量必须恰好出现在快照或订阅中一次，
        // 既不能重复，也不能遗漏。
        for index in 0..64 {
            let attached = if index % 2 == 0 {
                tokio::join!(handle.emit(delta("x")), registry.attach(id)).1
            } else {
                tokio::join!(registry.attach(id), handle.emit(delta("x"))).0
            };
            let (snapshot, mut rx) = attached.unwrap();
            let mut combined = snapshot.content;
            while let Ok(frame) = rx.try_recv() {
                combined.push_str(
                    serde_json::from_str::<serde_json::Value>(&frame.data).unwrap()["text"]
                        .as_str()
                        .unwrap(),
                );
            }
            assert_eq!(combined, registry.attach(id).await.unwrap().0.content);
        }
        drop(handle);
    }

    #[tokio::test]
    async fn a_preparing_reservation_is_exclusive_but_not_subscribable() {
        let registry = Arc::new(Registry::default());
        let id = Uuid::now_v7();
        let handle = registry.begin(id).await.unwrap();
        assert!(matches!(
            registry.begin(id).await,
            Err(AppError::CodedConflict {
                code: "answer_running",
                ..
            })
        ));
        let attached = registry.attach(id);
        tokio::pin!(attached);
        assert!(futures_util::poll!(&mut attached).is_pending());
        drop(handle);
        assert!(attached.await.is_none());
        let _next = registry.begin(id).await.unwrap();
    }

    #[tokio::test]
    async fn an_attachment_waiting_for_preparation_joins_the_started_generation() {
        let registry = Arc::new(Registry::default());
        let id = Uuid::now_v7();
        let handle = registry.begin(id).await.unwrap();
        let attached = registry.attach(id);
        tokio::pin!(attached);
        assert!(futures_util::poll!(&mut attached).is_pending());

        handle.start().await;
        let (snapshot, mut receiver) = attached.await.unwrap();
        assert_eq!(snapshot.generation_id, handle.generation_id());
        handle.emit(delta("answer")).await;
        assert_eq!(receiver.recv().await.unwrap().data, delta("answer").data);
        handle
            .complete(Frame::new("done", json!({"stopped":false}).to_string()))
            .await;
        assert_eq!(receiver.recv().await.unwrap().event, "done");
    }

    #[tokio::test]
    async fn admission_is_exclusive_and_dropping_a_reservation_releases_it() {
        let registry = Arc::new(Registry::default());
        let id = Uuid::now_v7();
        let (first, second) = tokio::join!(registry.begin(id), registry.begin(id));
        assert_ne!(first.is_ok(), second.is_ok());
        let (handle, error) = match (first, second) {
            (Ok(handle), Err(error)) | (Err(error), Ok(handle)) => (handle, error),
            _ => unreachable!(),
        };
        assert!(matches!(
            error,
            AppError::CodedConflict {
                code: "answer_running",
                ..
            }
        ));
        let old_generation = handle.generation_id();
        drop(handle);
        assert!(registry.attach(id).await.is_none());

        let current = registry.begin(id).await.unwrap();
        let cancellation = current.cancellation();
        registry.stop(id, old_generation).await;
        assert!(!cancellation.is_cancelled());
        registry.stop(id, current.generation_id()).await;
        registry.stop(id, current.generation_id()).await;
        tokio::time::timeout(std::time::Duration::from_secs(1), cancellation.cancelled())
            .await
            .expect("Stop is retained even before the first waiter");
    }

    #[tokio::test]
    async fn the_terminal_is_sent_after_the_conversation_is_available() {
        let registry = Arc::new(Registry::default());
        let id = Uuid::now_v7();
        let handle = registry.begin(id).await.unwrap();
        handle.start().await;
        let (_, mut receiver) = registry.attach(id).await.unwrap();
        handle
            .complete(Frame::new("done", json!({"stopped":true}).to_string()))
            .await;
        assert_eq!(receiver.recv().await.unwrap().event, "done");
        let _next = registry
            .begin(id)
            .await
            .expect("a subscriber may immediately follow up");
    }
}
