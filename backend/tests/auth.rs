//! Auth & account management per CRD §1.1 (lines 126-293) and the protected-endpoint
//! access-control behavior (lines 265-273).

mod common;

use axum::http::StatusCode;
use common::spawn_app;
use serde_json::json;

// ---------------------------------------------------------------- Sign In

#[tokio::test]
async fn login_requires_email_and_password() {
    let app = spawn_app().await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "  "})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn login_failures_share_one_generic_message() {
    let app = spawn_app().await;
    app.seed_agent("real@test.dev", "correct-password", "agent")
        .await;

    // Unknown email and wrong password must be indistinguishable (CRD 139).
    let (s1, b1, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "ghost@test.dev", "password": "x"})),
        )
        .await;
    let (s2, b2, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "real@test.dev", "password": "wrong"})),
        )
        .await;
    assert_eq!(s1, StatusCode::UNAUTHORIZED);
    assert_eq!(s2, StatusCode::UNAUTHORIZED);
    assert_eq!(b1["error"], b2["error"]);
}

#[tokio::test]
async fn disabled_account_cannot_sign_in() {
    let app = spawn_app().await;
    let id = app.seed_agent("off@test.dev", "pw123456", "agent").await;
    sqlx::query("UPDATE agents SET is_active = 0 WHERE id = $1")
        .bind(&id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "off@test.dev", "password": "pw123456"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Invalid email or password");
}

#[tokio::test]
async fn successful_login_returns_tokens_session_and_agent_view() {
    let app = spawn_app().await;
    app.seed_agent("a@test.dev", "pw123456", "agent").await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "a@test.dev", "password": "pw123456"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let data = &body["data"];
    assert!(data["token"].is_string());
    assert!(data["refreshToken"].is_string());
    assert!(data["sessionId"].is_string());
    assert_eq!(data["expiresIn"], 7200);
    let agent = &data["agent"];
    assert_eq!(agent["email"], "a@test.dev");
    assert_eq!(agent["role"], "agent");
    assert_eq!(agent["isActive"], true);
    assert!(
        agent["createdAt"].is_i64(),
        "createdAt must be epoch millis"
    );
    // Sign-in recorded in the activity log (CRD 139).
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM activity_logs WHERE action = 'login'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn must_change_policy_diverts_login_to_forced_change_flow() {
    let app = spawn_app().await;
    let id = app.seed_agent("mc@test.dev", "pw123456", "agent").await;
    sqlx::query("UPDATE agents SET password_policy = 'must_change' WHERE id = $1")
        .bind(&id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "mc@test.dev", "password": "pw123456"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["mustChangePassword"], true);
    assert!(body["data"]["tempToken"].is_string());
    assert!(body["data"].get("token").is_none(), "no full sign-in");

    let temp = body["data"]["tempToken"].as_str().unwrap();
    let (status, _, _) = app.request("GET", "/api/auth/me", Some(temp), None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "temp-change token is not full access"
    );
}

// ---------------------------------------------------------------- Create Account

#[tokio::test]
async fn register_requires_admin() {
    let app = spawn_app().await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (token, _, _) = app.login("agent@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request("POST", "/api/auth/register", Some(&token),
            Some(json!({"email": "n@test.dev", "password": "pw", "displayName": "N", "role": "agent"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = app
        .request("POST", "/api/auth/register", None,
            Some(json!({"email": "n@test.dev", "password": "pw", "displayName": "N", "role": "agent"})))
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn register_validates_fields_and_role() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let (token, _, _) = app.login("admin@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/register",
            Some(&token),
            Some(json!({"email": "n@test.dev", "password": "pw"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, _) = app
        .request("POST", "/api/auth/register", Some(&token),
            Some(json!({"email": "n@test.dev", "password": "pw", "displayName": "N", "role": "superuser"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_creates_account_and_conflicts_on_duplicate() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let team_id = app.seed_team("Support").await;
    let (token, _, _) = app.login("admin@test.dev", "pw123456").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/register",
            Some(&token),
            Some(json!({"email": "new@test.dev", "password": "pw123456",
                        "displayName": "Newbie", "role": "agent", "teamId": team_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["user"]["email"], "new@test.dev");
    assert_eq!(body["data"]["user"]["teamId"], team_id);
    assert_eq!(body["data"]["user"]["teamName"], "Support");

    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/register",
            Some(&token),
            Some(json!({"email": "new@test.dev", "password": "pw123456",
                        "displayName": "Dup", "role": "agent"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn register_reactivates_soft_deleted_account_in_place() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let old_id = app.seed_agent("back@test.dev", "oldpw1234", "agent").await;
    let team_id = app.seed_team("T1").await;
    app.add_membership(&old_id, team_id, "member", true).await;
    sqlx::query("UPDATE agents SET deleted_at = $1 WHERE id = $2")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(&old_id)
        .execute(&app.state.db)
        .await
        .unwrap();

    let (token, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/register",
            Some(&token),
            Some(json!({"email": "back@test.dev", "password": "newpw1234",
                        "displayName": "Back", "role": "agent"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Same record reactivated, not duplicated (CRD 149/153).
    assert_eq!(body["data"]["user"]["id"], old_id.as_str());
    let memberships: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM team_members WHERE agent_id = $1")
            .bind(&old_id)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(memberships, 0, "prior team memberships are cleared");
}

// ---------------------------------------------------------------- Sign Out

#[tokio::test]
async fn logout_requires_session_and_revokes_credentials() {
    let app = spawn_app().await;
    app.seed_agent("out@test.dev", "pw123456", "agent").await;
    let (token, refresh, session) = app.login("out@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request("POST", "/api/auth/logout", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "missing X-Session-ID");

    let (status, body, _) = app
        .request_with_headers(
            "POST",
            "/api/auth/logout",
            Some(&token),
            Some(json!({"refreshToken": refresh})),
            &[("X-Session-ID", session.as_str())],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Revoked access credential is rejected on next use (CRD 163).
    let (status, _, _) = app.request("GET", "/api/auth/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------- Renew Credentials

#[tokio::test]
async fn refresh_rotates_and_detects_reuse() {
    let app = spawn_app().await;
    app.seed_agent("r@test.dev", "pw123456", "agent").await;
    let (_, refresh, _) = app.login("r@test.dev", "pw123456").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/refresh",
            None,
            Some(json!({"refreshToken": refresh})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["data"]["token"].is_string());
    let new_refresh = body["data"]["refreshToken"].as_str().unwrap().to_string();
    assert_ne!(new_refresh, refresh, "rolling rotation");

    // Replaying the consumed credential is a terminal event (CRD 169/172).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/refresh",
            None,
            Some(json!({"refreshToken": refresh})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .to_lowercase()
        .contains("reuse"));
}

#[tokio::test]
async fn refresh_validates_input_and_token_type() {
    let app = spawn_app().await;
    app.seed_agent("r2@test.dev", "pw123456", "agent").await;
    let (access, _, _) = app.login("r2@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request("POST", "/api/auth/refresh", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // An access credential is the wrong type (CRD 171).
    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/refresh",
            None,
            Some(json!({"refreshToken": access})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/refresh",
            None,
            Some(json!({"refreshToken": "garbage"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ------------------------------------------------ Protected-endpoint gate behavior

#[tokio::test]
async fn protected_endpoints_reject_bad_credentials() {
    let app = spawn_app().await;
    app.seed_agent("g@test.dev", "pw123456", "agent").await;
    let (_, refresh, _) = app.login("g@test.dev", "pw123456").await;

    let (status, _, _) = app.request("GET", "/api/auth/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _, _) = app
        .request("GET", "/api/auth/me", Some("not-a-jwt"), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Renewal credential presented as access credential (CRD 268).
    let (status, _, _) = app
        .request("GET", "/api/auth/me", Some(&refresh), None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn context_team_header_is_validated() {
    let app = spawn_app().await;
    let id = app.seed_agent("ctx@test.dev", "pw123456", "agent").await;
    let team = app.seed_team("Mine").await;
    let other = app.seed_team("NotMine").await;
    app.add_membership(&id, team, "member", true).await;
    let (token, _, _) = app.login("ctx@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request_with_headers(
            "GET",
            "/api/auth/me",
            Some(&token),
            None,
            &[("X-Context-Team-ID", "abc")],
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, _) = app
        .request_with_headers(
            "GET",
            "/api/auth/me",
            Some(&token),
            None,
            &[("X-Context-Team-ID", &other.to_string())],
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = app
        .request_with_headers(
            "GET",
            "/api/auth/me",
            Some(&token),
            None,
            &[("X-Context-Team-ID", &team.to_string())],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------- Profile

#[tokio::test]
async fn profile_and_me_return_user_views() {
    let app = spawn_app().await;
    let id = app.seed_agent("p@test.dev", "pw123456", "agent").await;
    let team = app.seed_team("Alpha").await;
    app.add_membership(&id, team, "lead", true).await;
    let (token, _, _) = app.login("p@test.dev", "pw123456").await;

    let (status, body, _) = app
        .request("GET", "/api/auth/profile", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["user"]["email"], "p@test.dev");
    assert_eq!(body["data"]["user"]["teamId"], team);
    assert_eq!(body["data"]["user"]["teamName"], "Alpha");

    let (status, body, _) = app.request("GET", "/api/auth/me", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["email"], "p@test.dev");
    assert!(body["data"]["createdAt"].is_i64());
}

#[tokio::test]
async fn update_me_enforces_allowlist_and_skips_noops() {
    let app = spawn_app().await;
    app.seed_agent("u@test.dev", "pw123456", "agent").await;
    let (token, _, _) = app.login("u@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request("PUT", "/api/auth/me", Some(&token), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no updatable field");

    let (status, _, _) = app
        .request(
            "PUT",
            "/api/auth/me",
            Some(&token),
            Some(json!({"displayName": "  "})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, _) = app
        .request(
            "PUT",
            "/api/auth/me",
            Some(&token),
            Some(json!({"displayName": "x".repeat(51)})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request(
            "PUT",
            "/api/auth/me",
            Some(&token),
            Some(json!({"displayName": "Renamed"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["displayName"], "Renamed");
    assert_eq!(body["message"], "Profile updated");

    // No-op update writes nothing and reports no changes (CRD 194/197).
    let (status, body, _) = app
        .request(
            "PUT",
            "/api/auth/me",
            Some(&token),
            Some(json!({"displayName": "Renamed"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "No changes");
}

// ---------------------------------------------------------------- Passwords

#[tokio::test]
async fn change_password_requires_current_password_proof() {
    let app = spawn_app().await;
    app.seed_agent("cp@test.dev", "oldpw1234", "agent").await;
    let (token, refresh, session) = app.login("cp@test.dev", "oldpw1234").await;

    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/change-password",
            Some(&token),
            Some(json!({"newPassword": "n"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/change-password",
            Some(&token),
            Some(json!({"currentPassword": "wrong", "newPassword": "newpw1234"})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'change_password_failed'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(audits, 1, "failed attempt is audited");

    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/change-password",
            Some(&token),
            Some(json!({"currentPassword": "oldpw1234", "newPassword": "newpw1234"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = app.request("GET", "/api/auth/me", Some(&token), None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "old access token invalidated"
    );
    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/refresh",
            None,
            Some(json!({"refreshToken": refresh})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "old refresh token revoked"
    );
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_sessions WHERE id = $1")
        .bind(&session)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "existing auth session deleted");
    // New password works on a fresh sign-in.
    app.login("cp@test.dev", "newpw1234").await;
}

#[tokio::test]
async fn reset_member_password_is_role_gated_and_blocks_self_reset() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let plain = app.seed_agent("plain@test.dev", "pw123456", "agent").await;
    let target = app.seed_agent("target@test.dev", "pw123456", "agent").await;

    // Plain member (no lead/supervisor membership) is refused.
    let (ptoken, _, _) = app.login("plain@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/teams/members/{target}/reset"),
            Some(&ptoken),
            Some(json!({"newPassword": "x12345678"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (atoken, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (target_token, target_refresh, target_session) =
        app.login("target@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/teams/members/{target}/reset"),
            Some(&atoken),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let admin_id: String =
        sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@test.dev'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let (status, _, _) = app
        .request(
            "POST",
            &format!("/api/teams/members/{admin_id}/reset"),
            Some(&atoken),
            Some(json!({"newPassword": "x12345678"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "self-reset disallowed here");

    let (status, _, _) = app
        .request(
            "POST",
            "/api/teams/members/no-such-user/reset",
            Some(&atoken),
            Some(json!({"newPassword": "x12345678"})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/teams/members/{target}/reset"),
            Some(&atoken),
            Some(json!({"newPassword": "fresh9999", "policy": "must_change"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["passwordPolicy"], "must_change");
    let _ = plain;
    let (status, _, _) = app
        .request("GET", "/api/auth/me", Some(&target_token), None)
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "target old access token invalidated"
    );
    let (status, _, _) = app
        .request(
            "POST",
            "/api/auth/refresh",
            None,
            Some(json!({"refreshToken": target_refresh})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "target old refresh token revoked"
    );
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_sessions WHERE id = $1")
        .bind(&target_session)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(sessions, 0, "target auth sessions deleted");

    // must_change diverts the target's next sign-in (CRD 215).
    let (status, body, _) = app
        .request(
            "POST",
            "/api/auth/login",
            None,
            Some(json!({"email": "target@test.dev", "password": "fresh9999"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["mustChangePassword"], true);
}

// ---------------------------------------------------------------- /phase2-auth

#[tokio::test]
async fn service_token_endpoints_are_admin_gated_and_validated() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("agent@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request("POST", "/phase2-auth/monitoring-token", Some(&agent), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/monitoring-token?expiresIn=10",
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "out-of-range expiresIn");

    let (status, body, _) = app
        .request("POST", "/phase2-auth/monitoring-token", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["type"], "monitoring");
    assert_eq!(body["data"]["expiresIn"], 604800);
    assert!(body["data"]["token"].is_string());

    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/user-token",
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "targetUserId required");

    let target_id: String =
        sqlx::query_scalar("SELECT id FROM agents WHERE email = 'agent@test.dev'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    let (status, body, _) = app
        .request(
            "POST",
            "/phase2-auth/user-token",
            Some(&admin),
            Some(json!({"targetUserId": target_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["type"], "user");
    assert_eq!(body["data"]["user"]["role"], "agent");
}

#[tokio::test]
async fn service_token_refresh_requires_admin_access_and_rotates_tokens() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("agent@test.dev", "pw123456").await;

    let (_, issued, _) = app
        .request("POST", "/phase2-auth/monitoring-token", Some(&admin), None)
        .await;
    let service_token = issued["data"]["token"].as_str().unwrap().to_string();

    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/refresh-token",
            None,
            Some(json!({"token": service_token})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "refresh is not public");

    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/refresh-token",
            Some(&agent),
            Some(json!({"token": service_token})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "non-admin access denied");

    let (status, body, _) = app
        .request(
            "POST",
            "/phase2-auth/refresh-token",
            Some(&admin),
            Some(json!({"token": service_token})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["type"], "monitoring");
    let refreshed = body["data"]["token"].as_str().unwrap();
    assert_ne!(refreshed, service_token);

    let refreshed_claims =
        mcss_backend::domain::auth::tokens::verify(refreshed, &app.state.config.jwt_secret)
            .unwrap();
    assert_eq!(refreshed_claims.token_type, "monitoring");
    let root_iat = refreshed_claims.service_root_iat.unwrap();
    assert!(root_iat <= refreshed_claims.iat);

    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/refresh-token",
            Some(&admin),
            Some(json!({"token": service_token})),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "old token was revoked");
}

#[tokio::test]
async fn verify_token_reports_validity_without_http_errors() {
    let app = spawn_app().await;
    app.seed_agent("v@test.dev", "pw123456", "agent").await;
    let (token, _, _) = app.login("v@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request("POST", "/phase2-auth/verify-token", None, Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request(
            "POST",
            "/phase2-auth/verify-token",
            None,
            Some(json!({"token": token})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["valid"], true);
    assert!(body["data"].get("payload").is_none());
    assert!(body["data"].get("remainingSeconds").is_none());

    // Invalid token: still HTTP 200, valid:false (CRD 247).
    let (status, body, _) = app
        .request(
            "POST",
            "/phase2-auth/verify-token",
            None,
            Some(json!({"token": "junk"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["valid"], false);
}

#[tokio::test]
async fn batch_tokens_rejected_in_production() {
    let app = common::spawn_app_with_env("production").await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/batch-tokens",
            Some(&admin),
            Some(json!({"users": [{"id": "u1"}]})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn batch_tokens_validates_and_issues_in_development() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;

    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/batch-tokens",
            Some(&admin),
            Some(json!({"users": []})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let too_many: Vec<_> = (0..11).map(|i| json!({"id": format!("u{i}")})).collect();
    let (status, _, _) = app
        .request(
            "POST",
            "/phase2-auth/batch-tokens",
            Some(&admin),
            Some(json!({"users": too_many})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, body, _) = app
        .request(
            "POST",
            "/phase2-auth/batch-tokens",
            Some(&admin),
            Some(json!({"users": [{"id": "u1"}, {"id": "u2"}]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["count"], 2);
    assert!(body["data"]["warning"].is_string());
}

#[tokio::test]
async fn auth_status_reports_admin_capability_map() {
    let app = spawn_app().await;
    app.seed_agent("admin@test.dev", "pw123456", "admin").await;
    app.seed_agent("agent@test.dev", "pw123456", "agent").await;
    let (admin, _, _) = app.login("admin@test.dev", "pw123456").await;
    let (agent, _, _) = app.login("agent@test.dev", "pw123456").await;

    let (status, body, _) = app
        .request("GET", "/phase2-auth/status", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["authenticated"], true);
    assert_eq!(
        body["data"]["permissions"]["canIssueMonitoringTokens"],
        true
    );

    let (_, body, _) = app
        .request("GET", "/phase2-auth/status", Some(&agent), None)
        .await;
    assert_eq!(
        body["data"]["permissions"]["canIssueMonitoringTokens"],
        false
    );
}

// ---------------------------------------------------------------- Auth sessions (§1.2A)

#[tokio::test]
async fn sessions_expire_and_are_deleted_on_logout() {
    let app = spawn_app().await;
    app.seed_agent("s@test.dev", "pw123456", "agent").await;
    let (token, _, session) = app.login("s@test.dev", "pw123456").await;

    // Session exists with a 24h expiry.
    let expires: String = sqlx::query_scalar("SELECT expires_at FROM auth_sessions WHERE id = $1")
        .bind(&session)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert!(expires > chrono::Utc::now().to_rfc3339());

    // Expired sessions stop resolving (logout then refuses with 401).
    sqlx::query("UPDATE auth_sessions SET expires_at = '2000-01-01T00:00:00Z' WHERE id = $1")
        .bind(&session)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request_with_headers(
            "POST",
            "/api/auth/logout",
            Some(&token),
            None,
            &[("X-Session-ID", session.as_str())],
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
