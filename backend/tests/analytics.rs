//! Analytics per CRD §6.1 (lines 4203-4503).

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

async fn seed_conversations(app: &TestApp, team: i64, n: usize, closed: usize) {
    for i in 0..n {
        let customer = app.seed_customer("line", &format!("U-an-{team}-{i}"), "C", Some(team)).await;
        let status = if i < closed { "closed" } else { "active" };
        app.seed_conversation(customer, Some(team), status).await;
    }
}

// ---------------------------------------------------------------- core

#[tokio::test]
async fn conversation_analytics_summary_trends_distribution() {
    let app = spawn_app().await;
    let (admin, _) = users(&app).await;
    let team = app.seed_team("AN").await;
    seed_conversations(&app, team, 5, 2).await;

    let (status, body, _) = app
        .request("GET", "/api/analytics/conversations?timeRange=7d", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"]["data"];
    assert_eq!(data["summary"]["totalConversations"], 5);
    assert_eq!(data["summary"]["activeConversations"], 3);
    assert_eq!(data["summary"]["closedConversations"], 2);
    assert!(!data["trends"].as_array().unwrap().is_empty());
    let dist = data["distribution"].as_array().unwrap();
    assert_eq!(dist[0]["category"], "line");
    assert_eq!(dist[0]["value"], 5);
    assert_eq!(dist[0]["percentage"], 100.0);
    let meta = &body["data"]["metadata"];
    assert_eq!(meta["aggregation"], "daily");
    assert!(meta["queryDurationMs"].is_i64());

    // Invalid explicit range: start after end -> 400.
    let (status, _, _) = app
        .request("GET",
            "/api/analytics/conversations?startDate=2026-06-10T00:00:00Z&endDate=2026-06-01T00:00:00Z",
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn non_admins_are_scoped_to_their_team() {
    let app = spawn_app().await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    seed_conversations(&app, team_a, 3, 0).await;
    seed_conversations(&app, team_b, 2, 0).await;
    let agent_id = app.seed_agent("scoped@test.dev", "pw123456", "agent").await;
    app.add_membership(&agent_id, team_a, "member", true).await;
    let (agent, _, _) = app.login("scoped@test.dev", "pw123456").await;

    let (_, body, _) = app
        .request("GET", "/api/analytics/conversations", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["data"]["summary"]["totalConversations"], 3,
        "caller's team injected when no filter supplied");
}

#[tokio::test]
async fn message_and_performance_analytics() {
    let app = spawn_app().await;
    let (admin, _) = users(&app).await;
    let team = app.seed_team("M").await;
    let customer = app.seed_customer("line", "U-msg", "C", Some(team)).await;
    let conversation = app.seed_conversation(customer, Some(team), "active").await;
    for i in 0..4 {
        app.seed_message(&conversation, "customer", &format!("m{i}"), None).await;
    }

    let (status, body, _) = app
        .request("GET", "/api/analytics/messages", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["data"]["summary"]["totalMessages"], 4);
    assert!(body["data"]["data"]["summary"]["messagesPerHour"].as_f64().unwrap() > 0.0);
    assert_eq!(body["data"]["data"]["typeDistribution"][0]["category"], "text");

    let (status, body, _) = app
        .request("GET", "/api/analytics/performance", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["data"]["summary"]["uptimePercent"].is_number());
    assert_eq!(body["data"]["metadata"]["aggregation"], "hourly", "24h default window");
}

#[tokio::test]
async fn custom_query_and_export_require_query_permission() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    let (status, _, _) = app
        .request("POST", "/api/analytics/custom", Some(&agent),
            Some(json!({"query": "conversations"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "plain agents lack the query level");

    let (status, _, _) = app
        .request("POST", "/api/analytics/custom", Some(&admin),
            Some(json!({"query": "DROP TABLE agents"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "only safe named datasets run");

    let (status, body, _) = app
        .request("POST", "/api/analytics/custom", Some(&admin),
            Some(json!({"query": "conversations"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["data"].is_array());
    assert_eq!(body["data"]["metadata"]["cacheHit"], false);

    // Export: bad metric family -> validation error; valid -> artifact descriptor.
    let (status, _, _) = app
        .request("POST", "/api/analytics/export", Some(&admin),
            Some(json!({"metrics": ["quantum_flux"]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app
        .request("POST", "/api/analytics/export", Some(&admin),
            Some(json!({"format": "csv", "metrics": ["total_conversations"]})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["downloadUrl"].as_str().unwrap().contains("sig="));
    assert!(data["fileName"].as_str().unwrap().ends_with(".csv"));
    assert_eq!(data["downloadCount"], 0);
    assert!(data["expiresAt"].is_string(), "24h expiry advertised");
}

#[tokio::test]
async fn metrics_record_and_bucketed_query() {
    let app = spawn_app().await;
    let (admin, _) = users(&app).await;

    let (status, _, _) = app
        .request("POST", "/api/analytics/metrics", Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing metrics data");
    let (status, _, _) = app
        .request("POST", "/api/analytics/metrics", Some(&admin),
            Some(json!({"metric": {"id": "m1", "name": "cpu", "value": "high",
                                   "timestamp": 1, "tags": {}}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "non-numeric value rejected");

    let base = chrono::Utc::now().timestamp_millis();
    let batch: Vec<_> = (0..4)
        .map(|i| json!({"id": format!("m{i}"), "name": "cpu", "value": (i + 1) as f64 * 10.0,
                        "timestamp": base + i * 1000, "tags": {"host": "a"}}))
        .collect();
    let (status, _, _) = app
        .request("POST", "/api/analytics/metrics", Some(&admin), Some(json!({"metrics": batch})))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Raw query.
    let (status, body, _) = app
        .request("GET", &format!("/api/analytics/metrics/cpu?startTime={}&endTime={}",
            base - 1000, base + 10_000), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["metrics"].as_array().unwrap().len(), 4);

    // Aggregated query: avg of 10,20,30,40 over one 1h bucket = 25.
    let (_, body, _) = app
        .request("GET", &format!(
            "/api/analytics/metrics/cpu?startTime={}&endTime={}&aggregation=avg&period=1h",
            base - 1000, base + 10_000), Some(&admin), None)
        .await;
    let entries = body["data"]["metrics"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["value"], 25.0);
    assert_eq!(entries[0]["sampleCount"], 4);

    // Invalid range: start not before end.
    let (status, _, _) = app
        .request("GET", "/api/analytics/metrics/cpu?startTime=10&endTime=5", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- comparison

#[tokio::test]
async fn comparison_single_multi_and_presets() {
    let app = spawn_app().await;
    let (admin, _) = users(&app).await;
    let team = app.seed_team("CMP").await;
    seed_conversations(&app, team, 4, 1).await;

    let now = chrono::Utc::now();
    // 'Z'-suffixed timestamps: '+00:00' would decode as a space in a query string.
    let cur_s = (now - chrono::Duration::days(7)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
    // End one minute ahead so same-second fractional timestamps stay inside.
    let cur_e = (now + chrono::Duration::minutes(1)).format("%Y-%m-%dT%H:%M:%SZ").to_string();

    // Missing params -> 400.
    let (status, _, _) = app
        .request("GET", "/api/analytics/comparison/metric?metric=total_conversations",
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request("GET", &format!(
            "/api/analytics/comparison/metric?metric=total_conversations&currentStart={cur_s}&currentEnd={cur_e}"),
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cmp = &body["data"]["comparison"];
    assert_eq!(cmp["current"], 4.0);
    assert_eq!(cmp["previous"], 0.0);
    // Zero prior + positive current -> 100% and an upward trend (CRD 4298).
    assert_eq!(cmp["changePercent"], 100.0);
    assert_eq!(cmp["trend"], "up");
    assert!(cmp["previousPeriod"]["label"].is_string());

    // Unknown metric yields 0 / stable.
    let (_, body, _) = app
        .request("GET", &format!(
            "/api/analytics/comparison/metric?metric=quantum&currentStart={cur_s}&currentEnd={cur_e}"),
            Some(&admin), None)
        .await;
    assert_eq!(body["data"]["comparison"]["current"], 0.0);
    assert_eq!(body["data"]["comparison"]["trend"], "stable");

    // Multi-metric with overall verdict.
    let (status, body, _) = app
        .request("GET", &format!(
            "/api/analytics/comparison/metrics?metrics=total_conversations,closed_conversations&currentStart={cur_s}&currentEnd={cur_e}"),
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let summary = &body["data"]["comparison"]["summary"];
    assert_eq!(summary["totalMetrics"], 2);
    assert_eq!(summary["overallTrend"], "positive");

    // Preset + cache stats.
    let (status, body, _) = app
        .request("GET", &format!(
            "/api/analytics/comparison/preset/conversation?currentStart={cur_s}&currentEnd={cur_e}"),
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["metadata"]["preset"], "conversation");
    let (status, _, _) = app
        .request("GET", "/api/analytics/comparison/cache/stats", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------- dashboards

#[tokio::test]
async fn dashboard_config_data_and_widgets() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    // Widget types + templates.
    let (_, body, _) = app
        .request("GET", "/api/analytics/dashboard/widget-types", Some(&agent), None)
        .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 6);
    let (_, body, _) = app
        .request("GET", "/api/analytics/dashboard/templates?category=operations", Some(&agent), None)
        .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    // Save validation + defaults.
    let (status, _, _) = app
        .request("POST", "/api/analytics/dashboard/config", Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "name required");
    let (status, body, _) = app
        .request("POST", "/api/analytics/dashboard/config/main", Some(&admin),
            Some(json!({"name": "Main", "widgets": [
                {"id": "w1", "type": "metric", "title": "Total",
                 "dataSource": {"type": "analytics", "query": "total_conversations"},
                 "position": {"x": 0, "y": 0, "width": 4, "height": 2}}
            ]})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["refreshInterval"], 30000, "default applied");
    assert_eq!(body["data"]["permissions"]["owner"], "admin@test.dev".split('@').next().map(|_| body["data"]["permissions"]["owner"].as_str().unwrap()).unwrap());

    // Get config + dashboard data resolution.
    let (_, body, _) = app
        .request("GET", "/api/analytics/dashboard/config/main", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["name"], "Main");
    let (status, body, _) = app
        .request("GET", "/api/analytics/dashboard/data/main", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["w1"]["data"]["value"], 0);

    // Bad timeRange -> 400; widget data + 404.
    let (status, _, _) = app
        .request("GET", "/api/analytics/dashboard/data/main?timeRange=not-json", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("GET", "/api/analytics/dashboard/widget/w1/data?dashboardId=main", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request("GET", "/api/analytics/dashboard/widget/ghost/data?dashboardId=main", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Widget create/clone/role gates.
    let (status, _, _) = app
        .request("POST", "/api/analytics/dashboard/widget", Some(&agent),
            Some(json!({"type": "metric", "title": "X",
                        "dataSource": {"type": "analytics", "query": "q"},
                        "position": {"x": 0, "y": 0, "width": 2, "height": 2}})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "plain agent lacks team-level role");
    let (status, body, _) = app
        .request("POST", "/api/analytics/dashboard/widget/w1/clone", Some(&admin),
            Some(json!({"dashboardId": "main"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Layout optimize reflows within the container.
    let (status, body, _) = app
        .request("POST", "/api/analytics/dashboard/layout/optimize", Some(&admin),
            Some(json!({"dashboardId": "main", "containerWidth": 4})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let widgets = body["data"]["widgets"].as_array().unwrap();
    assert_eq!(widgets.len(), 2);
    assert_eq!(widgets[1]["position"]["y"], 2, "second widget wrapped to the next row");
}

#[tokio::test]
async fn realtime_dashboard_control() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;
    app.request("POST", "/api/analytics/dashboard/config/rt", Some(&admin),
        Some(json!({"name": "RT", "widgets": [
            {"id": "w1", "type": "metric", "title": "T",
             "dataSource": {"type": "analytics", "query": "total_messages"},
             "position": {"x": 0, "y": 0, "width": 4, "height": 2}}
        ]}))).await;

    // widget_update requires widgetId; unsupported type -> 400.
    let (status, _, _) = app
        .request("POST", "/api/analytics/realtime/broadcast", Some(&admin),
            Some(json!({"dashboardId": "rt", "type": "widget_update"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/analytics/realtime/broadcast", Some(&admin),
            Some(json!({"dashboardId": "rt", "type": "telepathy"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/analytics/realtime/broadcast", Some(&admin),
            Some(json!({"dashboardId": "rt", "type": "config_change", "data": {}})))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Trigger single + whole-dashboard refresh.
    let (status, body, _) = app
        .request("POST", "/api/analytics/realtime/trigger-update/rt/w1", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, _) = app
        .request("POST", "/api/analytics/realtime/trigger-update/rt/ghost", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body, _) = app
        .request("POST", "/api/analytics/realtime/trigger-update/rt", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["updatedWidgets"].as_array().unwrap().len(), 1);

    // Status/cleanup are admin-only; health is open to authenticated callers.
    let (status, _, _) = app
        .request("GET", "/api/analytics/realtime/status", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request("POST", "/api/analytics/realtime/cleanup", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _) = app
        .request("GET", "/api/analytics/realtime/health", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "healthy");
}

// ---------------------------------------------------------------- security

#[tokio::test]
async fn security_dashboard_metrics_events_summary() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    // Public health.
    let (status, body, _) = app.request("GET", "/api/security/dashboard/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"]["module"], "security-dashboard");

    // Seed webhook + cors events.
    for (kind, severity) in [("invalid_signature", "high"), ("invalid_signature", "high"), ("payload_too_large", "medium")] {
        sqlx::query(
            "INSERT INTO webhook_security_events (id, event_type, severity, platform, source_ip, created_at)
             VALUES (?, ?, ?, 'line', '1.2.3.4', ?)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(kind)
        .bind(severity)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&app.state.db)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO cors_events (id, outcome, origin, method, path, timestamp)
         VALUES ('ce-1', 'rejected', 'https://evil.example.com', 'GET', '/api/x', ?)",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    // Admin-only.
    let (status, _, _) = app
        .request("GET", "/api/security/dashboard/metrics", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body, _) = app
        .request("GET", "/api/security/dashboard/metrics?timeRange=24h", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let m = &body["data"];
    assert_eq!(m["summary"]["totalEvents"], 4);
    assert_eq!(m["summary"]["bySeverity"]["high"], 2);
    assert_eq!(m["summary"]["topThreats"][0]["type"], "invalid_signature");
    assert_eq!(m["webhookSecurity"]["totalEvents"], 3);
    assert_eq!(m["corsMonitoring"]["rejected"], 1);
    assert_eq!(m["corsMonitoring"]["topRejectedOrigins"][0]["origin"], "https://evil.example.com");
    assert_eq!(m["alerts"]["count"], 2);

    // Recent events merged + limit validation.
    let (status, body, _) = app
        .request("GET", "/api/security/dashboard/events/recent?limit=2", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], 2);
    let (status, _, _) = app
        .request("GET", "/api/security/dashboard/events/recent?limit=0", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Summary condensed view.
    let (status, body, _) = app
        .request("GET", "/api/security/dashboard/summary", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["webhook"]["total"], 3);
    assert_eq!(body["data"]["webhook"]["topEventType"], "invalid_signature");
    assert_eq!(body["data"]["cors"]["rejected"], 1);
}
