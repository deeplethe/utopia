use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use utopia_core::AppError;

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
        // Legacy errors keep their response; localizable conflicts share the code envelope.
        let mut code: Option<&'static str> = None;
        let mut detail: Option<String> = None;
        let (status, message) = match &self.0 {
            AppError::Invalid {
                code: c,
                message,
                detail: d,
            } => {
                code = Some(c);
                detail.clone_from(d);
                (StatusCode::UNPROCESSABLE_ENTITY, message.clone())
            }
            AppError::NotFound => (StatusCode::NOT_FOUND, self.0.to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.0.to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, self.0.to_string()),
            AppError::CodedConflict { code: c, message } => {
                code = Some(c);
                (StatusCode::CONFLICT, message.clone())
            }
            AppError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            AppError::Validation(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
            AppError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".into(),
                )
            }
            AppError::Other(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal server error".into(),
                )
            }
        };
        let mut body = json!({ "error": message });
        if let Some(c) = code {
            body["code"] = json!(c);
        }
        if let Some(d) = detail {
            body["detail"] = json!(d);
        }
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn conflict_codes_preserve_other_error_mappings() {
        for (error, status, code) in [
            (
                AppError::CodedConflict {
                    code: "alignment_busy",
                    message: "reworded".into(),
                },
                409,
                Some("alignment_busy"),
            ),
            (
                AppError::CodedConflict {
                    code: "another_conflict",
                    message: "busy".into(),
                },
                409,
                Some("another_conflict"),
            ),
            (AppError::Conflict("legacy".into()), 409, None),
            (AppError::Unauthorized, 401, None),
            (AppError::Forbidden, 403, None),
            (AppError::NotFound, 404, None),
            (
                AppError::invalid("bad_input", "invalid"),
                422,
                Some("bad_input"),
            ),
            (
                AppError::Other(anyhow::anyhow!("private detail")),
                500,
                None,
            ),
        ] {
            let response = ApiErr(error).into_response();
            assert_eq!(response.status().as_u16(), status);
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["code"].as_str(), code);
            if status == 500 {
                assert_eq!(body["error"], "Internal server error");
            }
        }
    }
}
