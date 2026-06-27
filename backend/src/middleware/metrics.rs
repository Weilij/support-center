//! Request metrics capture per CRD §7.1 (lines 5615-5618).

use axum::body::Body;
use axum::extract::State;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;
use std::time::Instant;

use crate::state::AppState;

const SKIP_PREFIXES: &[&str] = &[
    "/api/monitoring",
    "/api/system/health",
    "/api/system/metrics",
    "/health",
    "/static",
];

pub async fn metrics_layer(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let skip = method == axum::http::Method::OPTIONS
        || !path.starts_with("/api")
        || SKIP_PREFIXES.iter().any(|p| path.starts_with(p));

    if skip {
        return next.run(req).await;
    }

    let start = Instant::now();
    let resp = next.run(req).await;
    let elapsed_ms = start.elapsed().as_millis() as i64;
    let status = resp.status().as_u16() as i64;
    let normalized = normalize_path(&path);
    let db = state.db.clone();
    // Emission is async and can never alter or fail the request.
    tokio::spawn(async move {
        let tags = serde_json::json!({
            "method": method.as_str(),
            "path": normalized,
            "status": status,
        })
        .to_string();
        if let Err(error) = sqlx::query(
            "INSERT INTO metrics (name, value, timestamp, tags, unit) VALUES ('http_request', $1, $2, $3, 'ms')",
        )
        .bind(elapsed_ms)
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(&tags)
        .execute(&db)
        .await
        {
            tracing::warn!(error = %error, tags, "request metrics insert failed");
        }
    });
    resp
}

/// Collapse dynamic id segments (numeric, UUID-form, long hex) so cardinality stays bounded.
pub fn normalize_path(path: &str) -> String {
    path.split('/')
        .map(|seg| if is_dynamic_segment(seg) { ":id" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

fn is_dynamic_segment(seg: &str) -> bool {
    if seg.is_empty() {
        return false;
    }
    if seg.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    if seg.len() == 36 && uuid::Uuid::parse_str(seg).is_ok() {
        return true;
    }
    seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_hexdigit())
}
