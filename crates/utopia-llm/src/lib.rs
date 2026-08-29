//! utopia-llm: OpenAI 兼容协议的薄客户端。
//! 一套代码适配 DeepSeek / Qwen(DashScope 兼容模式) / GLM / OpenAI / Ollama / vLLM。

use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// OpenAI 协议的工具调用（assistant 回合携带）。
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// JSON 字符串参数（协议原样透传）
    pub arguments: String,
}

/// 工具对话的一个 assistant 回合：文本与工具调用至少其一。
#[derive(Debug)]
pub struct AssistantTurn {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl AssistantTurn {
    /// 还原为 OpenAI 协议的 assistant 消息（回灌对话历史用）。
    pub fn to_message(&self) -> serde_json::Value {
        let mut msg = json!({ "role": "assistant", "content": self.content });
        if !self.tool_calls.is_empty() {
            msg["tool_calls"] = json!(self
                .tool_calls
                .iter()
                .map(|c| json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments },
                }))
                .collect::<Vec<_>>());
        }
        msg
    }
}

/// 工具结果消息（role=tool）。
pub fn tool_result_message(tool_call_id: &str, content: &str) -> serde_json::Value {
    json!({ "role": "tool", "tool_call_id": tool_call_id, "content": content })
}

/// 带工具的流式回合事件。
#[derive(Debug)]
pub enum ToolStreamItem {
    /// 增量正文（即时转发给前端）
    Delta(String),
    /// 流结束：完整回合（累积正文 + 归并后的工具调用）
    Turn(AssistantTurn),
}

/// **没能从端点拿到一个能解析的回答。** 两种：连不上（DNS、连接、TLS、超时），
/// 或者连上了但回来的根本不是这个 API（响应体解不成 JSON）。
///
/// 合成一类是有意的：从用户那边看这两种是同一件事——"你配的这个地址不是模型 API"，
/// 该做的也是同一件事：去看 URL、看代理。第一版只收传输层失败，结果最常见的那种
/// 故障（URL 配错、代理挡在中间回了 HTML）一条告警都不产生，
/// 正是"失败无声"本身。
///
/// **不含**端点干干净净地回了 4xx/5xx：那说明它就是模型 API，只是密钥、
/// 配额或模型名不对——另一类问题，该找的人也不同。
///
/// 有类型而不是匹配错误文本：调用链上任何一层加一句 context 都会改文本，
/// 而 `anyhow` 的 source 链让 `downcast_ref` 一路都认得出来。
#[derive(Debug, thiserror::Error)]
#[error("LLM endpoint gave no usable answer: {0}")]
pub struct Unreachable(#[from] pub reqwest::Error);

/// anyhow 错误链里有没有 [`Unreachable`]。
pub fn is_unreachable(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<Unreachable>())
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    pub model: String,
}

