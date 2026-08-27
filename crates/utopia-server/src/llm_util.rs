//! 从工作区设置构造 LLM 客户端。

use utopia_core::models::LlmSettings;
use utopia_llm::LlmClient;

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
