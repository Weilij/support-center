//! Monitoring & Health per CRD §6.3 (lines 4697-4879).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::json;

async fn users(app: &TestApp) -> (String, String) {
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("agent@test.dev", "pw123456").await;
    (admin, agent)
}

#[tokio::test]
async fn public_probe_reports_composite_health() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/monitoring/health", None, None).await;
    assert_eq!(status, StatusCode::OK, "all in-process instances healthy -> 200");
    assert_eq!(body["status"], "healthy");
    assert!(body["timestamp"].is_i64(), "epoch milliseconds");
    assert_eq!(body["components"]["infrastructure"]["total"], 4);
    assert_eq!(body["components"]["circuitBreaker"]["status"], "closed");
    assert!(body["components"]["alerts"]["active"].is_number());
    assert!(body["summary"]["instancesByType"]["conversation-room"].is_number());
}

#[tokio::test]
async fn admin_gates_use_the_documented_rejection() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;
    for (method, path) in [
        ("GET", "/api/monitoring/metrics"),
        ("GET", "/api/monitoring/alerts/history"),
        ("POST", "/api/monitoring/circuit-breaker/reset"),
        ("POST", "/api/monitoring/circuit-breaker/open"),
        ("GET", "/api/monitoring/instances/conversation-room"),
        ("POST", "/api/monitoring/health-check"),
        ("GET", "/api/monitoring/dashboard"),
        ("GET", "/api/monitoring/stats"),
    ] {
        let (status, body, _) = app.request(method, path, Some(&agent), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(body["error"], "Admin access required", "{path}");
    }
    // Unauthenticated -> 401 everywhere.
    let (status, _, _) = app.request("GET", "/api/monitoring/alerts", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn metrics_detail_and_instances_by_type() {
    let app = spawn_app().await;
    let (admin, _) = users(&app).await;

    let (status, body, _) = app.request("GET", "/api/monitoring/metrics", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let instances = body["infrastructure"]["instances"].as_array().unwrap();
    assert_eq!(instances.len(), 4);
    assert!(instances[0]["latency"].is_number());
    assert!(body["infrastructure"]["summary"]["averageLatency"].is_number());
    assert_eq!(body["circuitBreaker"]["state"], "closed");

    let (status, body, _) = app
        .request("GET", "/api/monitoring/instances/message-broadcaster", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 1);

    // Unrecognized type: empty list, not an error (CRD 4775).
    let (status, body, _) = app
        .request("GET", "/api/monitoring/instances/quantum-router", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 0);
}

#[tokio::test]
async fn circuit_breaker_open_reset_cycle_with_audit() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    let (status, body, _) = app
        .request("GET", "/api/monitoring/circuit-breaker/status", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK, "any authenticated role may read");
    assert_eq!(body["state"], "closed");

    let (status, body, _) = app
        .request("POST", "/api/monitoring/circuit-breaker/open", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["newState"], "open");
    assert_eq!(body["message"], "Circuit breaker opened (emergency stop)");

    let (_, body, _) = app
        .request("GET", "/api/monitoring/circuit-breaker/status", Some(&agent), None)
        .await;
    assert_eq!(body["state"], "open", "gating state observable to all");

    let (status, body, _) = app
        .request("POST", "/api/monitoring/circuit-breaker/reset", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["newState"], "closed");
    assert_eq!(body["message"], "Circuit breaker reset successfully");

    // Both transitions audit-logged with the acting user (CRD 4758, 4767).
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action LIKE 'circuit_breaker%'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(audits, 2);
}

#[tokio::test]
async fn alerts_and_manual_sweep() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    let (status, body, _) = app.request("GET", "/api/monitoring/alerts", Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 0, "no breaching instances in-process");

    let (status, body, _) = app
        .request("POST", "/api/monitoring/health-check", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    assert_eq!(body["stats"]["totalInstances"], 4);
    assert_eq!(body["stats"]["healthyInstances"], 4);

    let (status, body, _) = app
        .request("GET", "/api/monitoring/alerts/history?limit=5", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["limit"], 5);
}

#[tokio::test]
async fn dashboard_history_config_and_stats() {
    let app = spawn_app().await;
    let (admin, _) = users(&app).await;

    let (status, body, _) = app.request("GET", "/api/monitoring/dashboard", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    assert_eq!(data["system"]["status"], "healthy");
    assert!(data["system"]["uptime"].as_str().unwrap().ends_with('%'));
    assert!(data["system"]["averageResponseTime"].as_str().unwrap().ends_with("ms"));
    assert_eq!(data["components"].as_array().unwrap().len(), 2);
    assert_eq!(data["infrastructure"]["database"]["status"], "healthy");
    assert_eq!(data["performance"]["databaseQueryTime"], 0, "boundary placeholder");

    // Health-check + history round trip.
    let (status, body, _) = app
        .request("POST", "/api/monitoring/health/check", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["overall"]["status"], "healthy");
    let (_, body, _) = app
        .request("GET", "/api/monitoring/health/history", Some(&admin), None)
        .await;
    assert!(body["data"]["total"].as_i64().unwrap() >= 2, "dashboard + manual check recorded");

    // Config validation: interval bounds 10s-5min (CRD 4806).
    let (status, body, _) = app
        .request("PUT", "/api/monitoring/config", Some(&admin),
            Some(json!({"checkInterval": 5000})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Check interval must be between 10 seconds and 5 minutes");
    let (status, body, _) = app
        .request("PUT", "/api/monitoring/config", Some(&admin),
            Some(json!({"checkInterval": 60000, "alertThresholds": {"responseTime": 2500}})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["updated"], true);

    let (_, body, _) = app.request("GET", "/api/monitoring/stats", Some(&admin), None).await;
    assert_eq!(body["data"]["monitoring"]["checkIntervalMs"], 60000, "merged config visible");
    assert!(body["data"]["monitoring"]["totalChecks"].as_i64().unwrap() >= 1);
}
