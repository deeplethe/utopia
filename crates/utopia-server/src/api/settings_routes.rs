use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use utopia_core::models::Role;
use utopia_llm::ChatMessage;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::ApiResult;
use crate::llm_util;
use crate::state::AppState;

/// GET：脱敏视图（密钥只回传是否已配置）。
pub async fn get(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let s = utopia_store::settings::get(&state.pool, workspace_id).await?;
    Ok(Json(match s {
        None => json!({}),
        Some(s) => json!({
            "chat_base_url": s.chat_base_url,
            "chat_model": s.chat_model,
            "chat_reasoning_effort": s.chat_reasoning_effort,
            "has_chat_key": s.chat_api_key.as_deref().is_some_and(|k| !k.is_empty()),
            "embed_base_url": s.embed_base_url,
            "embed_model": s.embed_model,
            "embed_dim": s.embed_dim,
            "has_embed_key": s.embed_api_key.as_deref().is_some_and(|k| !k.is_empty()),
            "ocr_provider": s.ocr_provider,
            "ocr_base_url": s.ocr_base_url,
            "ocr_backend": s.ocr_backend,
            "ocr_model": s.ocr_model,
            "has_ocr_key": s.ocr_api_key.as_deref().is_some_and(|k| !k.is_empty()),
            "transcribe_base_url": s.transcribe_base_url,
            "transcribe_model": s.transcribe_model,
            "has_transcribe_key": s.transcribe_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        }),
    }))
}

#[derive(Deserialize)]
pub struct PutSettingsReq {
    pub chat_base_url: Option<String>,
    /// None 或空串 = 保留旧密钥
    pub chat_api_key: Option<String>,
    pub chat_model: Option<String>,
    /// minimal | low | medium | high；空 = 不带字段，端点按默认。缺席 = 不改
    pub chat_reasoning_effort: Option<String>,
    pub embed_base_url: Option<String>,
    pub embed_api_key: Option<String>,
    pub embed_model: Option<String>,
    pub embed_dim: Option<i32>,
}

