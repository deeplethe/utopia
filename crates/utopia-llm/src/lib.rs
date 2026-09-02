//! utopia-llm: OpenAI 兼容协议的薄客户端。
//! 一套代码适配 DeepSeek / Qwen(DashScope 兼容模式) / GLM / OpenAI / Ollama / vLLM。

use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

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

/// 端点在限流。**跟 [`Unreachable`] 一样做成类型**，理由也一样：调用方要据此
/// 决定「等一会儿再来」而不是「这块废了」，而错误文本一路都在被 context 改写。
///
/// 限流与其他 4xx 的区别是**它会自己好**。密钥错了重试一万次还是错，配额满了
/// 等一分钟就过去——两者混在一起，重试预算就会花在永远不会好的那一类上。
#[derive(Debug, thiserror::Error)]
#[error("LLM endpoint is rate limiting ({status}): {detail}")]
pub struct RateLimited {
    pub status: u16,
    /// **常常是 `None`。** 多数厂商的 429 不带 `Retry-After`（实测 SiliconFlow
    /// 就不带），所以调用方必须自带退避，把这一项当「有则更准」的补充而不是判据。
    pub retry_after: Option<Duration>,
    pub detail: String,
}

/// anyhow 错误链里的 [`RateLimited`]，穿透 context 层。
pub fn rate_limited(err: &anyhow::Error) -> Option<&RateLimited> {
    err.chain().find_map(|e| e.downcast_ref::<RateLimited>())
}

/// 账号付不起这次请求：欠费，或者套餐配额用尽。
///
/// **跟 [`RateLimited`] 分开，因为它不会自己好。** 限流等一分钟就过去，
/// 欠费等到天亮也还是欠费——重试只是在把同一个错误说三遍，而真正该发生的事
/// （有人去充值）不会因为重试而发生。
///
/// 实测：一次跑测里 14 篇文档因为它整篇失败，而当时它跟普通失败走同一条路，
/// 唯一能知道原因的办法是去数据库里翻 `graph_error`。
#[derive(Debug, thiserror::Error)]
#[error("LLM account cannot pay for this request ({status}): {detail}")]
pub struct OutOfCredit {
    pub status: u16,
    pub detail: String,
}

/// anyhow 错误链里的 [`OutOfCredit`]，穿透 context 层。
pub fn out_of_credit(err: &anyhow::Error) -> Option<&OutOfCredit> {
    err.chain().find_map(|e| e.downcast_ref::<OutOfCredit>())
}

/// `Retry-After` 的整数秒形态。
///
/// 规范还允许 HTTP-date，这里**不解析**：为一个很少有人发的头引一个日期库不划算，
/// 而解析失败当成没有正是对的——调用方本来就得有退避，多猜一个数只会更难查。
fn retry_after_of(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

/// 非 2xx 统一在这里成型，分成三类：欠费、限流、其他。
///
/// **503 不算限流**：它可能是端点真挂了、也可能是中间代理，把它算进来会让
/// 「等一会儿再来」用在等不回来的地方。判据窄一点，宁可退回「其他」。
fn failure(
    kind: &str,
    status: reqwest::StatusCode,
    retry_after: Option<Duration>,
    body: &serde_json::Value,
) -> anyhow::Error {
    let detail = err_detail(body);
    // **欠费要先判，而且不能只看状态码。**
    //
    // 402 是标准答案（SiliconFlow 用它），但 OpenAI 的余额耗尽走的是 **429**，
    // 靠 body 里的 `insufficient_quota` 区分。只按状态码分类的话，一个没钱的
    // OpenAI 账号会被当成限流，然后无限退避重试一个永远不会好的东西——
    // 而退避越久，症状越像"端点慢"，越查不到根上。
    if status == reqwest::StatusCode::PAYMENT_REQUIRED || says_out_of_credit(body) {
        return anyhow::Error::new(OutOfCredit {
            status: status.as_u16(),
            detail,
        });
    }
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return anyhow::Error::new(RateLimited {
            status: status.as_u16(),
            retry_after,
            detail,
        });
    }
    anyhow::anyhow!("{kind} request failed ({status}): {detail}")
}

#[derive(Clone)]
pub struct LlmClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    pub model: String,
}

