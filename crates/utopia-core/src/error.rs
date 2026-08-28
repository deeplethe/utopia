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