pub async fn put(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PutSettingsReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let nonempty = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    utopia_store::settings::upsert(
        &state.pool,
        workspace_id,
        nonempty(&req.chat_base_url).as_deref(),
        nonempty(&req.chat_api_key).as_deref(),
        nonempty(&req.chat_model).as_deref(),
        nonempty(&req.embed_base_url).as_deref(),
        nonempty(&req.embed_api_key).as_deref(),
        nonempty(&req.embed_model).as_deref(),
        req.embed_dim,
    )
    .await?;
    if let Some(effort) = &req.chat_reasoning_effort {
        let effort = nonempty(&Some(effort.clone()));
        if let Some(e) = effort.as_deref() {
            if !matches!(e, "none" | "minimal" | "low" | "medium" | "high") {
                return Err(utopia_core::AppError::invalid(
                    "bad_reasoning_effort",
                    "reasoning effort is one of none, minimal, low, medium, high, or empty",
                )
                .into());
            }
        }
        utopia_store::settings::set_chat_reasoning_effort(
            &state.pool,
            workspace_id,
            effort.as_deref(),
        )
        .await?;
    }
    // **配好嵌入模型的这一刻，就是本体索引能开工的最早时刻。**
    //
    // 注册时自动建的默认库会装上本体包，而那一刻还没有模型：`embed_ontology`
    // 空转即报 done，此后没有任何东西会再排一次——库里躺着一份没有向量的本体，
    // 直到有人去编辑本体或跑一次类型消解。于是默认库的第一批文档必然走
    // 「全量铺本体」那条又贵又差的路。
    //
    // 在这里补一次，索引在用户还在想上传什么的时候就建起来了。抽取那边也各自
    // 兜底（见 extraction.rs），这里只是把它提前，不是唯一的保障。
    if nonempty(&req.embed_model).is_some() {
        for kb in utopia_store::kbs::list(&state.pool, workspace_id).await? {
            let _ = utopia_store::jobs::enqueue_unless_queued(
                &state.pool,
                "embed_ontology",
                serde_json::json!({ "kb_id": kb.id }),
            )
            .await;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct PutOcrReq {
    /// `mineru` 或 `ark`（0065）；老客户端不传时保留已有供应商，新设置默认 MinerU
    pub provider: Option<String>,
    /// 空 = 关掉：扫描件、图片照第一刀降级
    pub base_url: Option<String>,
    /// None 或空串 = 保留旧密钥；换供应商时不保留
    pub api_key: Option<String>,
    pub backend: Option<String>,
    /// 方舟的视觉模型名；MinerU 忽略
    pub model: Option<String>,
}

/// 版面识别服务（0040 第二刀）。单独一个接口：管理页上它是自己的一张卡片，存它不该把
/// 对话、嵌入那几列按表单里的空值抹掉。
///
/// 配上的这一刻，这个工作区里因为没有它而停在 failed 的扫描件、图片重新排进处理队列——
/// 告警里说的「配上会自己重新处理」就是这里
pub async fn put_ocr(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PutOcrReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let nonempty = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let provider = nonempty(&req.provider);
    if provider
        .as_deref()
        .is_some_and(|p| !matches!(p, "mineru" | "ark"))
    {
        return Err(utopia_core::AppError::invalid(
            "unsupported_reader_provider",
            "The OCR provider must be mineru or ark",
        )
        .into());
    }
    let base_url = nonempty(&req.base_url);
    let saved = utopia_store::settings::upsert_ocr(
        &state.pool,
        workspace_id,
        provider.as_deref(),
        base_url.as_deref(),
        nonempty(&req.api_key).as_deref(),
        nonempty(&req.backend).as_deref(),
        nonempty(&req.model).as_deref(),
    )
    .await?;
    let mut requeued = 0usize;
    if saved.ocr_ready() {
        let docs =
            utopia_store::documents::requeue_waiting_for_reader(&state.pool, workspace_id, "ocr")
                .await?;
        requeued = docs.len();
        for (document_id, kb_id) in docs {
            state.emit_document(kb_id, document_id);
        }
    }
    Ok(Json(json!({ "ok": true, "requeued": requeued })))
}

#[derive(Deserialize)]
pub struct PutTranscribeReq {
    /// 地址或模型空 = 关掉：录音照第一刀降级
    pub base_url: Option<String>,
    /// None 或空串 = 保留旧密钥
    pub api_key: Option<String>,
    pub model: Option<String>,
}

/// 转写模型（0040 第三刀）。跟版面识别服务一样单独一个接口；配上的这一刀，这个工作区里
/// 等着转写的录音重新排队——包括上一个模型分不出说话人、被拒收的那些
pub async fn put_transcribe(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<PutTranscribeReq>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let nonempty = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let (base_url, model) = (nonempty(&req.base_url), nonempty(&req.model));
    utopia_store::settings::upsert_transcribe(
        &state.pool,
        workspace_id,
        base_url.as_deref(),
        nonempty(&req.api_key).as_deref(),
        model.as_deref(),
    )
    .await?;
    let mut requeued = 0usize;
    if base_url.is_some() && model.is_some() {
        let docs = utopia_store::documents::requeue_waiting_for_reader(
            &state.pool,
            workspace_id,
            "transcribe",
        )
        .await?;
        requeued = docs.len();
        for (document_id, kb_id) in docs {
            state.emit_document(kb_id, document_id);
        }
    }
    Ok(Json(json!({ "ok": true, "requeued": requeued })))
}

/// 对话模型按任务真正发出的三种请求各试一次（#1354）：普通一问；抽取那种流式、带完成上限、
/// 温度 0 的一问；带一个工具的一问。只发普通一问的话，拒收 `max_tokens`、`temperature` 或工具的
/// 端点照样「测试通过」，毛病要等到抽取任务成片失败、对话悄悄退回一步检索时才露出来。
/// 普通一问就失败的，端点本身不通，后两问不再发，免得同一个错报三遍。每问只要一个词，
/// 托管模型上花不了几个 token。后两问同时发，每一问都有 [`PROBE_LIMIT`] 的上限：接了连接却
/// 不说话的端点，最多让人等两个上限，而不是三次五分钟的沉默超时
async fn chat_checks(client: &utopia_llm::LlmClient) -> serde_json::Value {
    let msg = [ChatMessage {
        role: "user".into(),
        content: "Reply with exactly one word: OK".into(),
    }];
    let reply = match limited(client.chat(&msg)).await {
        Ok(reply) => reply.chars().take(50).collect::<String>(),
        Err(e) => return json!({ "ok": false, "error": e.to_string() }),
    };
    let tool = json!([{ "type": "function", "function": {
        "name": "ping", "description": "Answers pong.",
        "parameters": { "type": "object", "properties": {} } } }]);
    let ask = [json!({ "role": "user", "content": "Call the ping tool." })];
    let (streaming, tools) = tokio::join!(
        limited(async { client.chat_at_streaming(&msg, Some(0.0)).await.map(|_| ()) }),
        limited(async { client.chat_tools(&ask, &tool).await.map(|_| ()) }),
    );
    let check = |r: &anyhow::Result<()>| match r {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    let failed: Vec<String> = [("streaming", &streaming), ("tools", &tools)]
        .into_iter()
        .filter_map(|(name, r)| r.as_ref().err().map(|e| format!("{name}: {e}")))
        .collect();
    let mut out = json!({
        "ok": failed.is_empty(),
        "reply": reply,
        "checks": { "plain": { "ok": true }, "streaming": check(&streaming), "tools": check(&tools) },
    });
    if !failed.is_empty() {
        out["error"] = json!(format!("plain ok; {}", failed.join("; ")));
    }
    out
}

/// 设置页测试里每一问的上限
const PROBE_LIMIT: std::time::Duration = std::time::Duration::from_secs(45);

async fn limited<T>(f: impl std::future::Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    tokio::time::timeout(PROBE_LIMIT, f)
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "no answer within {} s",
                PROBE_LIMIT.as_secs()
            ))
        })
}