/// 建连多久算失败。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// **多久没有新字节算这个请求死了。**
///
/// 不能用 `Client::timeout`（请求总时长）：`chat_tools_stream` 是真流式，
/// 一次长对话正当地跑几分钟，总时长封顶会把它拦腰砍断。而 `read_timeout`
/// 量的是**沉默**——流式下 token 持续到达，永远碰不到它；非流式下它兜住的
/// 正是「请求发出去就石沉大海」。
///
/// 为什么是 300 秒而不是 60：非流式调用的首字节要等模型把整段生成完，
/// 大提示词的抽取正常就要 60–120 秒，服务端排队时更久。设小了会把正常请求
/// 判死，而这条路上「误杀」比「晚 5 分钟发现」贵得多——它会让本来能成的抽取失败。
///
/// **没有它的代价实测过**：`reqwest::Client::new()` 默认不设任何超时，
/// 32 路并发打同一个账号时请求全部挂住，32 个 worker 槽被永久占满，
/// 流水线停摆而**一条错误都不报**（jobs 的孤儿回收只在进程启动时跑一次，
/// 进程活着就永远收不了尸）。7459 块的一次灌入死在第 55 块上。
const READ_TIMEOUT: Duration = Duration::from_secs(300);

impl LlmClient {
    pub fn new(base_url: &str, api_key: Option<&str>, model: &str) -> Self {
        Self::with_timeouts(base_url, api_key, model, CONNECT_TIMEOUT, READ_TIMEOUT)
    }

    /// 超时可注入，只为**测得动**——生产走 [`LlmClient::new`]。
    /// 拿 300 秒去测一次挂死要跑 5 分钟，那样的测试没人会留着。
    pub fn with_timeouts(
        base_url: &str,
        api_key: Option<&str>,
        model: &str,
        connect: Duration,
        read: Duration,
    ) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(connect)
                .read_timeout(read)
                .build()
                // 只在 TLS 后端起不来时失败，那种情况下退回默认客户端也没有意义，
                // 但也不该让整个进程崩在这里
                .unwrap_or_else(|e| {
                    tracing::error!(error = %e, "HTTP 客户端建不起来，退回无超时的默认客户端");
                    reqwest::Client::new()
                }),
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
        let retry_after = retry_after_of(resp.headers());
        let body: serde_json::Value = resp.json().await.map_err(Unreachable)?;
        if !status.is_success() {
            return Err(failure("LLM", status, retry_after, &body));
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
        let retry_after = retry_after_of(resp.headers());
        let body: serde_json::Value = resp.json().await.map_err(Unreachable)?;
        if !status.is_success() {
            return Err(failure("LLM", status, retry_after, &body));
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
            let retry_after = retry_after_of(resp.headers());
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            return Err(failure("LLM", status, retry_after, &body));
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
            let retry_after = retry_after_of(resp.headers());
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            return Err(failure("LLM", status, retry_after, &body));
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
        let retry_after = retry_after_of(resp.headers());
        let body: serde_json::Value = resp.json().await.map_err(Unreachable)?;
        if !status.is_success() {
            return Err(failure("Embedding", status, retry_after, &body));
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

/// 响应体说不说这是余额问题。
///
/// **给 429 用的**：OpenAI 用同一个状态码表示「太快了」和「没钱了」，
/// `error.code` 或 `error.type` 里的 `insufficient_quota` 才是分界。
///
/// 只认这一个标识、不去匹配 message 的自由文本：措辞会改、会本地化，
/// 而 `code` 是接口契约的一部分。认不出来就退回按状态码判，那是安全的一侧
/// （当成限流退避几次，比当成欠费直接放弃温和）。
fn says_out_of_credit(body: &serde_json::Value) -> bool {
    ["code", "type"]
        .iter()
        .filter_map(|k| body["error"][k].as_str())
        .any(|v| v == "insufficient_quota")
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

    /// **接了连接却一个字节都不回**的服务端。
    ///
    /// 这是生产上真正发生的形态，也是最难发现的一种：TCP 连得上、TLS 握得成、
    /// 请求发得出去，然后没了。连接错误会立刻报，这种不会——没有超时的话
    /// `send().await` 就永远停在那里，而调用它的 worker 槽再也不释放。
    async fn a_server_that_never_answers() -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // 收下连接就攥着不放。**必须持有 socket**：一 drop 就是 FIN，
            // 那样测的又变成了连接被关闭，不是沉默
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });
        addr
    }

    /// 请求挂住时必须**报错返回**，而不是永远等下去。
    ///
    /// 没有这条守着，回归的样子是：一次灌入死在第 55 块，32 个 worker 槽被
    /// 永久占满，jobs 表里全是 running，而日志和界面上一个字都没有。
    #[tokio::test]
    async fn a_silent_server_ends_in_an_error_not_a_hang() {
        let addr = a_server_that_never_answers().await;
        let client = LlmClient::with_timeouts(
            &format!("http://{addr}"),
            None,
            "m",
            Duration::from_secs(5),
            Duration::from_millis(300),
        );
        let started = tokio::time::Instant::now();
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.chat(&[ChatMessage {
                role: "user".into(),
                content: "hi".into(),
            }]),
        )
        .await;

        // 外层 timeout 触发 = 客户端自己没有把它掐掉，正是要修的那个 bug
        let inner = out.expect("客户端没有超时，请求一直挂着");
        assert!(inner.is_err(), "沉默的服务端不该被当成成功");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "read_timeout 没生效：等了 {:?}",
            started.elapsed()
        );
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

