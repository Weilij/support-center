//! Reports per CRD §6.2 (lines 4505-4695).

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

fn generate_body() -> serde_json::Value {
    json!({
        "type": "conversation_summary",
        "title": "Weekly summary",
        "format": "json",
        "timeRange": "last_7_days",
    })
}

#[tokio::test]
async fn health_and_info_are_public() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/reports/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["module"], "reports");
    let (status, body, _) = app.request("GET", "/api/reports/info", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["generatableTypes"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn generate_validates_and_completes_pipeline() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;

    // Validation failures.
    let cases = [
        (json!({"title": "t", "format": "json", "timeRange": "today"}), "missing type"),
        (json!({"type": "quantum", "title": "t", "format": "json", "timeRange": "today"}), "bad type"),
        (json!({"type": "conversation_summary", "format": "json", "timeRange": "today"}), "missing title"),
        (json!({"type": "conversation_summary", "title": "t", "format": "wav", "timeRange": "today"}), "bad format"),
        (json!({"type": "conversation_summary", "title": "t", "format": "json", "timeRange": "fortnight"}), "bad range"),
        (json!({"type": "conversation_summary", "title": "t", "format": "json", "timeRange": "custom"}), "missing custom dates"),
        (json!({"type": "conversation_summary", "title": "t", "format": "pdf", "timeRange": "today"}), "non-generatable format"),
        (json!({"type": "cost_analysis", "title": "t", "format": "json", "timeRange": "today"}), "catalog-only type"),
    ];
    for (body, label) in cases {
        let (status, _, _) = app.request("POST", "/api/reports", Some(&agent), Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
    }

    // Restricted types are admin-only (403, not 400).
    let (status, _, _) = app
        .request("POST", "/api/reports", Some(&agent),
            Some(json!({"type": "system_health", "title": "t", "format": "json", "timeRange": "today"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Successful generation completes the lifecycle synchronously.
    let (status, body, _) = app.request("POST", "/api/reports", Some(&agent), Some(generate_body())).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let report = &body["data"]["report"];
    assert_eq!(report["status"], "completed");
    assert!(report["fileSize"].as_i64().unwrap() > 0);
    assert!(report["downloadPath"].is_string());
    assert!(report["expiresAt"].is_string(), "30-day retention advertised");

    // Title sanitization strips markup.
    let (_, body, _) = app
        .request("POST", "/api/reports", Some(&agent),
            Some(json!({"type": "message_statistics", "title": "<script>x</script>Stats",
                        "format": "csv", "timeRange": "today"})))
        .await;
    assert_eq!(body["data"]["report"]["title"], "scriptx/scriptStats");
}

#[tokio::test]
async fn list_detail_download_delete_flow() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;
    let (_, body, _) = app.request("POST", "/api/reports", Some(&agent), Some(generate_body())).await;
    let id = body["data"]["report"]["id"].as_str().unwrap().to_string();

    // List with pagination + summary.
    let (status, body, _) = app.request("GET", "/api/reports", Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["summary"]["completed"], 1);
    assert_eq!(body["data"]["pagination"]["total"], 1);
    let (status, _, _) = app.request("GET", "/api/reports?status=brewing", Some(&agent), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Detail with download history scaffold.
    let (status, _, _) = app.request("GET", "/api/reports/not-a-uuid", Some(&agent), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body, _) = app.request("GET", &format!("/api/reports/{id}"), Some(&agent), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["downloadHistory"].is_array());

    // Download: creator OK; outsider denied; history recorded.
    app.seed_agent("outsider@test.dev", "pw123456", "agent").await;
    let (outsider, _, _) = app.login("outsider@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request("GET", &format!("/api/reports/{id}/download"), Some(&outsider), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri(format!("/api/reports/{id}/download"))
        .header("Authorization", format!("Bearer {agent}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers().get("content-disposition").unwrap().to_str().unwrap().contains("attachment"));
    let downloads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM report_downloads WHERE report_id = $1")
        .bind(&id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(downloads, 1);

    // Delete: only creator or admin; soft delete removes the file.
    let (status, _, _) = app
        .request("DELETE", &format!("/api/reports/{id}"), Some(&outsider), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request("DELETE", &format!("/api/reports/{id}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app.request("GET", &format!("/api/reports/{id}"), Some(&agent), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "soft-deleted reports vanish from reads");
}

#[tokio::test]
async fn stats_and_batch_are_admin_only() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;
    app.request("POST", "/api/reports", Some(&agent), Some(generate_body())).await;

    let (status, _, _) = app.request("GET", "/api/reports/stats", Some(&agent), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app.request("GET", "/api/reports/stats", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 1);
    assert_eq!(body["data"]["byStatus"]["completed"], 1);
    assert_eq!(body["data"]["byType"]["cost_analysis"], 0, "zero-initialized catalog keys");

    let (_, listing, _) = app.request("GET", "/api/reports", Some(&agent), None).await;
    let id = listing["data"]["reports"][0]["id"].as_str().unwrap().to_string();
    let (status, _, _) = app
        .request("POST", "/api/reports/batch", Some(&agent),
            Some(json!({"reportIds": [id], "action": "delete"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body, _) = app
        .request("POST", "/api/reports/batch", Some(&admin),
            Some(json!({"reportIds": [id, uuid::Uuid::new_v4().to_string()], "action": "delete"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["successCount"], 1);
    assert_eq!(body["data"]["failedCount"], 1, "missing item fails per-item");
    assert_eq!(body["data"]["success"], false, "overall true only when no item failed");

    let (status, _, _) = app
        .request("POST", "/api/reports/batch", Some(&admin),
            Some(json!({"reportIds": ["not-a-uuid"], "action": "delete"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn templates_and_preview() {
    let app = spawn_app().await;
    let (_, agent) = users(&app).await;

    let (status, body, _) = app
        .request("GET", "/api/reports/templates/conversation_summary", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["templates"].as_array().unwrap().len(), 2);
    let (_, body, _) = app
        .request("GET", "/api/reports/templates/cost_analysis", Some(&agent), None)
        .await;
    assert_eq!(body["data"]["templates"].as_array().unwrap().len(), 0, "no presets -> empty");
    let (status, _, _) = app
        .request("GET", "/api/reports/templates/quantum", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Preview works for catalog-only types too, persisting nothing.
    let (status, body, _) = app
        .request("POST", "/api/reports/preview", Some(&agent),
            Some(json!({"type": "conversation_summary", "timeRange": "last_7_days"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["totals"]["conversations"].as_i64().unwrap() >= 120);
    let persisted: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reports")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(persisted, 0, "preview persists nothing");
    let (status, _, _) = app
        .request("POST", "/api/reports/preview", Some(&agent),
            Some(json!({"type": "conversation_summary"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "timeRange required");
}

#[tokio::test]
async fn scheduled_reports_lifecycle_and_execution() {
    let app = spawn_app().await;
    let (admin, agent) = users(&app).await;

    // Validation.
    let cases = [
        (json!({"type": "conversation_summary", "format": "json",
                "schedule": {"frequency": "daily", "time": "08:00"}}), "missing name"),
        (json!({"name": "n", "type": "conversation_summary", "format": "json",
                "schedule": {"frequency": "hourly", "time": "08:00"}}), "bad frequency"),
        (json!({"name": "n", "type": "conversation_summary", "format": "json",
                "schedule": {"frequency": "daily", "time": "8am"}}), "bad time"),
        (json!({"name": "n", "type": "conversation_summary", "format": "json",
                "schedule": {"frequency": "weekly", "time": "08:00"}}), "weekly needs dayOfWeek"),
        (json!({"name": "n", "type": "conversation_summary", "format": "json",
                "schedule": {"frequency": "daily", "time": "08:00"},
                "recipients": [{"email": "not-an-email", "name": "X"}]}), "bad recipient"),
    ];
    for (body, label) in cases {
        let (status, _, _) = app
            .request("POST", "/api/reports/scheduled", Some(&agent), Some(body))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "case: {label}");
    }

    let (status, body, _) = app
        .request("POST", "/api/reports/scheduled", Some(&agent),
            Some(json!({"name": "Daily digest", "type": "conversation_summary", "format": "json",
                        "schedule": {"frequency": "daily", "time": "08:00"},
                        "recipients": [{"email": "ops@test.dev", "name": "Ops"}]})))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let sched_id = body["data"]["id"].as_str().unwrap().to_string();
    assert!(body["data"]["nextRunAt"].is_string());

    // Listing: creator sees own; admin sees all.
    let (_, body, _) = app.request("GET", "/api/reports/scheduled", Some(&agent), None).await;
    assert_eq!(body["data"]["count"], 1);
    let (_, body, _) = app.request("GET", "/api/reports/scheduled", Some(&admin), None).await;
    assert_eq!(body["data"]["count"], 1);

    // Only the creator may update/delete.
    let (status, _, _) = app
        .request("PUT", &format!("/api/reports/scheduled/{sched_id}"), Some(&admin),
            Some(json!({"name": "Hijack", "type": "conversation_summary", "format": "json",
                        "schedule": {"frequency": "daily", "time": "09:00"}})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request("PUT", &format!("/api/reports/scheduled/{sched_id}"), Some(&agent),
            Some(json!({"name": "Daily digest v2", "type": "conversation_summary", "format": "json",
                        "schedule": {"frequency": "daily", "time": "09:00"}})))
        .await;
    assert_eq!(status, StatusCode::OK);

    // Execution: make it due, run the pass, observe the generated report.
    sqlx::query("UPDATE scheduled_reports SET next_run_at = '2000-01-01T00:00:00Z' WHERE id = $1")
        .bind(&sched_id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let processed = mcss_backend::domain::reports::scheduler::run_due(&app.state).await;
    assert_eq!(processed, 1);
    let (last_status, run_count): (Option<String>, i64) = sqlx::query_as(
        "SELECT last_status, run_count FROM scheduled_reports WHERE id = $1",
    )
    .bind(&sched_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(last_status.as_deref(), Some("success"));
    assert_eq!(run_count, 1);
    let generated: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reports WHERE time_range = 'last_24_hours'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(generated, 1, "date-stamped report generated over last-24h window");
    let runs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM scheduled_report_runs WHERE schedule_id = $1 AND status = 'success'",
    )
    .bind(&sched_id)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(runs, 1);

    // Delete (creator).
    let (status, _, _) = app
        .request("DELETE", &format!("/api/reports/scheduled/{sched_id}"), Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body, _) = app.request("GET", "/api/reports/scheduled", Some(&agent), None).await;
    assert_eq!(body["data"]["count"], 0);
}
