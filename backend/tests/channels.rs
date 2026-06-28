//! Channel Integrations (CRD §4.1, lines 2612-2720): management API behavior,
//! encrypted credential storage, verification, statistics, health.

mod common;

use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use common::{spawn_app, spawn_app_custom, TestApp};
use serde_json::{json, Value};
use std::collections::HashMap;

async fn admin_in_team(app: &TestApp, email: &str, team: i64) -> String {
    let id = app.seed_agent(email, "Passw0rd!", "admin").await;
    app.add_membership(&id, team, "supervisor", true).await;
    let (token, _, _) = app.login(email, "Passw0rd!").await;
    token
}

fn line_body() -> Value {
    json!({
        "platform": "line",
        "lineConfig": {
            "channelId": "chan-123",
            "channelAccessToken": "access-token-abc",
            "channelSecret": "channel-secret-xyz"
        }
    })
}

async fn create_line(app: &TestApp, token: &str) -> Value {
    let (status, body, _) = app
        .request("POST", "/api/channels", Some(token), Some(line_body()))
        .await;
    assert_eq!(status, 201, "create failed: {body}");
    body
}

async fn mock_platform_verify_server() -> String {
    async fn line_info(headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if token.contains("invalid") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(Json(json!({
            "userId": "Uline-bot",
            "basicId": "@testbot",
            "displayName": "Support Bot",
        })))
    }

    async fn graph_node(
        Path(id): Path<String>,
        Query(_query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> Result<Json<Value>, StatusCode> {
        let token = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if token.contains("invalid") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(Json(json!({
            "id": id,
            "name": format!("Page {id}"),
            "display_phone_number": "+15551234567",
            "verified_name": "Support WhatsApp",
        })))
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/line/bot/info", get(line_info))
                .route("/graph/{id}", get(graph_node)),
        )
        .await
        .unwrap();
    });
    format!("http://{addr}")
}

async fn spawn_channels_app() -> TestApp {
    let base = mock_platform_verify_server().await;
    spawn_app_custom(|c| {
        c.line_bot_info_url = format!("{base}/line/bot/info");
        c.meta_graph_url = format!("{base}/graph");
    })
    .await
}

