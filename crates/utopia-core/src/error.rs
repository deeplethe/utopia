#[derive(thiserror::Error, Debug)]
pub enum AppError {
    #[error("Not found")]
    NotFound,
    #[error("Not signed in or invalid credentials")]
    Unauthorized,
    #[error("You don't have permission to do that")]
    Forbidden,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Validation(String),
    /// 带稳定 code 的校验错误。**message 仍是英文原句**——它是给不做本地化的
    /// 客户端（MCP、CLI）与日志用的；界面拿 code 去 i18n 里查措辞。
    ///
    /// 界面语言在客户端之后，后端不再拥有 locale（见 docs/decisions/0004），
    /// 所以留在这里的字符串是永久英文。用户能撞到的都该带上 code。
    #[error("{message}")]
    Invalid {
        code: &'static str,
        message: String,
        /// 机器给的补充（cron 解析器的报错之类）。措辞归界面，细节归这里
        detail: Option<String>,
    },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        AppError::Invalid {
            code,
            message: message.into(),
            detail: None,
        }
    }
    pub fn invalid_detail(
        code: &'static str,
        message: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        AppError::Invalid {
            code,
            message: message.into(),
            detail: Some(detail.into()),
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// 标在一个**不会因为重试而变好**的失败上（见 issue #195）。
///
/// 队列的默认假设是「再等一会儿也许就好了」，多数失败确实如此：端点抖一下、
/// 数据库忙一瞬、限流一分钟就过去。余额耗尽不是——三次重试隔着 30 秒、2 分钟、
/// 4 分半，七分钟里没有人会去充值，重试只是把同一句错误重说三遍，而运维需要
/// 看见的那条「失败」被推迟了七分钟才出现。
///
/// **判据留在处理器那一侧，不在队列里。** 什么算没救跟领域有关——
/// `utopia-store` 看不见 `utopia-llm` 的错误类型，也不该看见。处理器把这个标记
/// 挂上去（`err.context(Terminal)`），队列只问「挂了没有」。
///
/// 挂上它不影响别的：告警照报（`observe_job_failure` 一并认这个标记，
/// 否则失败得更快反而没人被告知），`last_error` 照写。
#[derive(Debug, Clone, Copy)]
pub struct Terminal;

impl std::fmt::Display for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("will not recover by retrying")
    }
}

impl std::error::Error for Terminal {}

/// 这次失败被标成不必重试了吗。
///
/// **问 `anyhow::Error::is`，不沿 `chain()` 逐个问。** 标记挂成 context
/// （`e.context(Terminal)`，`main.rs` 两处都这么写）时，链上那一环的具体类型是
/// anyhow 内部的 `ContextError<Terminal, _>`，`dyn Error::is::<Terminal>()` 认不出它，
/// 于是「没救的失败不再退避」从来没有生效。`anyhow::Error::is` 会同时看 context
/// 与被包的错误，并穿过上层再套的 `context(...)`，两种挂法都认得
pub fn is_terminal(err: &anyhow::Error) -> bool {
    err.is::<Terminal>()
}

/// 这次失败不是错了，是**等一下再试**——把任务挂回 `queued`，不烧预算（#526）。
///
/// 跟 [`Terminal`] 是一对：一个是「不会变好，别重试了」，一个是「现在做不了，
/// 等一会儿再做」。两者都是领域判断——抽取器知道本体向量还没补齐，队列看不见
/// 那张图。处理器挂标记（`err.context(Deferred::new(Duration::from_secs(30)))`），
/// `mark_failed` 认这个标记并把 `run_at` 推到未来、把 `attempts` 退回去。
///
/// 跟默认退避的区别：默认的 `30s × attempts²` 是错的——它把「再试一次值得」
/// 的失败按次数指数延后，而 `Deferred` 的语义是「这个时间点过了再来」，
/// 与失败次数无关。两次都因为同一个等待挂回队列，下次 `run_at` 都是同一个
/// 偏移，不会有 60s、120s 的递增。
///
/// **与 `Terminal` 同挂时 `Terminal` 赢**：`mark_failed` 先问 [`is_terminal`]。
/// 等待也有期限——从第一次挂回去算起超过 `jobs::DEFER_WINDOW_SECS` 还在等，就不再算等待，
/// 按普通失败退避、烧预算，免得一个永远补不齐的索引让任务永远排着。
#[derive(Debug, Clone, Copy)]
pub struct Deferred {
    pub retry_in: std::time::Duration,
}

