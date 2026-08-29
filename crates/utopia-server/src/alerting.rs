//! 告警的产生侧（0005）。存取在 `utopia_store::alerts`，这里只管**什么时候报、
//! 什么时候消**。
//!
//! `llm.unreachable` 的自愈没有天然信号：一次任务成功不代表它调过模型，
//! 而给每个 LLM 调用点都插一句"成功了"是六个方法乘 N 个调用点的改动。
//! 所以做成不对称的一对——
//!
//! - **升报**：任何任务因为端点连不上而失败（错误链里有 `Unreachable`）
//! - **自愈**：只在这条告警亮着的时候才起探针，每分钟敲一次端点
//!
//! 健康的时候一次多余的网络调用都不发；坏了之后一分钟内自己关掉。

use utopia_core::models::Role;
use utopia_store::alerts;

use crate::llm_util;
use crate::state::AppState;

/// 任务失败时看一眼是不是端点连不上。**只看错误链里的类型，不匹配文本**——
/// 调用链上任何一层加一句 context 都会改文本。
pub async fn observe_job_failure(state: &AppState, err: &anyhow::Error) {
    if !utopia_llm::is_unreachable(err) {
        return;
    }
    // 系统级：kb_id = None。哪个库的任务撞上的不重要，端点是整个部署共用的
    match alerts::raise(
        &state.pool,
        alerts::NewAlert {
            // 系统级：哪个库的任务撞上的不重要，端点是整个部署共用的
            kb_id: None,
            severity: "error",
            kind: alerts::kind::LLM_UNREACHABLE,
            min_role: Role::Admin,
            subject_type: Some("system"),
            subject: None,
            detail: serde_json::json!({ "error": err.to_string() }),
        },
    )
    .await
    {
        Ok((_, changed)) => {
            // 只在**有变化**时推：端点断了之后每个任务都会撞上，
            // 每次都点亮所有人的角标等于把这条告警变成噪音
            if changed {
                state.emit_alert();
            }
        }
        Err(e) => tracing::warn!(error = %e, "上报 llm.unreachable 失败"),
    }
}

/// 端点探针。**只在 `llm.unreachable` 亮着时才真的发请求**——
/// 一分钟一次的空转在健康部署上是纯浪费，而这条告警绝大多数时间不该存在。
pub fn spawn_llm_probe(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match alerts::is_open(&state.pool, None, alerts::kind::LLM_UNREACHABLE).await {
                Ok(false) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, "查询 llm.unreachable 状态失败");
                    continue;
                }
                Ok(true) => {}
            }
            if probe_once(&state).await {
                match alerts::resolve(&state.pool, None, alerts::kind::LLM_UNREACHABLE).await {
                    Ok(true) => {
                        tracing::info!("模型端点恢复，llm.unreachable 自愈");
                        state.emit_alert();
                    }
                    Ok(false) => {}
                    Err(e) => tracing::warn!(error = %e, "清除 llm.unreachable 失败"),
                }
            }
        }
    });
}

/// 敲一次端点。判据是**拿没拿到响应**，不是响应对不对——
/// 端点回 401 说明它活着，那是密钥的问题，不该由这条告警管。
async fn probe_once(state: &AppState) -> bool {
    // 工作区级设置：任取一个配了对话模型的。端点是部署共用的，
    // 哪个工作区的配置探到的都是同一个地址
    let Ok(Some(settings)) = utopia_store::settings::any_with_chat(&state.pool).await else {
        // 没有任何工作区配了模型 —— 探不了，也就谈不上恢复。
        // 不在这里清除告警：清了就成了"配置没了所以问题解决了"
        return false;
    };
    let Some(client) = llm_util::chat_client(&settings) else {
        return false;
    };
    let msg = [utopia_llm::ChatMessage {
        role: "user".into(),
        content: "ping".into(),
    }];
    match client.chat(&msg).await {
        Ok(_) => true,
        Err(e) => !utopia_llm::is_unreachable(&e),
    }
}