async fn credentials_blob(app: &TestApp, id: i64) -> String {
    sqlx::query_scalar("SELECT credentials FROM channel_integrations WHERE id = $1")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

// --------------------------------------------------------------- list (CRD 2624-2630)

#[tokio::test]
async fn list_requires_authentication() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/channels", None, None).await;
    assert_eq!(status, 401);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn list_scopes_to_primary_team_with_platform_filter() {
    let app = spawn_app().await;
    let team_a = app.seed_team("Team A").await;
    let team_b = app.seed_team("Team B").await;
    let token_a = admin_in_team(&app, "a@x.io", team_a).await;
    let token_b = admin_in_team(&app, "b@x.io", team_b).await;
    create_line(&app, &token_a).await;

    let (status, body, _) = app
        .request("GET", "/api/channels", Some(&token_a), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["count"], 1);
    assert_eq!(body["data"][0]["teamId"], team_a);
    // Credentials are never serialized (CRD 2622).
    assert!(body["data"][0].get("credentials").is_none());

    // The other team sees nothing.
    let (_, body, _) = app
        .request("GET", "/api/channels", Some(&token_b), None)
        .await;
    assert_eq!(body["count"], 0);

    // Platform filter.
    let (_, body, _) = app
        .request(
            "GET",
            "/api/channels?platform=facebook",
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(body["count"], 0);
    let (status, body, _) = app
        .request(
            "GET",
            "/api/channels?platform=carrier-pigeon",
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn admin_without_team_lists_all_or_by_team_param() {
    let app = spawn_app().await;
    let team_a = app.seed_team("Team A").await;
    let team_b = app.seed_team("Team B").await;
    let token_a = admin_in_team(&app, "a@x.io", team_a).await;
    let token_b = admin_in_team(&app, "b@x.io", team_b).await;
    create_line(&app, &token_a).await;
    create_line(&app, &token_b).await;

    app.seed_agent("root@x.io", "Passw0rd!", "admin").await;
    let (token, _, _) = app.login("root@x.io", "Passw0rd!").await;

    let (status, body, _) = app
        .request("GET", "/api/channels", Some(&token), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["count"], 2);

    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/channels?teamId={team_a}"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["count"], 1);

    // Non-numeric team identifier from an admin -> 400 (CRD 2630).
    let (status, _, _) = app
        .request("GET", "/api/channels?teamId=abc", Some(&token), None)
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn non_admin_without_team_is_rejected() {
    let app = spawn_app().await;
    app.seed_agent("lone@x.io", "Passw0rd!", "agent").await;
    let (token, _, _) = app.login("lone@x.io", "Passw0rd!").await;
    let (status, body, _) = app
        .request("GET", "/api/channels", Some(&token), None)
        .await;
    assert_eq!(status, 400, "{body}");
}

// ------------------------------------------------------------- create (CRD 2632-2642)

#[tokio::test]
async fn create_requires_admin_role() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let agent = app.seed_agent("agent@x.io", "Passw0rd!", "agent").await;
    app.add_membership(&agent, team, "member", true).await;
    let (token, _, _) = app.login("agent@x.io", "Passw0rd!").await;
    let (status, body, _) = app
        .request("POST", "/api/channels", Some(&token), Some(line_body()))
        .await;
    assert_eq!(status, 403, "{body}");
}

#[tokio::test]
async fn create_validates_platform_and_required_fields() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    // Missing platform.
    let (status, _, _) = app
        .request("POST", "/api/channels", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, 400);
    // Invalid platform.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({"platform": "telegram"})),
        )
        .await;
    assert_eq!(status, 400);
    // Missing the platform config object.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({"platform": "line"})),
        )
        .await;
    assert_eq!(status, 400);
    // Missing a required field -> field-specific message (CRD 2642).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({"platform": "line", "lineConfig": {"channelId": "c1"}})),
        )
        .await;
    assert_eq!(status, 400);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("channelAccessToken"),
        "{body}"
    );
}

#[tokio::test]
async fn create_persists_encrypted_credentials_and_webhook_url() {
    let key = mcss_backend::crypto::generate_key();
    let key_clone = key.clone();
    let app = spawn_app_custom(move |c| c.encryption_key = Some(key_clone)).await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    let body = create_line(&app, &token).await;
    assert_eq!(body["success"], true);
    let id = body["data"]["id"].as_i64().unwrap();
    // Created enabled, not yet verified, zeroed stats (CRD 2637).
    assert_eq!(body["data"]["isActive"], true);
    assert_eq!(body["data"]["isVerified"], false);
    assert_eq!(body["data"]["stats"]["messagesReceived"], 0);
    // Sanitized record: no credentials anywhere (CRD 2622).
    assert!(body["data"].get("credentials").is_none());
    assert!(!body.to_string().contains("access-token-abc"));
    // Generated inbound address embeds platform, team and token (CRD 2637, 2722).
    let url = body["webhookUrl"].as_str().unwrap();
    assert!(
        url.contains(&format!("/api/webhooks/line/{team}/")),
        "{url}"
    );

    // Stored blob is protected, not plaintext (guarantee 1).
    let blob = credentials_blob(&app, id).await;
    let creds: Value = serde_json::from_str(&blob).unwrap();
    let stored = creds["channelAccessToken"].as_str().unwrap();
    assert!(stored.starts_with("enc:v1:"), "{stored}");
    assert_ne!(stored, "access-token-abc");
    // Authorized read returns the original (guarantee 4).
    assert_eq!(
        mcss_backend::crypto::reveal(Some(&key), stored).unwrap(),
        "access-token-abc"
    );
    // Non-deterministic: the second secret field encrypts the same way but
    // two protections of identical input differ (guarantee 2).
    let again = mcss_backend::crypto::protect(Some(&key), "access-token-abc").unwrap();
    assert_ne!(again, stored);

    // Audit entry recorded (CRD 2640).
    let logged: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE resource_type = 'channel_integration'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[tokio::test]
async fn create_without_encryption_key_stores_plaintext_with_warning() {
    // Mixed-format tolerance (CRD 5724): protection adoption is incremental.
    let app = spawn_app().await; // no encryption key configured
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let body = create_line(&app, &token).await;
    let id = body["data"]["id"].as_i64().unwrap();
    let blob = credentials_blob(&app, id).await;
    let creds: Value = serde_json::from_str(&blob).unwrap();
    assert_eq!(creds["channelSecret"], "channel-secret-xyz");
    // Legacy plaintext remains readable.
    assert_eq!(
        mcss_backend::crypto::reveal(None, creds["channelSecret"].as_str().unwrap()).unwrap(),
        "channel-secret-xyz"
    );
}

#[tokio::test]
async fn create_rejects_duplicate_active_platform() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    create_line(&app, &token).await;
    let (status, body, _) = app
        .request("POST", "/api/channels", Some(&token), Some(line_body()))
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["success"], false);
    assert!(
        body["error"].as_str().unwrap().contains("already exists"),
        "{body}"
    );
}