    /// **限流要穿透 context 层被认出来。**
    ///
    /// 与 [`unreachable_survives_context_layers`] 同一个理由，后果更重：认不出来
    /// 就退回「这块废了」，而限流本来一分钟后就过去了。实测一次 1884 块的灌入里
    /// 55/60 篇文档因此整篇失败。
    #[tokio::test]
    async fn a_rate_limit_survives_context_layers() {
        let err = anyhow::Error::new(RateLimited {
            status: 429,
            retry_after: None,
            detail: "TPM limit reached".into(),
        })
        .context("抽取失败")
        .context("process_document 失败");
        let hit = rate_limited(&err).expect("限流没被认出来");
        assert_eq!(hit.status, 429);
        assert!(!err.to_string().contains("rate limiting"));
    }

    /// **没有 `Retry-After` 是常规情况，不是异常。**
    ///
    /// 多数厂商的 429 不带这个头。判定必须只看类型，退避由调用方自己出——
    /// 把 `retry_after.is_some()` 当判据的话，这些厂商一个都识别不了。
    #[tokio::test]
    async fn a_rate_limit_without_retry_after_is_still_a_rate_limit() {
        let err = anyhow::Error::new(RateLimited {
            status: 429,
            retry_after: None,
            detail: "TPM limit reached".into(),
        });
        assert!(rate_limited(&err).is_some());
        assert!(rate_limited(&err).unwrap().retry_after.is_none());
    }

    /// 反面：限流不该被当成端点不可达，两者的处置完全不同。
    #[tokio::test]
    async fn a_rate_limit_is_not_an_unreachable_endpoint() {
        let err = anyhow::Error::new(RateLimited {
            status: 429,
            retry_after: Some(Duration::from_secs(7)),
            detail: "slow down".into(),
        });
        assert!(!is_unreachable(&err));
        assert_eq!(
            rate_limited(&err).unwrap().retry_after,
            Some(Duration::from_secs(7))
        );
    }

    /// **429 不一定是限流。** OpenAI 用同一个状态码表示「太快了」和「没钱了」，
    /// 分界在 `error.code`。只按状态码分类的话，一个没钱的账号会被无限退避
    /// 重试——而重试越久，症状越像「端点慢」，越查不到根上。
    #[test]
    fn a_429_that_says_insufficient_quota_is_a_billing_problem() {
        let body = serde_json::json!({
            "error": { "message": "You exceeded your current quota", "code": "insufficient_quota" }
        });
        let e = failure("LLM", reqwest::StatusCode::TOO_MANY_REQUESTS, None, &body);
        assert!(out_of_credit(&e).is_some(), "该判成欠费");
        assert!(rate_limited(&e).is_none(), "不该判成限流");
    }

    /// 402 是标准答案，SiliconFlow 用的就是它。
    #[test]
    fn a_402_is_a_billing_problem() {
        let body = serde_json::json!({ "message": "Sorry, your account balance is insufficient" });
        let e = failure("LLM", reqwest::StatusCode::PAYMENT_REQUIRED, None, &body);
        assert!(out_of_credit(&e).is_some());
    }

    /// 反面：不带那个标识的 429 还是限流，别把会自己好的事判成要人动手。
    #[test]
    fn a_plain_429_is_still_a_rate_limit() {
        let body = serde_json::json!({ "error": { "message": "TPM limit reached" } });
        let e = failure("LLM", reqwest::StatusCode::TOO_MANY_REQUESTS, None, &body);
        assert!(rate_limited(&e).is_some());
        assert!(out_of_credit(&e).is_none());
    }

    /// 反面：别的 4xx 不是限流。密钥错了重试一万次还是错。
    #[tokio::test]
    async fn an_auth_failure_is_not_a_rate_limit() {
        let e = anyhow::anyhow!("LLM request failed (401 Unauthorized): bad key");
        assert!(rate_limited(&e).is_none());
    }

    /// 端点干干净净地回了 4xx 不算——那说明它就是模型 API，
    /// 只是密钥或模型名不对，该找的人和该做的事都不一样。
    #[tokio::test]
    async fn a_clean_api_error_is_a_different_problem() {
        let e = anyhow::anyhow!("LLM request failed (401 Unauthorized): bad key");
        assert!(!is_unreachable(&e));
    }
}