impl Deferred {
    pub fn new(retry_in: std::time::Duration) -> Self {
        Self { retry_in }
    }
}

impl std::fmt::Display for Deferred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 落进 `last_error` 的话要让人能直接读懂，所以写明秒数
        write!(f, "deferred; retry in {:?}", self.retry_in)
    }
}

impl std::error::Error for Deferred {}

/// 挂着的 `Deferred` 要等多久。与 [`is_terminal`] 同理用 anyhow 自己的 downcast，
/// 挂成 context 还是作为根都认得
pub fn is_deferred(err: &anyhow::Error) -> Option<std::time::Duration> {
    err.downcast_ref::<Deferred>().map(|d| d.retry_in)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use std::time::Duration;

    /// 标记作为根、外面套一句说明（`rss_full_content.rs` 的写法）
    #[test]
    fn terminal_is_recognised_through_context() {
        let err: anyhow::Error = anyhow::Error::new(Terminal).context("balance gone: third retry");
        assert!(is_terminal(&err));
        assert!(is_deferred(&err).is_none());
    }

    #[test]
    fn deferred_is_recognised_through_context() {
        let err: anyhow::Error = anyhow::Error::new(Deferred::new(Duration::from_secs(30)))
            .context("waiting on ontology index");
        assert!(!is_terminal(&err));
        assert_eq!(is_deferred(&err), Some(Duration::from_secs(30)));
    }

    /// 标记挂成 context（`main.rs` 与 `extraction::run` 的写法），外面再套一层也认得
    #[test]
    fn marker_attached_via_context_method_works() {
        let marked = anyhow::Error::msg("waiting")
            .context(Deferred::new(Duration::from_secs(7)))
            .context("job 42");
        assert_eq!(is_deferred(&marked), Some(Duration::from_secs(7)));

        let marked_t = anyhow::Error::msg("balance gone")
            .context(Terminal)
            .context("job 42");
        assert!(is_terminal(&marked_t));
    }

    /// 两个都挂时两个都认得；谁赢由 `mark_failed` 的提问顺序定（`Terminal` 先问）
    #[test]
    fn both_marks_are_seen() {
        let err = anyhow::Error::msg("balance gone after waiting")
            .context(Deferred::new(Duration::from_secs(10)))
            .context(Terminal);
        assert!(is_terminal(&err));
        assert_eq!(is_deferred(&err), Some(Duration::from_secs(10)));
    }

    /// 一个普通错误不该被认成 `Deferred` 或 `Terminal`
    #[test]
    fn plain_error_is_neither() {
        let err: anyhow::Error = anyhow::Error::msg("network blip");
        assert!(!is_terminal(&err));
        assert!(is_deferred(&err).is_none());
    }

    #[test]
    fn terminal_as_the_context() {
        let err = anyhow::anyhow!("model endpoint gone").context(Terminal);
        assert!(is_terminal(&err));
    }

    #[test]
    fn terminal_as_the_root() {
        let err = anyhow::Error::new(Terminal).context("source_mismatch");
        assert!(is_terminal(&err));
    }

    #[test]
    fn terminal_under_more_context() {
        let err = anyhow::anyhow!("balance gone")
            .context(Terminal)
            .context("job 42");
        assert!(is_terminal(&err));
        let io: Result<(), _> = Err(std::io::Error::other("io"));
        assert!(is_terminal(&io.context(Terminal).unwrap_err()));
    }

    #[test]
    fn a_plain_failure_is_not_terminal() {
        let err = anyhow::anyhow!("network blip").context("job 42");
        assert!(!is_terminal(&err));
    }
}