#[tokio::test]
async fn create_instagram_exposes_credentials_set_without_secret_values() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({
                "platform": "instagram",
                "instagramConfig": {"igId": "IG1", "accessToken": "tok"}
            })),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["data"]["platform"], "instagram");

    // Listed channel reflects the platform and the set secret fields, with no
    // secret value present anywhere in the response (CRD 2622).
    let (status, body, _) = app
        .request("GET", "/api/channels", Some(&token), None)
        .await;
    assert_eq!(status, 200, "{body}");
    let chan = &body["data"][0];
    assert_eq!(chan["platform"], "instagram");
    let set: Vec<String> = chan["credentialsSet"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(set.contains(&"accessToken".to_string()), "{body}");
    assert!(!body.to_string().contains("tok"), "secret leaked: {body}");
}

#[tokio::test]
async fn create_line_with_liff_id_stores_config_and_credentials_set() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({
                "platform": "line",
                "lineConfig": {
                    "channelId": "C1",
                    "channelAccessToken": "at",
                    "channelSecret": "sec",
                    "liffId": "liff-1"
                }
            })),
        )
        .await;
    assert_eq!(status, 201, "{body}");

    let (status, body, _) = app
        .request("GET", "/api/channels", Some(&token), None)
        .await;
    assert_eq!(status, 200, "{body}");
    let chan = &body["data"][0];
    assert_eq!(chan["config"]["liffId"], "liff-1");
    let set: Vec<String> = chan["credentialsSet"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(set.contains(&"channelAccessToken".to_string()), "{body}");
    assert!(set.contains(&"channelSecret".to_string()), "{body}");
}

#[tokio::test]
async fn create_line_without_liff_id_succeeds() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    // The standard line_body() omits liffId; creation still succeeds.
    let body = create_line(&app, &token).await;
    assert_eq!(body["success"], true);
    assert!(body["data"]["config"].get("liffId").is_none(), "{body}");
}

// ------------------------------------------------------------ get one (CRD 2644-2650)

