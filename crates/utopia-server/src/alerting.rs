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
/// 判据是错误的**类型**，不匹配错误文本——调用链上任何一层加一句 context
/// 都会改文本。分类本身抽在 [`alert_for`]。
pub async fn observe_job_failure(
    state: &AppState,
    job: &utopia_store::jobs::Job,
    err: &anyhow::Error,
) {
    if job.attempts < job.max_attempts {
        return;
    }
    let Some((kind, severity)) = alert_for(err) else {
        return;
    };
    // 系统级：哪个库的任务撞上的不重要，端点与配额是整个部署共用的
    if let Err(e) = alerts::raise(
        &state.pool,
        alerts::NewAlert {
            kb_id: None,
            severity,
            kind,
            min_role: Role::Admin,
            subject_type: Some("system"),
            subject_id: None,
            detail: serde_json::json!({ "job": job.kind, "error": err.to_string() }),
        },
    )
    .await
    {
        tracing::warn!(error = %e, %kind, "上报告警失败");
        return;
    }
    state.emit_alert();
}

/// 一次任务失败该报哪一类告警，报不报。
///
/// **抽成纯函数是为了能测**：`observe_job_failure` 要 `AppState` 和一个真数据库，
/// 而这里要守的是分类本身。三类该做的事完全不同——欠费要人去充值，限流要降并发
/// 或升配额，端点不可达要查网络和地址。**混成一条会把人指向错的地方，比不报还费时间。**
///
/// 顺序是「更该先说的放前面」，而不是判据有重叠：类型本身互斥，重叠在状态码上
/// （OpenAI 的余额耗尽也回 429），那一层已经在 `utopia_llm::failure` 里分开了。
fn alert_for(err: &anyhow::Error) -> Option<(&'static str, &'static str)> {
    // 欠费排在限流之前。两者的状态码会重叠——OpenAI 的余额耗尽也回 429——
    // 而分类在 `utopia_llm::failure` 里已经做完，这里的顺序只是把「更该先说
    // 的那件事」放前面：没钱是人必须动手的，限流不是
    if utopia_llm::out_of_credit(err).is_some() {
        // `error`：它不会自己好，在有人充值之前抽取一直是停的
        return Some((alerts::kind::LLM_OUT_OF_CREDIT, "error"));
    }
    if utopia_llm::rate_limited(err).is_some() {
        // `warning` 不是 `error`：配额会自己恢复，端点挂了不会
        return Some((alerts::kind::LLM_RATE_LIMITED, "warning"));
    }
    if utopia_llm::is_unreachable(err) {
        return Some((alerts::kind::LLM_UNREACHABLE, "error"));
    }
    None
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

/// 数据源挂上了，库表结构却没摄进来。
///
/// **报的不是那次失败，是它留下的状态。** 挂载分两步：写 `kb_data_sources`，
/// 再把 schema 生成文档摄进库。第一步成了、第二步没成的时候，源是真挂着的——
/// `query_data` 会入列，而模型检索不到任何表结构，只能瞎猜列名。
///
/// 挂载那一刻的报错只有点按钮的人看得见。此后这个库就一直静默地缺着，
/// 而这正是 0009 说的那件事：真正伤人的不是失败，是失败无声。
pub async fn observe_schema_sync_failure(
    state: &AppState,
    kb_id: uuid::Uuid,
    source_id: uuid::Uuid,
    source_name: &str,
    err: &anyhow::Error,
) {
    if let Err(e) = alerts::raise(
        &state.pool,
        alerts::NewAlert {
            kb_id: Some(kb_id),
            severity: "warning",
            kind: alerts::kind::SCHEMA_SYNC_FAILED,
            // 配置类：要修的是连接串或网络，不是内容。找 admin
            min_role: Role::Admin,
            subject_type: Some("data_source"),
            subject_id: Some(source_id),
            // 名字在这里存一份——源被删掉之后 subject_id 解析不出名字，
            // 而告警该留得住
            detail: serde_json::json!({
                "source": source_name,
                "error": err.to_string(),
            }),
        },
    )
    .await
    {
        tracing::warn!(error = %e, "上报 data_source.schema_sync_failed 失败");
        return;
    }
    state.emit_alert();
}

#[cfg(test)]
mod tests {
    use super::alert_for;
    use utopia_llm::{OutOfCredit, RateLimited};
    use utopia_store::alerts::kind;

    /// 限流与端点不可达是两类事，报的告警必须分开。
    ///
    /// 混成一条的后果不是"标签不好看"：`llm.unreachable` 说的是端点连不上，
    /// 管理员会去查网络和地址，而真正该做的是降并发或者升配额——**告警把人
    /// 指向了错误的地方**，比不报还费时间。
    #[test]
    fn a_rate_limit_is_not_an_unreachable_endpoint() {
        let err = anyhow::Error::new(RateLimited {
            status: 429,
            retry_after: None,
            detail: "TPM limit reached".into(),
        });
        assert_eq!(alert_for(&err), Some((kind::LLM_RATE_LIMITED, "warning")));
    }

    /// **判定要穿透 context 层。** 抽取那条路上至少加了一句
    /// 「限流退避 5 次仍未通过」，靠文本匹配的判定当天就废。
    #[test]
    fn it_survives_context_layers() {
        let err = anyhow::Error::new(RateLimited {
            status: 429,
            retry_after: None,
            detail: "slow down".into(),
        })
        .context("限流退避 5 次仍未通过")
        .context("extract_document 失败");
        assert_eq!(alert_for(&err), Some((kind::LLM_RATE_LIMITED, "warning")));
    }

    /// 欠费与限流必须分开：一个要人去充值，一个等一会儿就好。
    ///
    /// 报错了方向的代价是实打实的——实测 14 篇文档因为欠费整篇失败，
    /// 而当时唯一能知道原因的办法是去数据库里翻 `graph_error`。
    #[test]
    fn out_of_credit_is_not_a_rate_limit() {
        let err = anyhow::Error::new(OutOfCredit {
            status: 402,
            detail: "Sorry, your account balance is insufficient".into(),
        });
        assert_eq!(alert_for(&err), Some((kind::LLM_OUT_OF_CREDIT, "error")));
    }

    /// 判定同样要穿透 context 层。
    #[test]
    fn out_of_credit_survives_context_layers() {
        let err = anyhow::Error::new(OutOfCredit {
            status: 402,
            detail: "insufficient balance".into(),
        })
        .context("bootstrap_ontology 失败");
        assert_eq!(alert_for(&err), Some((kind::LLM_OUT_OF_CREDIT, "error")));
    }

    /// 反面：普通失败不该报告警，否则每一次解析错都弹一条，人会学会忽略它。
    #[test]
    fn an_ordinary_failure_raises_nothing() {
        let err = anyhow::anyhow!("结果解析失败").context("抽取失败");
        assert_eq!(alert_for(&err), None);
    }

    /// 反面：干净的 4xx 不是限流也不是不可达。密钥错了要换密钥，
    /// 而那既不该降并发也不该查网络。
    #[test]
    fn an_auth_failure_raises_nothing() {
        let err = anyhow::anyhow!("LLM request failed (401 Unauthorized): bad key");
        assert_eq!(alert_for(&err), None);
    }
}