impl LlmClient {
    pub fn new(base_url: &str, api_key: Option<&str>, model: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.map(String::from),
            model: model.to_string(),
        }
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut req = self.http.post(format!("{}{path}", self.base_url));
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        req
    }

    /// 非流式对话（连通性测试等轻量场景）。
    pub async fn chat(&self, messages: &[ChatMessage]) -> anyhow::Result<String> {
        let resp = self
            .request("/chat/completions")
            .json(&json!({ "model": self.model, "messages": messages, "stream": false }))
            .send()
            .await
            .map_err(Unreachable)?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(Unreachable)?;
        if !status.is_success() {
            anyhow::bail!("LLM request failed ({status}): {}", err_detail(&body));
        }
        log_usage(&self.model, &body);
        body["choices"][0]["message"]["content"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow::anyhow!("Unexpected LLM response shape: {body}"))
    }

    /// 工具对话（非流式）：messages 为 OpenAI 协议原始 JSON
    /// （支持 assistant.tool_calls 与 role=tool 回合），tools 为 function 定义数组。
    pub async fn chat_tools(
        &self,
        messages: &[serde_json::Value],
        tools: &serde_json::Value,
    ) -> anyhow::Result<AssistantTurn> {
        let resp = self
            .request("/chat/completions")
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": false,
            }))
            .send()
            .await
            .map_err(Unreachable)?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(Unreachable)?;
        if !status.is_success() {
            anyhow::bail!("LLM request failed ({status}): {}", err_detail(&body));
        }
        let msg = &body["choices"][0]["message"];
        if msg.is_null() {
            anyhow::bail!("Unexpected LLM response shape: {body}");
        }
        let content = msg["content"]
            .as_str()
            .map(String::from)
            .filter(|s| !s.is_empty());
        let tool_calls = msg["tool_calls"]
            .as_array()
            .map(|calls| {
                calls
                    .iter()
                    .filter_map(|c| {
                        Some(ToolCall {
                            id: c["id"].as_str()?.to_string(),
                            name: c["function"]["name"].as_str()?.to_string(),
                            arguments: c["function"]["arguments"]
                                .as_str()
                                .unwrap_or("{}")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(AssistantTurn {
            content,
            tool_calls,
        })
    }

    /// 工具对话（流式）：正文增量即时产出，工具调用按 OpenAI 协议的
    /// index 分片归并（id/name 首帧到达，arguments 逐帧续传），流末给出完整回合。
    pub async fn chat_tools_stream(
        &self,
        messages: &[serde_json::Value],
        tools: &serde_json::Value,
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<ToolStreamItem>> + Send + use<>> {
        let resp = self
            .request("/chat/completions")
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
            }))
            .send()
            .await
            .map_err(Unreachable)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            anyhow::bail!("LLM request failed ({status}): {}", err_detail(&body));
        }

        let mut bytes = resp.bytes_stream();
        let stream = async_stream::try_stream! {
            let mut buf = String::new();
            let mut content = String::new();
            let mut calls: Vec<ToolCall> = Vec::new();
            let mut done = false;
            'outer: while let Some(part) = bytes.next().await {
                let part = part?;
                buf.push_str(&String::from_utf8_lossy(&part));
                while let Some(pos) = buf.find("\n\n") {
                    let frame = buf[..pos].to_string();
                    buf.drain(..pos + 2);
                    for line in frame.lines() {
                        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                            continue;
                        };
                        if data == "[DONE]" {
                            done = true;
                            break 'outer;
                        }
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
                            continue;
                        };
                        let delta = &v["choices"][0]["delta"];
                        if let Some(text) = delta["content"].as_str() {
                            if !text.is_empty() {
                                content.push_str(text);
                                yield ToolStreamItem::Delta(text.to_string());
                            }
                        }
                        if let Some(tcs) = delta["tool_calls"].as_array() {
                            for tc in tcs {
                                let idx = tc["index"].as_u64().unwrap_or(0) as usize;
                                while calls.len() <= idx {
                                    calls.push(ToolCall {
                                        id: String::new(),
                                        name: String::new(),
                                        arguments: String::new(),
                                    });
                                }
                                let slot = &mut calls[idx];
                                if let Some(id) = tc["id"].as_str() {
                                    slot.id.push_str(id);
                                }
                                if let Some(n) = tc["function"]["name"].as_str() {
                                    slot.name.push_str(n);
                                }
                                if let Some(a) = tc["function"]["arguments"].as_str() {
                                    slot.arguments.push_str(a);
                                }
                            }
                        }
                    }
                }
            }
            let _ = done;
            calls.retain(|c| !c.name.is_empty());
            let content = if content.is_empty() { None } else { Some(content) };
            yield ToolStreamItem::Turn(AssistantTurn { content, tool_calls: calls });
        };
        Ok(stream)
    }

    /// 流式对话：产出增量文本片段。
    pub async fn chat_stream(
        &self,
        messages: &[ChatMessage],
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<String>> + Send + use<>> {
        let messages = messages
            .iter()
            .map(|m| serde_json::to_value(m).unwrap_or_default())
            .collect::<Vec<_>>();
        self.chat_stream_raw(&messages).await
    }

    /// 流式对话（原始 JSON 消息，可携带工具回合上下文）。
    pub async fn chat_stream_raw(
        &self,
        messages: &[serde_json::Value],
    ) -> anyhow::Result<impl Stream<Item = anyhow::Result<String>> + Send + use<>> {
        let resp = self
            .request("/chat/completions")
            .json(&json!({ "model": self.model, "messages": messages, "stream": true }))
            .send()
            .await
            .map_err(Unreachable)?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            anyhow::bail!("LLM request failed ({status}): {}", err_detail(&body));
        }

        let mut bytes = resp.bytes_stream();
        let stream = async_stream::try_stream! {
            let mut buf = String::new();
            while let Some(part) = bytes.next().await {
                let part = part?;
                buf.push_str(&String::from_utf8_lossy(&part));
                // SSE 帧以空行分隔；逐帧取出已完整到达的部分
                while let Some(pos) = buf.find("\n\n") {
                    let frame = buf[..pos].to_string();
                    buf.drain(..pos + 2);
                    for line in frame.lines() {
                        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                            continue;
                        };
                        if data == "[DONE]" {
                            return;
                        }
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(delta) = v["choices"][0]["delta"]["content"].as_str() {
                                if !delta.is_empty() {
                                    yield delta.to_string();
                                }
                            }
                        }
                    }
                }
            }
        };
        Ok(stream)
    }

    /// 批量 embedding。
    pub async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let resp = self
            .request("/embeddings")
            .json(&json!({ "model": self.model, "input": texts }))
            .send()
            .await
            .map_err(Unreachable)?;
        let status = resp.status();
        let body: serde_json::Value = resp.json().await.map_err(Unreachable)?;
        if !status.is_success() {
            anyhow::bail!("Embedding request failed ({status}): {}", err_detail(&body));
        }
        let data = body["data"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Unexpected embedding response shape"))?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let v = item["embedding"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("Embedding 响应缺少向量"))?
                .iter()
                .filter_map(|x| x.as_f64().map(|f| f as f32))
                .collect();
            out.push(v);
        }
        Ok(out)
    }
}