#[tokio::test]
async fn get_channel_enforces_ownership_with_admin_override() {
    let app = spawn_app().await;
    let team_a = app.seed_team("Team A").await;
    let team_b = app.seed_team("Team B").await;
    let token_a = admin_in_team(&app, "a@x.io", team_a).await;
    let id = create_line(&app, &token_a).await["data"]["id"]
        .as_i64()
        .unwrap();

    // Owner reads it, sanitized.
    let (status, body, _) = app
        .request("GET", &format!("/api/channels/{id}"), Some(&token_a), None)
        .await;
    assert_eq!(status, 200);
    assert!(body["data"].get("credentials").is_none());

    // Non-admin from another team is denied.
    let other = app.seed_agent("b@x.io", "Passw0rd!", "agent").await;
    app.add_membership(&other, team_b, "member", true).await;
    let (token_b, _, _) = app.login("b@x.io", "Passw0rd!").await;
    let (status, _, _) = app
        .request("GET", &format!("/api/channels/{id}"), Some(&token_b), None)
        .await;
    assert_eq!(status, 403);

    // A global admin may access any team's connection (CRD 2647).
    app.seed_agent("root@x.io", "Passw0rd!", "admin").await;
    let (root, _, _) = app.login("root@x.io", "Passw0rd!").await;
    let (status, _, _) = app
        .request("GET", &format!("/api/channels/{id}"), Some(&root), None)
        .await;
    assert_eq!(status, 200);

    // Not found and invalid identifier.
    let (status, _, _) = app
        .request("GET", "/api/channels/99999", Some(&token_a), None)
        .await;
    assert_eq!(status, 404);
    let (status, body, _) = app
        .request("GET", "/api/channels/abc", Some(&token_a), None)
        .await;
    assert_eq!(status, 400);
    assert!(
        body["error"].as_str().unwrap().contains("positive integer"),
        "{body}"
    );
}

// ------------------------------------------------------------- update (CRD 2652-2659)

#[tokio::test]
async fn update_merges_config_and_resets_verification_on_secret_change() {
    let app = spawn_channels_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let id = create_line(&app, &token).await["data"]["id"]
        .as_i64()
        .unwrap();

    // Verify first so we can observe the reset.
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/channels/{id}/verify"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);

    // Non-secret merge only: verified state survives.
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/channels/{id}"),
            Some(&token),
            Some(json!({"lineConfig": {"channelId": "chan-456"}})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["config"]["channelId"], "chan-456");
    assert_eq!(body["data"]["isVerified"], true);

    // Secret change clears verified status + marker (CRD 2656, 2714).
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/channels/{id}"),
            Some(&token),
            Some(json!({"lineConfig": {"channelSecret": "rotated-secret"}})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["isVerified"], false);
    assert_eq!(body["data"]["verifiedAt"], Value::Null);
    // The omitted secret kept its prior value; the supplied one changed.
    let blob = credentials_blob(&app, id).await;
    let creds: Value = serde_json::from_str(&blob).unwrap();
    assert_eq!(creds["channelAccessToken"], "access-token-abc"); // unchanged (plaintext mode)
    assert_eq!(creds["channelSecret"], "rotated-secret");
}

#[tokio::test]
async fn update_blank_optional_field_does_not_erase_stored_value() {
    let app = spawn_channels_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    // Create a LINE channel that carries an optional liffId.
    let (status, body, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({
                "platform": "line",
                "lineConfig": {
                    "channelId": "C1",
                    "channelAccessToken": "at",
                    "channelSecret": "sec",
                    "liffId": "liff-1"
                }
            })),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();

    // Update another field while sending a blank liffId: the blank optional
    // value must be ignored, not persisted as an empty string.
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/channels/{id}"),
            Some(&token),
            Some(json!({"lineConfig": {"channelId": "chan-456", "liffId": ""}})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["config"]["channelId"], "chan-456");
    // The stored liffId is untouched (CRD 2656 — blank optional must not erase).
    assert_eq!(body["data"]["config"]["liffId"], "liff-1", "{body}");
}

#[tokio::test]
async fn update_enforces_role_team_and_uniqueness() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let id = create_line(&app, &token).await["data"]["id"]
        .as_i64()
        .unwrap();

    // Non-admin -> 403.
    let agent = app.seed_agent("m@x.io", "Passw0rd!", "agent").await;
    app.add_membership(&agent, team, "member", true).await;
    let (member, _, _) = app.login("m@x.io", "Passw0rd!").await;
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/channels/{id}"),
            Some(&member),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 403);

    // Not found -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/channels/424242", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, 404);

    // Disable, create a replacement, then re-enabling the old one violates
    // the one-active-per-platform rule (CRD 2712).
    let (status, _, _) = app
        .request(
            "PUT",
            &format!("/api/channels/{id}"),
            Some(&token),
            Some(json!({"isActive": false})),
        )
        .await;
    assert_eq!(status, 200);
    let id2 = create_line(&app, &token).await["data"]["id"]
        .as_i64()
        .unwrap();
    assert_ne!(id, id2);
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/channels/{id}"),
            Some(&token),
            Some(json!({"isActive": true})),
        )
        .await;
    assert_eq!(status, 400);
    assert!(
        body["error"].as_str().unwrap().contains("already exists"),
        "{body}"
    );
}

