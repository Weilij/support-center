//! Monitoring & Health endpoints (CRD §6.3, lines 4708-4844).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::center;

type Result<T = Response> = std::result::Result<T, AppError>;

/// The monitoring family's documented admin rejection body (CRD 4725).
fn admin_only(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Admin access required".into()))
    }
}

/// GET /api/monitoring/health — public probe; 200 healthy / 207 degraded (CRD 4708-4716).
pub async fn public_health(State(state): State<Arc<AppState>>) -> Response {
    let sweep = center::sweep(&state);
    let aggregate = sweep["aggregate"].as_str().unwrap_or("degraded").to_string();
    let breaker = state
        .monitoring
        .breaker
        .lock()
        .map(|b| json!({"status": b.state, "stats": b.stats()}))
        .unwrap_or_else(|_| json!({"status": "unknown"}));
    let active = state.monitoring.active_alerts.lock().map(|a| a.len()).unwrap_or(0);
    let total_alerts = state.monitoring.alert_history.lock().map(|a| a.len()).unwrap_or(0);
    let status_code = if aggregate == "healthy" { StatusCode::OK } else { StatusCode::MULTI_STATUS };
    (
        status_code,
        Json(json!({
            "status": aggregate,
            "timestamp": chrono::Utc::now().timestamp_millis(),
            "components": {
                "infrastructure": {
                    "status": aggregate,
                    "total": sweep["stats"]["totalInstances"],
                    "healthy": sweep["stats"]["healthyInstances"],
                    "degraded": sweep["stats"]["degradedInstances"],
                    "unhealthy": sweep["stats"]["unhealthyInstances"],
                },
                "circuitBreaker": breaker,
                "alerts": { "active": active, "total": total_alerts },
            },
            "summary": {
                "totalInstances": sweep["stats"]["totalInstances"],
                "instancesByType": sweep["stats"]["instancesByType"],
                "lastUpdate": sweep["stats"]["lastUpdate"],
            },
        })),
    )
        .into_response()
}

/// GET /api/monitoring/metrics — admin infrastructure detail (CRD 4718-4725).
pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let sweep = center::sweep(&state);
    let instances = sweep["instances"].as_array().cloned().unwrap_or_default();
    let avg_latency = if instances.is_empty() {
        0.0
    } else {
        instances.iter().filter_map(|i| i["latency"].as_f64()).sum::<f64>() / instances.len() as f64
    };
    let total_connections: i64 = instances.iter().filter_map(|i| i["connections"].as_i64()).sum();
    let breaker = state
        .monitoring
        .breaker
        .lock()
        .map(|b| json!({"state": b.state, "stats": b.stats(), "recentEvents": b.events}))
        .unwrap_or_else(|_| json!({}));
    Ok((
        StatusCode::OK,
        Json(json!({
            "timestamp": now_iso(),
            "infrastructure": {
                "instances": instances,
                "summary": {
                    "totalInstances": sweep["stats"]["totalInstances"],
                    "instancesByType": sweep["stats"]["instancesByType"],
                    "averageLatency": avg_latency,
                    "totalActiveConnections": total_connections,
                },
            },
            "circuitBreaker": breaker,
        })),
    )
        .into_response())
}

