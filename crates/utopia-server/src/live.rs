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
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
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
    pub content: String,
    pub steps: Vec<serde_json::Value>,
    pub sources: Vec<serde_json::Value>,
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

    pub fn to_frame(&self) -> Frame {
        Frame::new(
            "snapshot",
            json!({
                "content": self.content,
                "steps": self.steps,
                "sources": self.sources,
            })
            .to_string(),
        )
    }
}

struct Entry {
    tx: broadcast::Sender<Frame>,
    snap: Arc<RwLock<Snapshot>>,
}

/// 进行中的生成，按会话查。
#[derive(Default)]
pub struct Registry(RwLock<HashMap<Uuid, Entry>>);

/// 一次生成期间握着的把手。发事件、结束时注销。
pub struct Handle {
    conversation_id: Uuid,
    tx: broadcast::Sender<Frame>,
    snap: Arc<RwLock<Snapshot>>,
    registry: Arc<Registry>,
}

impl Handle {
    /// 发一个事件：记进快照，然后广播。
    ///
    /// **广播时仍然握着快照的写锁**，这一点是必需的。只保证「先写后发」
    /// 挡不住重复：接上的人在两步之间订阅，就会既在快照里看到这一段、
    /// 又从广播里再收一次。握着锁发，`attach` 那边握着读锁订阅，两者互斥——
    /// 于是接上的时刻要么整个在这次 emit 之前，要么整个在它之后
    pub async fn emit(&self, frame: Frame) {
        let mut snap = self.snap.write().await;
        snap.apply(&frame);
        // 没有订阅者是常态（人走了），不是错
        let _ = self.tx.send(frame);
    }

    /// 生成结束。**注销之后再接上的人得到的是「没有在跑的」**，
    /// 那时答案已经落库，从库里读就是了
    pub async fn finish(self) {
        self.registry.0.write().await.remove(&self.conversation_id);
    }
}

impl Registry {
    /// 登记一次生成。同一个会话重复登记会顶掉旧的——正常情况下不会发生，
    /// 真发生了也是新的那次说了算
    pub async fn begin(self: &Arc<Self>, conversation_id: Uuid) -> Handle {
        let (tx, _) = broadcast::channel(256);
        let snap = Arc::new(RwLock::new(Snapshot::default()));
        self.0.write().await.insert(
            conversation_id,
            Entry {
                tx: tx.clone(),
                snap: snap.clone(),
            },
        );
        Handle {
            conversation_id,
            tx,
            snap,
            registry: self.clone(),
        }
    }

    /// 接上一次正在跑的生成：拿到此刻的快照，以及之后的增量。
    ///
    /// 返回 `None` = 这个会话没有在跑的生成。**那不是错**，是最常见的情况
    pub async fn attach(
        &self,
        conversation_id: Uuid,
    ) -> Option<(Snapshot, broadcast::Receiver<Frame>)> {
        let map = self.0.read().await;
        let entry = map.get(&conversation_id)?;
        // **握着快照的读锁再订阅。** `emit` 是握着写锁广播的，所以这一段
        // 与任何一次 emit 互斥：拿到的快照与订阅起点严丝合缝，
        // 中间那一小段既不会漏、也不会重
        let guard = entry.snap.read().await;
        let rx = entry.tx.subscribe();
        Some((guard.clone(), rx))
    }
}