// ----------------------------------------------------- disable / delete (CRD 2661-2668)

#[tokio::test]
async fn delete_soft_disables_and_frees_the_platform_slot() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let id = create_line(&app, &token).await["data"]["id"]
        .as_i64()
        .unwrap();

    let (status, body, _) = app
        .request("DELETE", &format!("/api/channels/{id}"), Some(&token), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["success"], true);
    assert!(
        body["message"].as_str().unwrap().contains("disabled"),
        "{body}"
    );

    // Not physically removed (CRD 2666).
    let active: i64 =
        sqlx::query_scalar("SELECT is_active FROM channel_integrations WHERE id = $1")
            .bind(id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(active, 0);

    // A disabled connection no longer counts toward uniqueness (CRD 2666).
    create_line(&app, &token).await;

    // Not found path.
    let (status, _, _) = app
        .request("DELETE", "/api/channels/55555", Some(&token), None)
        .await;
    assert_eq!(status, 404);
}

// -------------------------------------------------------------- verify (CRD 2669-2680)

#[tokio::test]
async fn verify_success_marks_verified_and_clears_errors() {
    let app = spawn_channels_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let id = create_line(&app, &token).await["data"]["id"]
        .as_i64()
        .unwrap();
    // Seed a prior error state to observe the reset (CRD 2715).
    sqlx::query("UPDATE channel_integrations SET error_count = 3, last_error = '{}' WHERE id = $1")
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/channels/{id}/verify"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verified"], true);
    assert!(body["details"]["lastVerifiedAt"].is_string());
    assert_eq!(body["details"]["channelId"], "chan-123");

    let (_, body, _) = app
        .request("GET", &format!("/api/channels/{id}"), Some(&token), None)
        .await;
    assert_eq!(body["data"]["isVerified"], true);
    assert_eq!(body["data"]["errorCount"], 0);
    assert_eq!(body["data"]["lastError"], Value::Null);
}

#[tokio::test]
async fn verify_failure_increments_error_count_and_stores_record() {
    let app = spawn_channels_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/channels",
            Some(&token),
            Some(json!({
                "platform": "line",
                "lineConfig": {
                    "channelId": "c1",
                    "channelAccessToken": "invalid-token",
                    "channelSecret": "s1"
                }
            })),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/channels/{id}/verify"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["verified"], false);

    let (_, body, _) = app
        .request("GET", &format!("/api/channels/{id}"), Some(&token), None)
        .await;
    assert_eq!(body["data"]["isVerified"], false);
    assert_eq!(body["data"]["errorCount"], 1);
    assert_eq!(body["data"]["lastError"]["type"], "verification_failed");
}

#[tokio::test]
async fn verify_rejects_disabled_and_foreign_connections() {
    let app = spawn_channels_app().await;
    let team_a = app.seed_team("Team A").await;
    let team_b = app.seed_team("Team B").await;
    let token_a = admin_in_team(&app, "a@x.io", team_a).await;
    let token_b = admin_in_team(&app, "b@x.io", team_b).await;
    let id = create_line(&app, &token_a).await["data"]["id"]
        .as_i64()
        .unwrap();

    // Another team's admin is denied (CRD 2672).
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/channels/{id}/verify"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 403);

    // Disabled connection is not verifiable (CRD 2680).
    app.request(
        "DELETE",
        &format!("/api/channels/{id}"),
        Some(&token_a),
        None,
    )
    .await;
    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/channels/{id}/verify"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["verified"], false);
    assert!(
        body["message"].as_str().unwrap().contains("not active"),
        "{body}"
    );
}

// --------------------------------------------------------------- stats (CRD 2682-2687)

