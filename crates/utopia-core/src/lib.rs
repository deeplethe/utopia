//! utopia-core: 领域模型、错误类型与配置。

pub mod config;
pub mod error;
pub mod models;
pub mod secrets;
pub mod text;

pub use error::{is_deferred, is_terminal, AppError, AppResult, Deferred, Terminal};
pub use text::without_nul;