/// 连通性测试：对话按任务的三种请求形状各试一次（见 [`chat_checks`]）；embedding 试算一条并
/// 返回维度；版面识别服务问一声健康。
pub async fn test(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    utopia_store::workspaces::require_role(&state.pool, user.id, workspace_id, Role::Admin).await?;
    let Some(s) = utopia_store::settings::get(&state.pool, workspace_id).await? else {
        return Ok(Json(
            json!({ "chat": { "ok": false, "error": "Not configured" },
                               "embed": { "ok": false, "error": "Not configured" },
                               "ocr": { "ok": false, "error": "Not configured" },
                               "transcribe": { "ok": false, "error": "Not configured" } }),
        ));
    };

    let chat_result = match llm_util::chat_probe_client(&s) {
        None => json!({ "ok": false, "error": "Not configured" }),
        Some(client) => chat_checks(&client).await,
    };

    let embed_result = match llm_util::embed_client(&s) {
        None => json!({ "ok": false, "error": "Not configured" }),
        Some(client) => match client.embed(&["connectivity test".to_string()]).await {
            Ok(v) if !v.is_empty() => json!({ "ok": true, "dim": v[0].len() }),
            Ok(_) => json!({ "ok": false, "error": "Empty response" }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        },
    };

    let ocr_result = match crate::readers::Ocr::from_settings(&s) {
        None => json!({ "ok": false, "error": "Not configured" }),
        Some(ocr) => match ocr.health().await {
            Ok(h) => json!({ "ok": true, "version": h["version"] }),
            Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
        },
    };

    let transcribe_result = match crate::readers::Transcriber::from_settings(&s) {
        None => json!({ "ok": false, "error": "Not configured" }),
        Some(t) => match t.check().await {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "ok": false, "error": format!("{e:#}") }),
        },
    };

    Ok(Json(json!({
        "chat": chat_result,
        "embed": embed_result,
        "ocr": ocr_result,
        "transcribe": transcribe_result,
    })))
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    /// 只认普通与流式、见到工具就 400 的端点；`down` 时什么都 500。收到的请求体由测试逐个查
    async fn endpoint(down: bool) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |r: &Request| {
                let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
                if down {
                    return ResponseTemplate::new(500).set_body_string("down");
                }
                if body.get("tools").is_some() {
                    return ResponseTemplate::new(400).set_body_json(serde_json::json!({
                        "error": { "message": "tools are not supported by this deployment" } }));
                }
                if body["stream"] == true {
                    let frame = serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": "OK" }, "finish_reason": "stop" }] });
                    return ResponseTemplate::new(200)
                        .insert_header("content-type", "text/event-stream")
                        .set_body_string(format!("data: {frame}\n\ndata: [DONE]\n\n"));
                }
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "choices": [{ "message": { "role": "assistant", "content": "OK" }, "finish_reason": "stop" }] }))
            })
            .mount(&server)
            .await;
        server
    }

    async fn bodies(server: &MockServer) -> Vec<serde_json::Value> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn each_job_shape_is_sent_and_a_refused_one_is_named() {
        let server = endpoint(false).await;
        let out =
            super::chat_checks(&utopia_llm::LlmClient::new(&server.uri(), None, "scripted")).await;
        assert_eq!(out["ok"], false, "{out}");
        assert_eq!(out["checks"]["plain"]["ok"], true);
        assert_eq!(out["checks"]["streaming"]["ok"], true, "{out}");
        assert_eq!(out["checks"]["tools"]["ok"], false);
        let error = out["error"].as_str().unwrap();
        assert!(error.starts_with("plain ok; tools: "), "{error}");
        assert!(!error.contains("streaming:"), "{error}");

        let sent = bodies(&server).await;
        assert_eq!(sent.len(), 3, "{sent:?}");
        let plain = &sent[0];
        assert!(
            plain["stream"] == false
                && plain.get("tools").is_none()
                && plain.get("max_tokens").is_none(),
            "{plain}"
        );
        // 后两问同时发，到达的先后不定
        let streamed = sent
            .iter()
            .find(|b| b["stream"] == true)
            .expect("a streamed request");
        assert_eq!(streamed["temperature"], 0.0, "{streamed}");
        assert!(
            streamed["max_tokens"].as_u64().is_some_and(|n| n > 0),
            "the job's completion ceiling rides along: {streamed}"
        );
        let tooled = sent
            .iter()
            .find(|b| b.get("tools").is_some())
            .expect("a request with a tool");
        assert_eq!(tooled["tools"][0]["function"]["name"], "ping");
    }

    #[tokio::test]
    async fn an_endpoint_that_is_down_is_reported_once() {
        let server = endpoint(true).await;
        let out =
            super::chat_checks(&utopia_llm::LlmClient::new(&server.uri(), None, "scripted")).await;
        assert_eq!(out["ok"], false);
        assert!(out.get("checks").is_none(), "{out}");
        assert_eq!(
            bodies(&server).await.len(),
            1,
            "the streamed and tool shapes are not sent when the plain one fails"
        );
    }
}
