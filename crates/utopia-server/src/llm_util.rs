//! 从工作区设置构造 LLM 客户端，以及按模型的并发闸门。

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use utopia_core::models::LlmSettings;
use utopia_llm::LlmClient;

use crate::state::AppState;

pub fn chat_client(s: &LlmSettings) -> Option<LlmClient> {
    if !s.chat_ready() {
        return None;
    }
    Some(LlmClient::new(
        s.chat_base_url.as_deref()?,
        s.chat_api_key.as_deref(),
        s.chat_model.as_deref()?,
    ))
}

pub fn embed_client(s: &LlmSettings) -> Option<LlmClient> {
    if !s.embed_ready() {
        return None;
    }
    Some(LlmClient::new(
        s.embed_base_url.as_deref()?,
        s.embed_api_key.as_deref(),
        s.embed_model.as_deref()?,
    ))
}

/// 按模型的信号量注册表。限额变了就换一把新的——旧的在飞许可自然跑完，
/// 换的瞬间可能短暂超出新限额，可接受；换来的是"改完即时生效"而不必做
/// 缓存失效，也不必和 tokio Semaphore 不能缩容的限制搏斗。
#[derive(Default)]
pub struct ModelGates {
    inner: std::sync::Mutex<HashMap<String, (usize, Arc<Semaphore>)>>,
}

impl ModelGates {
    fn gate(&self, key: &str, limit: usize) -> Arc<Semaphore> {
        let mut m = self.inner.lock().unwrap();
        match m.get(key) {
            Some((n, sem)) if *n == limit => sem.clone(),
            _ => {
                let sem = Arc::new(Semaphore::new(limit));
                m.insert(key.to_string(), (limit, sem.clone()));
                sem
            }
        }
    }
}

/// 后台任务调模型前取一张许可，持有到调用结束。
///
/// **只给后台任务用**（抽取、裁决、摄入嵌入、本体建议）。用户的对话与检索
/// 不走这里——让人打字等在十个后台抽取后面，产品就成了坏的；真正会打爆
/// 供应商速率限制的也从来不是一个人在打字。
///
/// 限额读不到（表还没建、库暂时不可达）时**放行**：并发限制是保护措施，
/// 不该因为读不到配置而把整条流水线卡死。
pub async fn acquire(
    state: &AppState,
    base_url: &str,
    model: &str,
) -> Option<OwnedSemaphorePermit> {
    let limit = utopia_store::model_limits::limit_for(&state.pool, base_url, model)
        .await
        .ok()?;
    let key = format!("{base_url}|{model}");
    state
        .model_gates
        .gate(&key, limit)
        .acquire_owned()
        .await
        .ok()
}

/// `acquire` 的便捷形式：直接从工作区设置取 chat 模型的身份。
pub async fn acquire_chat(state: &AppState, s: &LlmSettings) -> Option<OwnedSemaphorePermit> {
    let (base, model) = (s.chat_base_url.as_deref()?, s.chat_model.as_deref()?);
    acquire(state, base, model).await
}

/// `acquire` 的便捷形式：embedding 模型。
pub async fn acquire_embed(state: &AppState, s: &LlmSettings) -> Option<OwnedSemaphorePermit> {
    let (base, model) = (s.embed_base_url.as_deref()?, s.embed_model.as_deref()?);
    acquire(state, base, model).await
}
