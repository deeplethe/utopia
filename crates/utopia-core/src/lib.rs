//! utopia-core: 领域模型、错误类型与配置。

pub mod config;
pub mod error;
pub mod models;
pub mod secrets;

pub use error::{is_terminal, AppError, AppResult, Terminal};
