//! Queue monitoring endpoints (CRD 5196-5223). Figures are fixed within the
//! current behavioral boundary except where the live stats snapshot applies.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

pub async fn stats(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let snapshot = state.queue.stats.lock().map(|s| s.clone()).unwrap_or_default();
    Ok(envelope::ok(json!({
        "summary": {
            "totalQueues": 2,
            "healthyQueues": 2,
            "totalMessages": snapshot.total_processed,
            "status": "healthy",
        },
        "queues": {
            "messageQueue": {
                "name": "message-queue",
                "label": "MESSAGE_QUEUE",
                "purpose": "Outbound LINE delivery and inbound media processing",
                "status": "healthy",
                "metrics": {
                    "messagesInQueue": 0,
                    "processingRate": "real-time",
                    "errorRate": snapshot.errors,
                    "averageProcessingTimeMs": snapshot.average_ms() as u64,
                },
                "configuration": {
                    "maxBatchSize": super::MAX_BATCH_SIZE,
                    "maxBatchTimeout": 5,
                    "retryPolicy": "exponential-backoff",
                },
            },
            "deadLetterQueue": {
                "name": "dead-letter-queue",
                "size": state.queue.dead_letter_size(),
            },
        },
        "systemHealth": {
            "uptime": 0,
            "lastCheck": now_iso(),
        },
    })))
}

pub async fn health(Extension(_user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok(json!({
        "queues": { "available": true, "processingLatency": "<100ms" },
        "status": "healthy",
        "timestamp": now_iso(),
    })))
}

pub async fn performance(Extension(_user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok(json!({
        "throughput": {
            "messagesPerSecond": 100,
            "peak": 500,
            "averageProcessingTimeMs": 50,
        },
        "reliability": {
            "successRate": 99.9,
            "errorRate": 0.1,
            "retryRate": 0.5,
        },
        "timestamp": now_iso(),
    })))
}

#[derive(serde::Deserialize)]
pub struct MaintenanceBody {
    pub operation: Option<String>,
}

pub async fn maintenance(
    Extension(_user): Extension<AuthUser>,
    Json(body): Json<MaintenanceBody>,
) -> Result {
    const AVAILABLE: &[&str] = &["status"];
    match body.operation.as_deref() {
        Some("status") => Ok(envelope::ok(json!({
            "queueStatus": "healthy",
            "timestamp": now_iso(),
        }))),
        other => {
            let body: Value = json!({
                "success": false,
                "error": format!("Unknown operation '{}'", other.unwrap_or("")),
                "availableOperations": AVAILABLE,
                "timestamp": now_iso(),
            });
            Ok((axum::http::StatusCode::BAD_REQUEST, Json(body)).into_response())
        }
    }
}
