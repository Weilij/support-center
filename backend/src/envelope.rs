//! Canonical response envelopes per CRD §7.1 (lines 5648-5658).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

pub fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn now_iso() -> String {
    crate::db::now_iso()
}

/// `{success:true, data, timestamp, requestId}` with status 200.
pub fn ok<T: Serialize>(data: T) -> Response {
    with_status(StatusCode::OK, Some(json!(data)), None)
}

/// Success with a human message alongside data.
pub fn ok_msg<T: Serialize>(data: T, message: &str) -> Response {
    with_status(StatusCode::OK, Some(json!(data)), Some(message))
}

/// Success carrying only a message (no data payload).
pub fn message_only(message: &str) -> Response {
    with_status(StatusCode::OK, None, Some(message))
}

/// `201 Created` success envelope.
pub fn created<T: Serialize>(data: T) -> Response {
    with_status(StatusCode::CREATED, Some(json!(data)), None)
}

pub fn with_status(status: StatusCode, data: Option<Value>, message: Option<&str>) -> Response {
    let mut body = json!({
        "success": true,
        "timestamp": now_iso(),
        "requestId": request_id(),
    });
    if let Some(d) = data {
        body["data"] = d;
    }
    if let Some(m) = message {
        body["message"] = json!(m);
    }
    (status, Json(body)).into_response()
}

/// Success envelope whose `pagination` block sits beside `data` (teams/agents listings,
/// CRD 1828, 2168).
pub fn ok_with_pagination(items: Vec<Value>, page: i64, limit: i64, total: i64) -> Response {
    let total_pages = if total == 0 {
        0
    } else {
        (total + limit - 1) / limit
    };
    let body = json!({
        "success": true,
        "data": items,
        "pagination": { "page": page, "limit": limit, "total": total, "totalPages": total_pages },
        "timestamp": now_iso(),
        "requestId": request_id(),
    });
    (StatusCode::OK, Json(body)).into_response()
}

/// 200 envelope whose top-level `success` flag is caller-controlled (used by operations
/// whose overall success mirrors per-item outcomes, CRD 1886, 1922, 2185).
pub fn flagged(success: bool, data: Value, message: Option<&str>) -> Response {
    let mut body = json!({
        "success": success,
        "data": data,
        "timestamp": now_iso(),
        "requestId": request_id(),
    });
    if let Some(m) = message {
        body["message"] = json!(m);
    }
    (StatusCode::OK, Json(body)).into_response()
}

/// Pagination clamping per CRD line 5663: out-of-range values are clamped, not rejected.
/// Defaults: page 1, size 20; size 1..=100; page 1..=1000.
pub fn clamp_page(page: Option<i64>, page_size: Option<i64>) -> (i64, i64) {
    let page = page.unwrap_or(1).clamp(1, 1000);
    let size = page_size.unwrap_or(20).clamp(1, 100);
    (page, size)
}

/// Canonical paginated envelope (CRD line 5652): items + page/pageSize/limit/total/totalPages
/// + hasNext/hasPrev, wrapped in the success envelope.
pub fn paginated<T: Serialize>(items: &[T], page: i64, page_size: i64, total: i64) -> Response {
    let total_pages = if total == 0 {
        0
    } else {
        (total + page_size - 1) / page_size
    };
    ok(json!({
        "items": items,
        "page": page,
        "pageSize": page_size,
        "limit": page_size,
        "total": total,
        "totalPages": total_pages,
        "hasNext": page < total_pages,
        "hasPrev": page > 1 && total_pages > 0,
    }))
}
