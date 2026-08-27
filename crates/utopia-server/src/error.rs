use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use utopia_core::AppError;
use serde_json::json;

/// axum 响应包装（orphan rule：IntoResponse 不能直接实现在 core 类型上）。
pub struct ApiErr(pub AppError);

impl<E: Into<AppError>> From<E> for ApiErr {
    fn from(e: E) -> Self {
        ApiErr(e.into())
    }
}

pub type ApiResult<T> = Result<T, ApiErr>;

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        let (status, message) = match &self.0 {
            AppError::NotFound => (StatusCode::NOT_FOUND, self.0.to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.0.to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, self.0.to_string()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            AppError::Validation(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
            AppError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".into())
            }
            AppError::Other(e) => {
                tracing::error!(error = %e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".into())
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
