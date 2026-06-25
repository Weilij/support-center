//! LIFF integration per CRD §4.3 (lines 2862-2994).

mod common;

use axum::extract::Form;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use common::{spawn_app, spawn_app_custom};
use serde_json::json;
use std::collections::HashMap;

async fn line_verify_server() -> String {
    async fn verify(Form(form): Form<HashMap<String, String>>) -> Json<serde_json::Value> {
        let sub = form
            .get("id_token")
            .and_then(|token| token.strip_prefix("token:"))
            .unwrap_or("");
        Json(json!({ "sub": sub }))
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/verify", post(verify)))
            .await
            .unwrap();
    });
    format!("http://{addr}/verify")
}

#[tokio::test]
async fn health_and_config_are_public() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/liff/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "healthy");
    assert_eq!(body["module"], "liff");

    let (status, body, _) = app.request("GET", "/api/liff/config", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["liffId"], "test-liff-id");
    assert_eq!(body["data"]["lineBotId"], "@testbot");
    assert_eq!(body["data"]["lineOaId"], "testbot");
    assert_eq!(body["data"]["autoCloseDelay"], 2000);
}

#[tokio::test]
async fn config_requires_liff_id() {
    let app = spawn_app_custom(|c| c.liff_id = None).await;
    let (status, body, _) = app.request("GET", "/api/liff/config", None, None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn team_info_validates_and_returns_public_fields() {
    let app = spawn_app().await;
    let team = app.seed_team("LIFF Team").await;

    let (status, _, _) = app.request("GET", "/api/liff/teams/abc", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app.request("GET", "/api/liff/teams/999", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, _) = app
        .request("GET", &format!("/api/liff/teams/{team}"), None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["id"], team);
    assert_eq!(body["data"]["name"], "LIFF Team");
}

#[tokio::test]
async fn assign_team_is_idempotent_per_user_team_pair() {
    let verify_url = line_verify_server().await;
    let app = spawn_app_custom(|c| c.line_id_token_verify_url = verify_url).await;
    let team = app.seed_team("Routing").await;
    // Seed the LIFF code record so the scan counter applies.
    sqlx::query(
        "INSERT INTO team_liff_links (id, team_id, url, is_active, created_at)
         VALUES ('liff-1', $1, 'https://liff.line.me/x', 1, $2)",
    )
    .bind(team)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, _, _) = app
        .request("POST", "/api/liff/assign-team", None, Some(json!({"teamId": team})))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "LINE ID token required");
    let (status, _, _) = app
        .request("POST", "/api/liff/assign-team", None, Some(json!({"lineIdToken": "token:U-x"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "teamId required");
    let (status, _, _) = app
        .request("POST", "/api/liff/assign-team", None,
            Some(json!({"lineIdToken": "token:U-x", "lineUserId": "U-spoof", "teamId": 999})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, _) = app
        .request("POST", "/api/liff/assign-team", None,
            Some(json!({"lineIdToken": "token:U-x", "lineUserId": "U-spoof", "teamId": team, "displayName": "Scanner"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first_id = body["data"]["assignmentId"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["teamName"], "Routing");

    // Repeat: same record, no duplicate, counter not re-incremented.
    let (status, body, _) = app
        .request("POST", "/api/liff/assign-team", None,
            Some(json!({"lineIdToken": "token:U-x", "teamId": team})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["assignmentId"], first_id.as_str());

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customer_team_assignments WHERE platform_user_id = 'U-x'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let scans: i64 = sqlx::query_scalar("SELECT scan_count FROM team_liff_links WHERE team_id = $1")
        .bind(team)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(scans, 1, "scan counter incremented exactly once");
    let source: String = sqlx::query_scalar(
        "SELECT source FROM customer_team_assignments WHERE platform_user_id = 'U-x'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(source, "scan");
}

#[tokio::test]
async fn welcome_validates_and_reconciles_conversations() {
    // The LIFF welcome handler validates a configured LINE channel token before
    // reconciling (returns Internal when absent). Opt in to a present token; no
    // real network call is made (the push is a TODO stub).
    let verify_url = line_verify_server().await;
    let app = spawn_app_custom(|c| {
        c.line_channel_access_token = Some("test-push-token".into());
        c.line_id_token_verify_url = verify_url;
    }).await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;

    let (status, _, _) = app
        .request("POST", "/api/liff/welcome", None, Some(json!({"teamId": team_a})))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = app
        .request("POST", "/api/liff/welcome", None, Some(json!({"lineIdToken": "token:U-w"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/liff/welcome", None,
            Some(json!({"lineIdToken": "token:U-w", "lineUserId": "U-spoof", "teamId": 999})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // No customer record: reconciliation skipped, welcome still succeeds.
    let (status, body, _) = app
        .request("POST", "/api/liff/welcome", None,
            Some(json!({"lineIdToken": "token:U-ghost", "teamId": team_a})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["message"].is_string());

    // Existing friend with an open conversation on another team: reassigned.
    let customer = app.seed_customer("line", "U-w", "Friend", None).await;
    let conversation = app.seed_conversation(customer, Some(team_a), "active").await;
    let (status, _, _) = app
        .request("POST", "/api/liff/welcome", None,
            Some(json!({"lineIdToken": "token:U-w", "lineUserId": "U-spoof", "teamId": team_b})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let assigned: Option<i64> = sqlx::query_scalar("SELECT team_id FROM conversations WHERE id = $1")
        .bind(&conversation)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(assigned, Some(team_b), "open conversation reassigned to the target team");

    // Closed conversations are never reassigned: a new one is created instead.
    sqlx::query("UPDATE conversations SET status = 'closed' WHERE id = $1")
        .bind(&conversation)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request("POST", "/api/liff/welcome", None,
            Some(json!({"lineIdToken": "token:U-w", "teamId": team_a})))
        .await;
    assert_eq!(status, StatusCode::OK);
    let open_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM conversations WHERE customer_id = $1 AND status = 'active' AND team_id = $2",
    )
    .bind(customer)
    .bind(team_a)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(open_count, 1, "new active conversation created for the target team");
    let closed_team: Option<i64> =
        sqlx::query_scalar("SELECT team_id FROM conversations WHERE id = $1")
            .bind(&conversation)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(closed_team, Some(team_b), "closed conversation untouched");
}

#[tokio::test]
async fn welcome_requires_push_credential() {
    let verify_url = line_verify_server().await;
    let app = spawn_app_custom(|c| {
        c.line_channel_access_token = None;
        c.line_id_token_verify_url = verify_url;
    }).await;
    let team = app.seed_team("A").await;
    let (status, _, _) = app
        .request("POST", "/api/liff/welcome", None,
            Some(json!({"lineIdToken": "token:U-w", "teamId": team})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn join_page_renders_html_with_escaping() {
    let app = spawn_app().await;
    let team = app.seed_team("<script>alert(1)</script>").await;

    let (status, _, headers) = app.request("GET", "/join", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("content-type").unwrap().to_str().unwrap().contains("text/html"));

    use tower::ServiceExt;
    use http_body_util::BodyExt;
    let req = axum::http::Request::builder()
        .uri(format!("/join?team={team}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let html = String::from_utf8_lossy(
        &resp.into_body().collect().await.unwrap().to_bytes(),
    )
    .to_string();
    assert!(html.contains("&lt;script&gt;"), "team name HTML-escaped: {html}");
    assert!(!html.contains("<script>alert"), "no raw injection");

    let req = axum::http::Request::builder()
        .uri("/join?team=99999")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let html =
        String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).to_string();
    assert!(html.contains("失效"), "expired-link page: {html}");
}

#[tokio::test]
async fn admin_batch_generate_and_coverage() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("agent@test.dev", "pw123456").await;
    let t1 = app.seed_team("T1").await;
    let _t2 = app.seed_team("T2").await;

    // Admin-only.
    let (status, _, _) = app
        .request("POST", "/api/admin/liff-qr/batch-generate", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = app
        .request("GET", "/api/admin/liff-qr/status", None, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Coverage before: none covered.
    let (_, body, _) = app.request("GET", "/api/admin/liff-qr/status", Some(&admin), None).await;
    assert_eq!(body["data"]["totalTeams"], 2);
    assert_eq!(body["data"]["teamsWithLiffQR"], 0);

    // Generate for all missing teams.
    let (status, body, _) = app
        .request("POST", "/api/admin/liff-qr/batch-generate", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["total"], 2);
    assert_eq!(body["data"]["success"], 2);
    assert_eq!(body["data"]["failed"], 0);

    // Coverage after: 100%.
    let (_, body, _) = app.request("GET", "/api/admin/liff-qr/status", Some(&admin), None).await;
    assert_eq!(body["data"]["teamsWithLiffQR"], 2);
    assert_eq!(body["data"]["coverage"], "100.00%");
    assert_eq!(body["data"]["teams"][0]["hasLiffQR"], true);

    // Early exit when everything is covered.
    let (status, body, _) = app
        .request("POST", "/api/admin/liff-qr/batch-generate", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 0);
    assert!(body["message"].is_string());
    let _ = t1;
}
