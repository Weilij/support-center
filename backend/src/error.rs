//! Error taxonomy → wire mapping per CRD §7.1 (lines 5686-5697).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::json;

pub type HandlerResult<T = Response> = std::result::Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
pub struct FieldProblem {
    pub field: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String, Vec<FieldProblem>),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{message}")]
    TooManyRequests { message: String, retry_after: u64 },
    #[error("{0}")]
    ServiceUnavailable(String, &'static str),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn unauthorized() -> Self {
        Self::Unauthorized("Unauthorized".into())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Validation(..) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
            Self::ServiceUnavailable(..) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Validation(..) => "VALIDATION_ERROR",
            Self::BadRequest(_) => "VALIDATION_ERROR",
            Self::Unauthorized(_) => "UNAUTHORIZED",
            Self::Forbidden(_) => "FORBIDDEN",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Conflict(_) => "CONFLICT",
            Self::TooManyRequests { .. } => "TOO_MANY_REQUESTS",
            Self::ServiceUnavailable(_, code) => code,
            Self::Internal(_) => "INTERNAL_ERROR",
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!(error = %e, "database error");
        Self::Internal("Internal server error".into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut body = json!({
            "success": false,
            "error": self.to_string(),
            "code": self.code(),
            "timestamp": crate::db::now_iso(),
            "requestId": crate::envelope::request_id(),
        });
        if let Self::Validation(_, problems) = &self {
            body["data"] = json!({ "code": "VALIDATION_ERROR", "errors": problems });
        }
        let mut resp = (self.status(), Json(body)).into_response();
        if let Self::TooManyRequests { retry_after, .. } = &self {
            if let Ok(v) = retry_after.to_string().parse() {
                resp.headers_mut().insert("Retry-After", v);
            }
        }
        resp
    }
}
