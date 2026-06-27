//! File & Attachment Management (CRD §4.4, lines 2996-3216).
//!
//! Public signature-gated proxies are mounted among the priority routes so
//! authenticated catch-alls can never shadow them (CRD §7.1 precedence).

pub mod handlers;
pub mod limiter;
pub mod sign;
pub mod store;
pub mod validate;

use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post, put};
use axum::Router;
use std::sync::Arc;

use crate::middleware::auth::require_auth;
use crate::state::AppState;

/// Body-size ceiling for multipart upload endpoints (review #4).
/// Largest per-file cap is ADMIN_MAX (50 MB) + 1 MB multipart overhead.
const MAX_UPLOAD_BYTES: usize = 51 * 1024 * 1024;

pub fn routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // The signed direct-upload PUT carries a raw body up to ADMIN_MAX (50 MB),
    // so it needs the same body-size cap as the multipart routes — otherwise
    // axum's 2 MB default rejects legitimate larger uploads with 413 before the
    // handler's own size validation runs.
    let direct = Router::new()
        .route("/api/files/direct/{fileId}", put(handlers::direct_upload))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES));

    let public = Router::new()
        .route("/api/files/health", get(handlers::health))
        .route("/api/files/public/{*path}", get(handlers::public_proxy))
        .route(
            "/api/assets/video-placeholder.png",
            get(handlers::video_placeholder),
        )
        .route(
            "/api/files/download/{attachmentId}",
            get(handlers::public_download),
        )
        .route(
            "/api/r2-public/{folder}/{filename}",
            get(handlers::r2_public),
        )
        .merge(direct);

    // Upload routes get a body-size cap so oversized multipart bodies are
    // rejected by axum before the handler reads them into memory (review #4).
    let upload = Router::new()
        .route("/api/files", post(handlers::upload))
        .route(
            "/api/files/upload-multiple",
            post(handlers::upload_multiple),
        )
        .route(
            "/api/files/upload/{platform}",
            post(handlers::upload_platform),
        )
        .route(
            "/api/files/chunked/{sessionId}/chunk",
            post(handlers::chunked_chunk),
        )
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES));

    let authed = Router::new()
        .route("/api/files", get(handlers::list))
        .route("/api/files/info", get(handlers::info))
        // Static segments are registered alongside {fileId}; axum gives static
        // routes precedence so stats/search/batch are never captured (CRD 3066).
        .route("/api/files/stats/summary", get(handlers::stats_summary))
        .route("/api/files/search", get(handlers::search))
        .route("/api/files/batch", post(handlers::batch))
        .route(
            "/api/files/conversation/{conversationId}",
            get(handlers::conversation_files),
        )
        .route(
            "/api/files/message/{messageId}",
            get(handlers::message_files),
        )
        // LINE media carries no HMAC signature, so it must be auth-gated (H2).
        .route(
            "/api/files/line-proxy/{lineMessageId}",
            get(handlers::line_proxy),
        )
        .route(
            "/api/files/presigned-url",
            post(handlers::presigned_url).get(handlers::presigned_status),
        )
        .route(
            "/api/files/presigned-url/status",
            get(handlers::presigned_status),
        )
        .route("/api/files/chunked/init", post(handlers::chunked_init))
        .route(
            "/api/files/chunked/{sessionId}/complete",
            post(handlers::chunked_complete),
        )
        .route(
            "/api/files/chunked/{sessionId}/cancel",
            post(handlers::chunked_cancel),
        )
        .route(
            "/api/files/{fileId}",
            get(handlers::get_file).delete(handlers::delete_file),
        )
        .route(
            "/api/files/{fileId}/confirm",
            post(handlers::confirm_upload),
        )
        .route("/api/files/{fileId}/status", get(handlers::upload_status))
        .route(
            "/api/files/{fileId}/download-url",
            get(handlers::download_url),
        )
        .merge(upload)
        .layer(from_fn_with_state(state, require_auth));

    public.merge(authed)
}