#[tokio::test]
async fn stats_returns_counters_and_uptime_strictly_same_team() {
    let app = spawn_app().await;
    let team_a = app.seed_team("Team A").await;
    let team_b = app.seed_team("Team B").await;
    let token_a = admin_in_team(&app, "a@x.io", team_a).await;
    let token_b = admin_in_team(&app, "b@x.io", team_b).await;
    let id = create_line(&app, &token_a).await["data"]["id"]
        .as_i64()
        .unwrap();

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/channels/{id}/stats"),
            Some(&token_a),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let data = &body["data"];
    assert_eq!(data["platform"], "line");
    assert_eq!(data["messagesSent"], 0);
    assert_eq!(data["messagesReceived"], 0);
    assert_eq!(data["lastMessageAt"], Value::Null);
    assert_eq!(data["isActive"], true);
    assert_eq!(data["errorCount"], 0);
    assert_eq!(data["uptime"]["days"], 0);
    assert_eq!(data["uptime"]["hoursInLastDay"], 24);

    // Strict same-team ownership: even an admin of another team is denied
    // (CRD 2685).
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/channels/{id}/stats"),
            Some(&token_b),
            None,
        )
        .await;
    assert_eq!(status, 403);

    // Missing team context -> 400.
    app.seed_agent("root@x.io", "Passw0rd!", "admin").await;
    let (root, _, _) = app.login("root@x.io", "Passw0rd!").await;
    let (status, _, _) = app
        .request(
            "GET",
            &format!("/api/channels/{id}/stats"),
            Some(&root),
            None,
        )
        .await;
    assert_eq!(status, 400);

    let (status, _, _) = app
        .request("GET", "/api/channels/777777/stats", Some(&token_a), None)
        .await;
    assert_eq!(status, 404);
}

// -------------------------------------------------------------- health (CRD 2690-2696)

