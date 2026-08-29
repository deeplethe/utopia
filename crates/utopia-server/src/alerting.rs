//! 告警的产生侧（0005）。存取在 `utopia_store::alerts`，这里只管**什么时候报**。
//!
//! 没有"什么时候消"——一次故障一行，写完不再改。见那个模块的文档。

use utopia_core::models::Role;
use utopia_store::alerts;

use crate::state::AppState;

/// 告警保留多少天。**纯原子的代价就在这里**：一个坏掉的来源按小时同步，
/// 一天写 24 行，不清理这张表会长成第二个日志文件。
///
/// 30 天够回答"上个月那次是怎么回事"，而更久远的事该去翻 `audit_events` 与日志。
const RETAIN_DAYS: i32 = 30;

/// 任务失败时看一眼是不是模型端点不可用。
///
/// **只在任务最终放弃时报**（`attempts >= max_attempts`）。中间那几次重试报了
/// 就是同一件事发三遍——重试本来就是为了不惊动人，报出去等于把它的意义抵消掉。
/// 一个任务至多一条告警，不需要任何去重状态。
///
/// 判据是错误链里有没有 [`utopia_llm::Unreachable`]，**不匹配错误文本**——
/// 调用链上任何一层加一句 context 都会改文本。
pub async fn observe_job_failure(
    state: &AppState,
    job: &utopia_store::jobs::Job,
    err: &anyhow::Error,
) {
    if job.attempts < job.max_attempts {
        return;
    }
    if !utopia_llm::is_unreachable(err) {
        return;
    }
    // 系统级：哪个库的任务撞上的不重要，端点是整个部署共用的
    if let Err(e) = alerts::raise(
        &state.pool,
        alerts::NewAlert {
            kb_id: None,
            severity: "error",
            kind: alerts::kind::LLM_UNREACHABLE,
            min_role: Role::Admin,
            subject_type: Some("system"),
            subject_id: None,
            detail: serde_json::json!({ "job": job.kind, "error": err.to_string() }),
        },
    )
    .await
    {
        tracing::warn!(error = %e, "上报 llm.unreachable 失败");
        return;
    }
    state.emit_alert();
}

/// 每天清一次过期告警。
pub fn spawn_retention_sweep(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            match alerts::purge_older_than(&state.pool, RETAIN_DAYS).await {
                Ok(n) if n > 0 => {
                    tracing::info!(count = n, days = RETAIN_DAYS, "清理过期告警");
                    state.emit_alert();
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "清理过期告警失败"),
            }
        }
    });
}
