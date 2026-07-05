//! Small cross-domain handler helpers shared by the `domain::*` modules (previously
//! copy-pasted per module).

use axum::extract::rejection::JsonRejection;
use axum::Json;

use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;

/// A fallible axum JSON body extraction (rejections mapped to a 400 by `parse_json`).
pub type JsonBody<T> = std::result::Result<Json<T>, JsonRejection>;

/// Unwrap a JSON body, turning an axum rejection into a 400 "Invalid JSON".
pub fn parse_json<T>(body: JsonBody<T>) -> Result<T> {
    body.map(|Json(b)| b)
        .map_err(|_| AppError::BadRequest("Invalid JSON".into()))
}

/// Require the caller to hold the system `admin` role (403 otherwise).
pub fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator role required".into()))
    }
}