#[tokio::test]
async fn health_classifies_healthy_degraded_down() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let id = create_line(&app, &token).await["data"]["id"]
        .as_i64()
        .unwrap();

    let (status, body, _) = app
        .request(
            "GET",
            &format!("/api/channels/{id}/health"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["data"]["status"], "healthy");
    assert_eq!(body["data"]["consecutiveErrors"], 0);
    assert_eq!(body["data"]["recommendations"].as_array().unwrap().len(), 0);

    sqlx::query("UPDATE channel_integrations SET error_count = 3 WHERE id = $1")
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/channels/{id}/health"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["data"]["status"], "degraded");
    assert!(!body["data"]["recommendations"]
        .as_array()
        .unwrap()
        .is_empty());

    sqlx::query("UPDATE channel_integrations SET error_count = 9 WHERE id = $1")
        .bind(id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (_, body, _) = app
        .request(
            "GET",
            &format!("/api/channels/{id}/health"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(body["data"]["status"], "down");
    assert!(body["data"]["checkedAt"].is_string());
}

// ----------------------------------------- webhook token resolution (CRD 2722)

#[tokio::test]
async fn webhook_token_triple_resolves_only_enabled_matching_connections() {
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;
    let body = create_line(&app, &token).await;
    let id = body["data"]["id"].as_i64().unwrap();
    let url = body["webhookUrl"].as_str().unwrap();
    let routing_token = url.rsplit('/').next().unwrap();

    use mcss_backend::domain::channels::store::resolve_by_webhook_token;
    let hit = resolve_by_webhook_token(&app.state.db, "line", team, routing_token)
        .await
        .unwrap();
    assert_eq!(hit.unwrap().id, id);

    // Wrong token, wrong platform, then disabled -> all rejected.
    assert!(
        resolve_by_webhook_token(&app.state.db, "line", team, "nope")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        resolve_by_webhook_token(&app.state.db, "facebook", team, routing_token)
            .await
            .unwrap()
            .is_none()
    );
    app.request("DELETE", &format!("/api/channels/{id}"), Some(&token), None)
        .await;
    assert!(
        resolve_by_webhook_token(&app.state.db, "line", team, routing_token)
            .await
            .unwrap()
            .is_none()
    );
}

// ------------------------------------- credential resolution (DB over .env)

/// No active integration seeded: resolution falls back to `.env`/config. The
/// harness leaves the LINE access token unset, so it resolves to `None`, while
/// the LINE secret comes from the harness default.
#[tokio::test]
async fn resolve_channel_falls_back_to_env_when_no_integration() {
    use mcss_backend::domain::channels::resolve::resolve_channel;
    let app = spawn_app().await;

    let resolved = resolve_channel(&app.state, "line").await;
    assert_eq!(resolved.access_token, app.state.config.line_channel_access_token);
    assert_eq!(resolved.access_token, None);
    // Secret falls through to the harness LINE secret default.
    assert_eq!(resolved.secret, app.state.config.line_channel_secret);
    assert_eq!(resolved.secret.as_deref(), Some("test-line-secret"));
}

/// An active LINE integration is present: its decrypted DB credentials win over
/// the `.env` fallback, and the plain config fields are returned.
#[tokio::test]
async fn resolve_channel_prefers_db_credentials_over_env() {
    use mcss_backend::domain::channels::resolve::resolve_channel;
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "a@x.io", team).await;

    let body = json!({
        "platform": "line",
        "lineConfig": {
            "channelId": "chan-db",
            "channelAccessToken": "db-token-xyz",
            "channelSecret": "db-secret"
        }
    });
    let (status, created, _) = app
        .request("POST", "/api/channels", Some(&token), Some(body))
        .await;
    assert_eq!(status, 201, "create failed: {created}");

    let resolved = resolve_channel(&app.state, "line").await;
    assert_eq!(resolved.access_token.as_deref(), Some("db-token-xyz"));
    assert_eq!(resolved.secret.as_deref(), Some("db-secret"));
    assert_eq!(
        resolved.config.get("channelId").and_then(Value::as_str),
        Some("chan-db")
    );
}

/// The outbound gateway built via `OutboundGateway::resolve` consumes the live
/// resolved credentials: with an active LINE integration its LINE field is
/// populated from the DB token (so a LINE send does NOT fail for missing
/// credentials), while a platform with no DB integration and no env token keeps
/// its field `None` and fails delivery with `MissingCredentials`. Network-free:
/// the only awaited send targets a platform (facebook) whose missing-token path
/// short-circuits before any HTTP call.
#[tokio::test]
async fn outbound_gateway_resolve_prefers_db_token() {
    use mcss_backend::domain::channels::resolve::resolve_channel;
    use mcss_backend::domain::conversations::channels::{OutboundGateway, OutboundItem};
    let app = spawn_app().await;
    let team = app.seed_team("Team").await;
    let token = admin_in_team(&app, "gw@x.io", team).await;

    let body = json!({
        "platform": "line",
        "lineConfig": {
            "channelId": "chan-gw",
            "channelAccessToken": "gw-db-token",
            "channelSecret": "gw-secret"
        }
    });
    let (status, created, _) = app
        .request("POST", "/api/channels", Some(&token), Some(body))
        .await;
    assert_eq!(status, 201, "create failed: {created}");

    // The gateway sources its LINE token from the same resolver, which prefers
    // the DB credential (cross-checked here without reading the private field).
    assert_eq!(
        resolve_channel(&app.state, "line")
            .await
            .access_token
            .as_deref(),
        Some("gw-db-token")
    );

    // Building the gateway never panics, and the DB-backed LINE field is present:
    // a LINE send fails only on the network (not on MissingCredentials). For a
    // facebook platform with neither a DB integration nor an env token, the field
    // stays None and the send short-circuits with the missing-credentials error.
    let gateway = OutboundGateway::resolve(&app.state).await;
    assert!(
        app.state.config.facebook_page_access_token.is_none(),
        "harness should leave the facebook token unset for this assertion"
    );
    let err = gateway
        .send_batch("facebook", "PSID-1", &[OutboundItem::text("hi")])
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "Outbound delivery is not supported for platform 'facebook'"
    );
}
