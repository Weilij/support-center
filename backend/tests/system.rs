//! System Settings & Administration per CRD §6.6 (lines 5247-5487).

mod common;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use common::{spawn_app, TestApp};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

async fn users(app: &TestApp) -> (String, String) {
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("agent@test.dev", "pw123456").await;
    (admin, agent)
}

async fn webhook_sink() -> (String, Arc<Mutex<Vec<Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    async fn capture(State(seen): State<Arc<Mutex<Vec<Value>>>>, body: Bytes) -> Json<Value> {
        let parsed = serde_json::from_slice::<Value>(&body).unwrap_or_else(|_| json!({}));
        seen.lock().await.push(parsed);
        Json(json!({"ok": true}))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/alert", post(capture))
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/alert"), seen)
}

#[tokio::test]
async fn public_probes_and_descriptor() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/system/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "healthy");
    // M3: public liveness must not leak DB status or version.
    assert!(body.get("database").is_none() || body["database"].is_null());
    assert!(body.get("version").is_none() || body["version"].is_null());

    let (status, body, _) = app.request("GET", "/api/system/api", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["endpoints"]["auth"].is_string());

    // M3: public /api/health/health hides version/environment.
    let (_, body, _) = app.request("GET", "/api/health/health", None, None).await;
    assert!(body.get("version").is_none() || body["version"].is_null());
    assert!(body.get("environment").is_none() || body["environment"].is_null());

    // M3: public /api/health/status hides the component breakdown.
    let (_, body, _) = app.request("GET", "/api/health/status", None, None).await;
    assert!(body["data"].get("components").is_none() || body["data"]["components"].is_null());

    // M3: public data-optimization health hides config.
    let (_, body, _) = app.request("GET", "/api/data-optimization/health", None, None).await;
    let opt = if body.get("data").is_some() { &body["data"] } else { &body };
    assert!(opt.get("config").is_none() || opt["config"].is_null());

    for path in ["/api/health/health", "/api/health/status", "/api/health/ready",
                 "/api/health/live", "/api/reminders/health", "/api/data-optimization/health"] {
        let (status, _, _) = app.request("GET", path, None, None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn stats_status_and_metrics() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;
    let team = app.seed_team("S").await;
    let customer = app.seed_customer("line", "U-sys", "C", Some(team)).await;
    let conversation = app.seed_conversation(customer, Some(team), "active").await;
    app.seed_message(&conversation, "customer", "hello", None).await;

    let (status, body, _) = app.request("GET", "/api/system/stats", Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["totalMessages"], 1);
    assert_eq!(body["data"]["totalCustomers"], 1);
    assert_eq!(body["data"]["totalConversations"], 1);

    let (_, body, _) = app.request("GET", "/api/system/system/status", Some(&agent), None).await;
    assert_eq!(body["data"]["services"]["database"], "connected");
    assert_eq!(body["data"]["services"]["cache"], "available", "static availability");

    let (_, body, _) = app.request("GET", "/api/system/metrics", Some(&agent), None).await;
    assert_eq!(body["data"]["totalMessages"], 1);
    assert_eq!(body["data"]["errorRate"], 0, "boundary-fixed figure");

    let (_, body, _) = app.request("GET", "/api/system/messages/recall-stats", Some(&agent), None).await;
    assert_eq!(body["data"]["totalRecalls"], 0);

    // Message tree + replies + session stats.
    let (_, body, _) = app
        .request("GET", &format!("/api/system/conversations/{conversation}/message-tree"),
            Some(&agent), None)
        .await;
    assert_eq!(body["data"]["total"], 1);
    let (_, body, _) = app
        .request("GET", &format!("/api/system/conversations/{conversation}/sessions"),
            Some(&agent), None)
        .await;
    assert_eq!(body["data"]["analytics"]["totalSessions"], 0);
}

#[tokio::test]
async fn settings_read_merge_update_and_audit() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;

    // Defaults come back merged.
    let (status, body, _) = app.request("GET", "/api/system/settings", Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["general"]["language"], "zh-TW");
    assert_eq!(body["data"]["advanced"]["messageQueueSize"], 1000);

    // Validation.
    let cases = [
        (json!({}), "no groups -> no-op success path is message-only"),
    ];
    let _ = cases;
    let (status, _, _) = app
        .request("PUT", "/api/system/settings", Some(&agent),
            Some(json!({"general": {"language": "fr"}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("PUT", "/api/system/settings", Some(&agent),
            Some(json!({"advanced": {"cacheExpiry": 10}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Empty update is a successful no-op.
    let (status, body, _) = app
        .request("PUT", "/api/system/settings", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "No settings to update");

    // Real update persists + overlays + audits.
    let (status, _, _) = app
        .request("PUT", "/api/system/settings", Some(&agent),
            Some(json!({"general": {"systemName": "客服中心"},
                        "advanced": {"cacheExpiry": 7200}})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body, _) = app.request("GET", "/api/system/settings", Some(&agent), None).await;
    assert_eq!(body["data"]["general"]["systemName"], "客服中心");
    assert_eq!(body["data"]["advanced"]["cacheExpiry"], 7200);
    assert_eq!(body["data"]["general"]["language"], "zh-TW", "defaults preserved");
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'settings_update'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(audits, 1);

    // Secrets never disclosed: store a credential key directly, read back.
    sqlx::query("INSERT INTO system_settings (key, value, updated_at) VALUES ('settings.integrations.line.accessToken', '\"secret\"', '2026-01-01')")
        .execute(&app.state.db).await.unwrap();
    let (_, body, _) = app.request("GET", "/api/system/settings", Some(&agent), None).await;
    assert!(body["data"]["integrations"]["line"].get("accessToken").is_none(),
        "credential fields are stripped from reads");
}

#[tokio::test]
async fn integration_test_and_config_check() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    let (_, body, _) = app
        .request("POST", "/api/system/integrations/line/test", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(body["data"]["status"], "error", "incomplete credentials");
    let (_, body, _) = app
        .request("POST", "/api/system/integrations/line/test", Some(&agent),
            Some(json!({"channelId": "c", "channelSecret": "s", "accessToken": "t"})))
        .await;
    assert_eq!(body["data"]["status"], "success");
    let (_, body, _) = app
        .request("POST", "/api/system/integrations/telegram/test", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(body["data"]["status"], "error", "unsupported platform is a soft failure");

    // config-check: admin only; satisfied in development.
    let (status, _, _) = app.request("GET", "/api/system/config-check", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app.request("GET", "/api/system/config-check", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["satisfied"], true);

    // api-status dashboard.
    let (_, body, _) = app.request("GET", "/api/system/api-status", Some(&agent), None).await;
    assert_eq!(body["data"]["overall"], "operational");
}

#[tokio::test]
async fn unified_health_family() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;
    for path in ["/api/health/system", "/api/health/infrastructure", "/api/health/services",
                 "/api/health/stats"] {
        let (status, _, _) = app.request("GET", path, Some(&agent), None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    // M3: detail (components) preserved behind auth.
    let (_, body, _) = app.request("GET", "/api/health/system", Some(&agent), None).await;
    assert!(body["data"]["components"].is_array(), "authed detail retained");
    let (status, body, _) = app
        .request("GET", "/api/health/component/database", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "healthy");
    let (status, _, _) = app
        .request("GET", "/api/health/component/warp-core", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "unknown component");
    let (status, _, _) = app.request("POST", "/api/health/check/all", Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);

    // Prometheus-style text exposition.
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/health/metrics")
        .header("Authorization", format!("Bearer {agent}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert!(resp.headers().get("content-type").unwrap().to_str().unwrap().contains("text/plain"));
}

#[tokio::test]
async fn feedback_flow() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;
    let customer = app.seed_customer("line", "U-fb", "FB", None).await;
    let conversation = app.seed_conversation(customer, None, "active").await;

    let (status, _, _) = app
        .request("POST", "/api/feedback", Some(&agent), Some(json!({"rating": 5})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/feedback", Some(&agent),
            Some(json!({"conversationId": conversation, "customerId": customer, "rating": 9})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/feedback", Some(&agent),
            Some(json!({"conversationId": "ghost", "customerId": customer, "rating": 4})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for rating in [5, 4, 2] {
        let (status, _, _) = app
            .request("POST", "/api/feedback", Some(&agent),
                Some(json!({"conversationId": conversation, "customerId": customer, "rating": rating})))
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (_, body, _) = app.request("GET", "/api/feedback/stats", Some(&agent), None).await;
    assert_eq!(body["data"]["totalFeedback"], 3);
    assert!((body["data"]["satisfaction"].as_f64().unwrap() - 66.7).abs() < 0.1);
    assert_eq!(body["data"]["distribution"]["2"], 1);

    let (_, body, _) = app
        .request("GET", &format!("/api/feedback/conversation/{conversation}"), Some(&agent), None)
        .await;
    assert_eq!(body["data"]["count"], 3);
    assert_eq!(body["data"]["feedback"][0]["customerName"], "FB");

    let (_, body, _) = app.request("GET", "/api/feedback?pageSize=2", Some(&agent), None).await;
    assert_eq!(body["data"]["pagination"]["total"], 3);
    assert_eq!(body["data"]["feedback"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn alert_config_admin_gates_and_validation() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;
    let (webhook_url, webhook_seen) = webhook_sink().await;

    let (status, _, _) = app
        .request("POST", "/api/alert-config/channels/slack", Some(&agent),
            Some(json!({"webhookUrl": "https://hooks.slack.com/x"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request("POST", "/api/alert-config/channels/slack", Some(&admin),
            Some(json!({"webhookUrl": "https://example.com/x"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "must match the chat-webhook pattern");
    let (status, _, _) = app
        .request("POST", "/api/alert-config/channels/slack", Some(&admin),
            Some(json!({"webhookUrl": "https://hooks.slack.com/services/T/B/x"})))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, _) = app
        .request("POST", "/api/alert-config/channels/email", Some(&admin),
            Some(json!({"host": "smtp.test", "sender": "a@b.c", "password": "p", "recipients": []})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "recipients non-empty");
    let (status, body, _) = app
        .request("POST", "/api/alert-config/channels/email", Some(&admin),
            Some(json!({"host": "smtp.test", "sender": "a@b.c", "password": "p",
                        "recipients": ["ops@b.c", "sec@b.c"]})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["recipientCount"], 2);
    let (status, _, _) = app
        .request("POST", "/api/alert-config/channels/webhook", Some(&admin),
            Some(json!({"webhookUrl": webhook_url})))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (_, body, _) = app
        .request("GET", "/api/alert-config/channels/status", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["slack"]["configured"], true);
    assert_eq!(body["data"]["email"]["recipientCount"], 2);
    assert_eq!(body["data"]["webhook"]["configured"], true);

    let (status, _, _) = app
        .request("POST", "/api/alert-config/test-alert", Some(&admin),
            Some(json!({"level": "catastrophic"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app
        .request("POST", "/api/alert-config/test-alert", Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["level"], "warning", "default level");
    let received = webhook_seen.lock().await;
    assert_eq!(received.len(), 0, "warning defaults to console only");

    drop(received);
    let (status, body, _) = app
        .request("POST", "/api/alert-config/test-alert", Some(&admin),
            Some(json!({"level": "critical", "title": "API smoke"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["channelAttempts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["channel"] == "webhook" && a["success"] == true));
    let received = webhook_seen.lock().await;
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["title"], "API smoke");
}

#[tokio::test]
async fn data_optimization_and_kv_monitoring() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    let (status, _, _) = app.request("GET", "/api/data-optimization/config", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, body, _) = app.request("GET", "/api/data-optimization/config", Some(&admin), None).await;
    assert_eq!(body["data"]["cacheTtl"], 3600);

    let (status, _, _) = app
        .request("PUT", "/api/data-optimization/config", Some(&admin),
            Some(json!({"batchSize": 5})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "batch size bound 10-1000");
    let (status, body, _) = app
        .request("PUT", "/api/data-optimization/config", Some(&admin),
            Some(json!({"batchSize": 200})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["batchSize"], 200);

    let (status, _, _) = app
        .request("POST", "/api/data-optimization/test-cache", Some(&admin),
            Some(json!({"testSize": 5})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/data-optimization/test-batch", Some(&admin),
            Some(json!({"operationType": "teleport"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Index build + query round trip.
    let (status, _, _) = app
        .request("POST", "/api/data-optimization/indexes", Some(&admin),
            Some(json!({"name": "conv", "field": "status"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "sampleData required");
    let (status, _, _) = app
        .request("POST", "/api/data-optimization/indexes", Some(&admin),
            Some(json!({"name": "conv", "field": "status",
                        "sampleData": [{"status": "open"}, {"status": "closed"}]})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request("GET", "/api/data-optimization/indexes/conv/status", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "value required");
    let (_, body, _) = app
        .request("GET", "/api/data-optimization/indexes/conv/status?value=open", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["count"], 1);

    // Baseline: second init warns.
    let (_, body, _) = app
        .request("POST", "/api/data-optimization/initialize-baseline", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["initialized"], true);
    let (_, body, _) = app
        .request("POST", "/api/data-optimization/initialize-baseline", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["initialized"], false);

    // KV monitoring admin-gated.
    let (status, _, _) = app.request("GET", "/api/monitoring/kv/health", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    for path in ["/api/monitoring/kv/activity-cache", "/api/monitoring/kv/request-frequency",
                 "/api/monitoring/kv/savings", "/api/monitoring/kv/health"] {
        let (status, _, _) = app.request("GET", path, Some(&admin), None).await;
        assert_eq!(status, StatusCode::OK, "{path}");
    }
    let (status, _, _) = app.request("POST", "/api/monitoring/kv/reset", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn user_experience_and_migrations() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    let (status, _, _) = app
        .request("POST", "/api/user-experience/metrics", Some(&agent), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/user-experience/metrics", Some(&agent),
            Some(json!({"sessionId": "s1", "timestamp": 1})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request("GET", "/api/user-experience/survey/invitation?sessionId=s1", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app
        .request("POST", "/api/user-experience/survey", Some(&agent),
            Some(json!({"sessionId": "s1", "overallSatisfaction": 5, "scores": [5, 4, 9, 3, 2]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "sub-scores must be 1-5");
    let (status, _, _) = app.request("GET", "/api/user-experience/report", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request("GET", "/api/user-experience/report?timeRange=999", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, body, _) = app
        .request("GET", "/api/user-experience/ab-tests/exp-1/assignment", Some(&agent), None)
        .await;
    assert!(["A", "B"].contains(&body["data"]["variant"].as_str().unwrap()));

    // Migration: legacy filename backfill, dry-run default, real run mutates.
    mcss_backend::domain::files::store::put_object(
        &app.state.config.upload_dir, "legacy/one", b"data",
    )
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO attachments (id, file_name, content_type, storage_key, created_at)
         VALUES ('a-legacy', 'photo', 'image/png', 'legacy/one', '2026-01-01')",
    )
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, _, _) = app
        .request("POST", "/api/admin/migrations/backfill-legacy-filenames", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app
        .request("POST", "/api/admin/migrations/backfill-legacy-filenames", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["stats"]["dryRun"], true);
    assert_eq!(body["data"]["stats"]["fixed"], 1);
    let name: String = sqlx::query_scalar("SELECT file_name FROM attachments WHERE id = 'a-legacy'")
        .fetch_one(&app.state.db).await.unwrap();
    assert_eq!(name, "photo", "dry-run mutates nothing");

    let (status, body, _) = app
        .request("POST", "/api/admin/migrations/backfill-legacy-filenames?dryRun=false",
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["stats"]["fixed"], 1);
    let name: String = sqlx::query_scalar("SELECT file_name FROM attachments WHERE id = 'a-legacy'")
        .fetch_one(&app.state.db).await.unwrap();
    assert_eq!(name, "photo.png", "real run appends the derived extension");

    let (status, _, _) = app
        .request("POST", "/api/admin/migrations/backfill-legacy-filenames?limit=0",
            Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