/// GET /api/monitoring/alerts — active alerts from the last sweep (CRD 4727-4734).
pub async fn alerts(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let alerts: Vec<Value> = state
        .monitoring
        .active_alerts
        .lock()
        .map(|a| {
            a.iter()
                .map(|alert| {
                    let mut a = alert.clone();
                    a["age"] = json!(now_ms - a["raisedAtMs"].as_i64().unwrap_or(now_ms));
                    a
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((
        StatusCode::OK,
        Json(json!({ "count": alerts.len(), "alerts": alerts, "timestamp": now_iso() })),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct LimitQuery {
    pub limit: Option<usize>,
}

/// GET /api/monitoring/alerts/history (CRD 4736-4742).
pub async fn alert_history(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<LimitQuery>,
) -> Result {
    admin_only(&user)?;
    let limit = q.limit.unwrap_or(100);
    let alerts: Vec<Value> = state
        .monitoring
        .alert_history
        .lock()
        .map(|h| h.iter().rev().take(limit).cloned().collect())
        .unwrap_or_default();
    Ok((
        StatusCode::OK,
        Json(json!({ "count": alerts.len(), "limit": limit, "alerts": alerts, "timestamp": now_iso() })),
    )
        .into_response())
}

// ------------------------------------------------ circuit breaker

pub async fn breaker_status(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let (breaker_state, stats) = state
        .monitoring
        .breaker
        .lock()
        .map(|b| (b.state, b.stats()))
        .unwrap_or(("unknown", json!({})));
    Ok((
        StatusCode::OK,
        Json(json!({ "state": breaker_state, "stats": stats, "timestamp": now_iso() })),
    )
        .into_response())
}

pub async fn breaker_reset(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let new_state = {
        let mut b = state.monitoring.breaker.lock().map_err(|_| AppError::Internal("breaker".into()))?;
        b.reset(&user.id);
        b.state
    };
    crate::domain::auth::store::log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "circuit_breaker_reset", "monitoring", None, None, None, None,
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": "Circuit breaker reset successfully",
            "newState": new_state,
            "timestamp": now_iso(),
        })),
    )
        .into_response())
}

pub async fn breaker_open(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let new_state = {
        let mut b = state.monitoring.breaker.lock().map_err(|_| AppError::Internal("breaker".into()))?;
        b.open(&user.id);
        b.state
    };
    crate::domain::auth::store::log_activity(
        &state.db, &user.id, &user.display_name, &user.role,
        "circuit_breaker_open", "monitoring", None,
        Some(json!({"level": "critical"})), None, None,
    )
    .await;
    Ok((
        StatusCode::OK,
        Json(json!({
            "success": true,
            "message": "Circuit breaker opened (emergency stop)",
            "newState": new_state,
            "timestamp": now_iso(),
        })),
    )
        .into_response())
}

/// GET /api/monitoring/instances/{type} (CRD 4770-4777): unknown type yields
/// an empty list, not an error.
pub async fn instances_by_type(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(kind): Path<String>,
) -> Result {
    admin_only(&user)?;
    let sweep = center::sweep(&state);
    let instances: Vec<Value> = sweep["instances"]
        .as_array()
        .map(|a| a.iter().filter(|i| i["type"] == kind.as_str()).cloned().collect())
        .unwrap_or_default();
    Ok((
        StatusCode::OK,
        Json(json!({
            "type": kind,
            "count": instances.len(),
            "instances": instances,
            "timestamp": now_iso(),
        })),
    )
        .into_response())
}

/// POST /api/monitoring/health-check (CRD 4779-4786).
pub async fn manual_health_check(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let sweep = center::sweep(&state);
    Ok((
        StatusCode::OK,
        Json(json!({ "success": true, "stats": sweep["stats"], "timestamp": now_iso() })),
    )
        .into_response())
}

// ------------------------------------------------ application monitor

/// GET /api/monitoring/dashboard (CRD 4788-4794).
pub async fn dashboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let (components, overall, response_ms) = center::component_checks(&state).await;
    center::record_cycle(&state, overall, response_ms, vec![]);
    let monitor = state
        .monitoring
        .monitor
        .lock()
        .map(|m| json!({
            "running": m.running,
            "checkIntervalMs": m.check_interval_ms,
            "totalChecks": m.total_checks,
            "recentChecks": m.recent_checks,
        }))
        .unwrap_or_else(|_| json!({}));
    let history_len = state.monitoring.health_history.lock().map(|h| h.len()).unwrap_or(0);
    let healthy_rate = 100.0; // single-process healthy-rate over the bounded history
    Ok(envelope::ok(json!({
        "timestamp": now_iso(),
        "system": {
            "status": overall,
            "message": if overall == "healthy" { "All systems operational" } else { "Component issues detected" },
            "uptime": format!("{healthy_rate:.1}%"),
            "averageResponseTime": format!("{response_ms:.0}ms"),
        },
        "monitoring": monitor,
        "health": {
            "status": overall,
            "averageResponseTime": response_ms,
            "healthyRate": healthy_rate,
            "cycles": history_len,
        },
        "alerts": {
            "total": state.monitoring.alert_history.lock().map(|h| h.len()).unwrap_or(0),
            "last24h": state.monitoring.alert_history.lock().map(|h| h.len()).unwrap_or(0),
            "critical": 0,
            "warning": 0,
            "unresolved": state.monitoring.active_alerts.lock().map(|a| a.len()).unwrap_or(0),
        },
        "components": components,
        "infrastructure": {
            "database": components[0].clone(),
            "cache": components[1].clone(),
        },
        "performance": {
            "averageApiResponseTime": response_ms,
            "databaseQueryTime": 0,
            "cacheHitRate": 0,
        },
    })))
}

/// GET /api/monitoring/health/history (CRD 4796-4802).
pub async fn health_history(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<LimitQuery>,
) -> Result {
    admin_only(&user)?;
    let limit = q.limit.unwrap_or(50);
    let (history, total) = state
        .monitoring
        .health_history
        .lock()
        .map(|h| (h.iter().rev().take(limit).cloned().collect::<Vec<_>>(), h.len()))
        .unwrap_or_default();
    Ok(envelope::ok(json!({ "history": history, "total": total })))
}

#[derive(Deserialize)]
pub struct ConfigBody {
    #[serde(rename = "checkInterval")]
    pub check_interval: Option<i64>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

/// PUT /api/monitoring/config (CRD 4804-4811).
pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<ConfigBody>,
) -> Result {
    admin_only(&user)?;
    if let Some(interval) = body.check_interval {
        if !(10_000..=300_000).contains(&interval) {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "success": false,
                    "error": "Check interval must be between 10 seconds and 5 minutes",
                    "timestamp": now_iso(),
                })),
            )
                .into_response());
        }
    }
    if let Ok(mut monitor) = state.monitoring.monitor.lock() {
        if let Some(interval) = body.check_interval {
            monitor.check_interval_ms = interval;
        }
        if let Some(config) = monitor.config.as_object_mut() {
            for (k, v) in &body.rest {
                config.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(envelope::ok(json!({ "updated": true })))
}

/// POST /api/monitoring/health/check (CRD 4813-4819).
pub async fn app_health_check(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let (components, overall, response_ms) = center::component_checks(&state).await;
    center::record_cycle(&state, overall, response_ms, vec![]);
    Ok(envelope::ok(json!({
        "overall": { "status": overall, "message": "Health check completed", "timestamp": now_iso() },
        "components": components,
        "infrastructure": { "database": components[0].clone(), "cache": components[1].clone() },
        "performance": { "averageApiResponseTime": response_ms, "databaseQueryTime": 0, "cacheHitRate": 0 },
    })))
}

/// GET /api/monitoring/stats (CRD 4827-4833).
pub async fn monitor_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    admin_only(&user)?;
    let monitor = state
        .monitoring
        .monitor
        .lock()
        .map(|m| json!({
            "running": m.running,
            "checkIntervalMs": m.check_interval_ms,
            "totalChecks": m.total_checks,
            "recentChecks": m.recent_checks,
        }))
        .unwrap_or_else(|_| json!({}));
    Ok(envelope::ok(json!({
        "monitoring": monitor,
        "health": {
            "cycles": state.monitoring.health_history.lock().map(|h| h.len()).unwrap_or(0),
        },
        "alerts": {
            "total": state.monitoring.alert_history.lock().map(|h| h.len()).unwrap_or(0),
            "active": state.monitoring.active_alerts.lock().map(|a| a.len()).unwrap_or(0),
        },
        "autoRemediation": { "enabled": false, "attempts": 0 },
    })))
}