/// 记一次调用的 token 开销。**缓存命中数是这里最重要的一列**：抽取靠
/// "system 消息在一篇文档内逐块完全相同" 吃供应商的前缀缓存，
/// 往 system 里塞逐块变化的内容会让它悄悄归零——只有这个数看得见。
///
/// 字段名各家不一：OpenAI 用 prompt_tokens_details.cached_tokens，
/// DeepSeek 用 prompt_cache_hit_tokens。两个都读，谁在读谁。
fn log_usage(model: &str, body: &serde_json::Value) {
    let u = &body["usage"];
    if u.is_null() {
        return;
    }
    let n = |k: &str| u[k].as_u64();
    let cached = u["prompt_tokens_details"]["cached_tokens"]
        .as_u64()
        .or_else(|| n("prompt_cache_hit_tokens"));
    tracing::info!(
        model,
        prompt = n("prompt_tokens"),
        completion = n("completion_tokens"),
        cached,
        "llm usage"
    );
}

fn err_detail(body: &serde_json::Value) -> String {
    body["error"]["message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or("unknown error")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真发一次注定失败的请求，拿一个货真价实的 `reqwest::Error`。
    /// 端口 1 上不会有东西监听，而 127.0.0.1 不走代理。
    async fn a_real_transport_error() -> reqwest::Error {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get("http://127.0.0.1:1/")
            .send()
            .await
            .expect_err("端口 1 不该连得上")
    }

    /// **判定必须穿透 context 层。**
    ///
    /// 这是整条链上最容易悄悄坏掉的一环：调用方每加一句 `.context("抽取失败")`
    /// 就换掉一次错误文本，靠文本匹配的判定当天就废——而症状是告警再也不出现，
    /// 没有任何测试会红，用户也不会来报"我没收到告警"。
    #[tokio::test]
    async fn unreachable_survives_context_layers() {
        let raw = a_real_transport_error().await;
        let err = anyhow::Error::new(Unreachable(raw))
            .context("embedding 失败")
            .context("process_document 失败");
        assert!(is_unreachable(&err));
        // 顺带钉住"文本会变"这件事本身：最外层已经不含端点的任何字样
        assert!(!err.to_string().contains("endpoint"));
    }

    /// 反面：普通错误不该被认成端点问题，否则告警会对任何失败都亮。
    #[tokio::test]
    async fn an_ordinary_failure_is_not_the_endpoint() {
        let e = anyhow::anyhow!("Embedding 响应缺少向量").context("抽取失败");
        assert!(!is_unreachable(&e));
    }

    /// 端点干干净净地回了 4xx 不算——那说明它就是模型 API，
    /// 只是密钥或模型名不对，该找的人和该做的事都不一样。
    #[tokio::test]
    async fn a_clean_api_error_is_a_different_problem() {
        let e = anyhow::anyhow!("LLM request failed (401 Unauthorized): bad key");
        assert!(!is_unreachable(&e));
    }
}
