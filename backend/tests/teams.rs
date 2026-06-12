//! Behavioral tests for the Teams domain (CRD §3.2, lines 1792-2154).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::json;

async fn admin_token(app: &TestApp) -> String {
    app.seed_agent("admin@test.com", "password1", "admin").await;
    app.login("admin@test.com", "password1").await.0
}

async fn membership(app: &TestApp, agent_id: &str, team_id: i64) -> Option<(String, i64)> {
    sqlx::query_as("SELECT role, is_primary FROM team_members WHERE agent_id = $1 AND team_id = $2")
        .bind(agent_id)
        .bind(team_id)
        .fetch_optional(&app.state.db)
        .await
        .unwrap()
}

// ------------------------------------------------------------- health / info / qr test

#[tokio::test]
async fn health_and_info_are_public() {
    let app = spawn_app().await;
    let (status, body, _) = app.request("GET", "/api/teams/health", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "healthy");
    assert_eq!(body["data"]["module"], "teams");
    assert!(body["data"]["version"].is_string());

    let (status, body, _) = app.request("GET", "/api/teams/info", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["module"], "teams");
    assert!(body["data"]["endpoints"].is_array());
}

#[tokio::test]
async fn qr_code_test_endpoint_is_public() {
    let app = spawn_app().await;
    let (status, body, _) =
        app.request("POST", "/api/teams/7/qr-code-test", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["test"], true);
    assert!(body["data"]["imageUrl"].as_str().unwrap().contains("qr"));
}

// ----------------------------------------------------------------------- list teams

#[tokio::test]
async fn list_teams_admin_paginates_filters_and_searches() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let alpha = app.seed_team("Alpha Support").await;
    app.seed_team("Beta Sales").await;
    let inactive = app.seed_team("Sleepy").await;
    sqlx::query("UPDATE teams SET is_active = 0 WHERE id = $1")
        .bind(inactive)
        .execute(&app.state.db)
        .await
        .unwrap();

    let (status, body, _) = app.request("GET", "/api/teams", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 2); // inactive excluded by default
    assert_eq!(body["pagination"]["total"], 2);
    assert_eq!(body["pagination"]["page"], 1);

    let (_, body, _) = app
        .request("GET", "/api/teams?includeInactive=true", Some(&token), None)
        .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 3);

    let (_, body, _) = app.request("GET", "/api/teams?search=alpha", Some(&token), None).await;
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], alpha);
    assert!(items[0]["memberCount"].is_number());
}

#[tokio::test]
async fn list_teams_agent_with_primary_team_gets_only_that_team() {
    let app = spawn_app().await;
    let team = app.seed_team("Mine").await;
    app.seed_team("Other").await;
    let agent = app.seed_agent("agent@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "member", true).await;
    let token = app.login("agent@test.com", "password1").await.0;

    let (status, body, _) = app.request("GET", "/api/teams?page=2&limit=1", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], team);
    // No pagination block on the single-team agent path (CRD 1828).
    assert!(body.get("pagination").is_none());
}

// ------------------------------------------------------------------------- get team

#[tokio::test]
async fn get_team_returns_statistics() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Stats").await;
    let agent = app.seed_agent("m1@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "member", true).await;

    let (status, body, _) =
        app.request("GET", &format!("/api/teams/{team}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["memberCount"], 1);
    assert_eq!(body["data"]["activeMemberCount"], 1);
    assert_eq!(body["data"]["qrScanCount"], 0);
}

#[tokio::test]
async fn get_team_error_conditions() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    // Non-integer id -> 400 (CRD 1835).
    let (status, _, _) = app.request("GET", "/api/teams/abc", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Nonexistent -> 404.
    let (status, _, _) = app.request("GET", "/api/teams/9999", Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Non-member agent -> 403 access denied.
    let mine = app.seed_team("Mine").await;
    let other = app.seed_team("Other").await;
    let agent = app.seed_agent("agent2@test.com", "password1", "agent").await;
    app.add_membership(&agent, mine, "member", true).await;
    let agent_token = app.login("agent2@test.com", "password1").await.0;
    let (status, body, _) =
        app.request("GET", &format!("/api/teams/{other}"), Some(&agent_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["success"], false);
}

// ----------------------------------------------------------------------- create team

#[tokio::test]
async fn create_team_persists_and_attaches_qr_artifacts() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams",
            Some(&token),
            Some(json!({"name": "  New Team ", "description": "desc", "qrCode": "JOIN-1"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["name"], "New Team");
    assert_eq!(body["data"]["isActive"], true);
    assert_eq!(body["data"]["qrCode"], "JOIN-1");
    assert!(body["data"]["qrCodeImage"].is_string());
    assert!(body["data"]["joinUrl"].is_string());
    assert!(body["data"]["liffQr"]["imageUrl"].is_string());

    // Reversible create audit entry recorded (CRD 1840).
    let audits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM activity_logs WHERE action = 'team create'",
    )
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn create_team_error_conditions() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    // Empty name -> 400.
    let (status, _, _) = app
        .request("POST", "/api/teams", Some(&token), Some(json!({"name": "   "})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Duplicate qrCode -> 409.
    let (status, _, _) = app
        .request("POST", "/api/teams", Some(&token), Some(json!({"name": "A", "qrCode": "DUP"})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = app
        .request("POST", "/api/teams", Some(&token), Some(json!({"name": "B", "qrCode": "DUP"})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Malformed JSON -> 400.
    let (status, _) = app.request_raw("POST", "/api/teams", Some(&token), "{not json").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Non-admin -> 403.
    app.seed_agent("plain@test.com", "password1", "agent").await;
    let agent_token = app.login("plain@test.com", "password1").await.0;
    let (status, _, _) = app
        .request("POST", "/api/teams", Some(&agent_token), Some(json!({"name": "X"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------------- update team

#[tokio::test]
async fn update_team_by_in_team_supervisor() {
    let app = spawn_app().await;
    let team = app.seed_team("Old Name").await;
    let sup = app.seed_agent("sup@test.com", "password1", "agent").await;
    app.add_membership(&sup, team, "supervisor", true).await;
    let token = app.login("sup@test.com", "password1").await.0;

    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/teams/{team}"),
            Some(&token),
            Some(json!({"name": "New Name", "isActive": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["name"], "New Name");
    assert_eq!(body["data"]["isActive"], false);
    let audits: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM activity_logs WHERE action = 'team update'")
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(audits, 1);
}

#[tokio::test]
async fn update_team_error_conditions() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("T").await;
    // Insufficient in-team rank -> 403 with role context (CRD 1851).
    let member = app.seed_agent("mem@test.com", "password1", "agent").await;
    app.add_membership(&member, team, "member", true).await;
    let member_token = app.login("mem@test.com", "password1").await.0;
    let (status, body, _) = app
        .request("PUT", &format!("/api/teams/{team}"), Some(&member_token), Some(json!({"name": "x"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().contains("supervisor"));
    // Nonexistent team -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/teams/9999", Some(&admin), Some(json!({"name": "x"})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Non-integer id -> 400.
    let (status, _, _) = app
        .request("PUT", "/api/teams/xyz", Some(&admin), Some(json!({"name": "x"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ----------------------------------------------------------------------- delete team

#[tokio::test]
async fn delete_team_is_soft_and_audited() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    let team = app.seed_team("Doomed").await;
    let (status, body, _) =
        app.request("DELETE", &format!("/api/teams/{team}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], true);
    let deleted_at: Option<String> =
        sqlx::query_scalar("SELECT deleted_at FROM teams WHERE id = $1")
            .bind(team)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert!(deleted_at.is_some());
    // Subsequent reads see 404; nonexistent delete -> 404.
    let (status, _, _) =
        app.request("GET", &format!("/api/teams/{team}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) =
        app.request("DELETE", &format!("/api/teams/{team}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_team_requires_admin() {
    let app = spawn_app().await;
    let team = app.seed_team("Safe").await;
    let sup = app.seed_agent("sup2@test.com", "password1", "agent").await;
    app.add_membership(&sup, team, "supervisor", true).await;
    let token = app.login("sup2@test.com", "password1").await.0;
    let (status, _, _) =
        app.request("DELETE", &format!("/api/teams/{team}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------------- search teams

#[tokio::test]
async fn search_teams_matches_name_or_description() {
    let app = spawn_app().await;
    let token = admin_token(&app).await;
    app.seed_team("Apollo").await;
    sqlx::query("UPDATE teams SET description = 'lunar missions' WHERE name = 'Apollo'")
        .execute(&app.state.db)
        .await
        .unwrap();
    app.seed_team("Gemini").await;

    let (status, body, _) =
        app.request("GET", "/api/teams/search/LUNAR", Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["name"], "Apollo");

    // Blank query -> 400 (CRD 1867).
    let (status, _, _) = app.request("GET", "/api/teams/search/%20", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ------------------------------------------------------------------------- statistics

#[tokio::test]
async fn team_stats_reports_aggregates_and_period() {
    let app = spawn_app().await;
    let team = app.seed_team("Stats").await;
    let agent = app.seed_agent("stat@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "member", true).await;
    let customer = app.seed_customer("line", "u1", "Cust", Some(team)).await;
    app.seed_conversation(customer, Some(team), "active").await;
    let token = app.login("stat@test.com", "password1").await.0;

    let (status, body, _) = app
        .request("GET", &format!("/api/teams/{team}/stats?includeMembers=true"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["totalMembers"], 1);
    assert_eq!(body["data"]["conversationsHandled"], 1);
    assert_eq!(body["data"]["averageResponseTime"], 0);
    assert_eq!(body["data"]["qrScans"], 0);
    assert!(body["data"]["period"]["from"].is_string());
    assert_eq!(body["data"]["members"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn team_stats_error_conditions() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    // Nonexistent team surfaces as a server error (CRD 1874).
    let (status, _, _) = app.request("GET", "/api/teams/9999/stats", Some(&admin), None).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    // Non-member agent -> 403.
    let team = app.seed_team("Closed").await;
    app.seed_agent("nostats@test.com", "password1", "agent").await;
    let token = app.login("nostats@test.com", "password1").await.0;
    let (status, _, _) =
        app.request("GET", &format!("/api/teams/{team}/stats"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn all_team_stats_is_admin_only() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    app.seed_team("One").await;
    app.seed_team("Two").await;
    let (status, body, _) = app.request("GET", "/api/teams/stats/all", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);

    app.seed_agent("nob@test.com", "password1", "agent").await;
    let token = app.login("nob@test.com", "password1").await.0;
    let (status, _, _) = app.request("GET", "/api/teams/stats/all", Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// --------------------------------------------------------------------------- transfer

#[tokio::test]
async fn transfer_moves_agents_and_reports_failures() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let from = app.seed_team("From").await;
    let to = app.seed_team("To").await;
    let mover = app.seed_agent("mover@test.com", "password1", "agent").await;
    app.add_membership(&mover, from, "lead", true).await;
    let outsider = app.seed_agent("out@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams/transfer",
            Some(&admin),
            Some(json!({"fromTeamId": from, "toTeamId": to, "agentIds": [mover, outsider]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    // Overall success only when no failures (CRD 1886).
    assert_eq!(body["success"], false);
    assert_eq!(body["data"]["transferred"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["failed"].as_array().unwrap().len(), 1);
    assert!(membership(&app, &mover, from).await.is_none());
    // Role and primary flag preserved across the move (CRD 1885).
    let (role, primary) = membership(&app, &mover, to).await.unwrap();
    assert_eq!(role, "lead");
    assert_eq!(primary, 1);
}

#[tokio::test]
async fn transfer_validation_errors() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let (status, _, _) = app
        .request("POST", "/api/teams/transfer", Some(&admin), Some(json!({"toTeamId": 1, "agentIds": ["x"]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/teams/transfer", Some(&admin), Some(json!({"fromTeamId": 1, "toTeamId": 2, "agentIds": []})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------- team-scoped members

#[tokio::test]
async fn team_members_lists_sorted_by_name() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let b = app.seed_agent("bb@test.com", "password1", "agent").await;
    sqlx::query("UPDATE agents SET display_name = 'Zed' WHERE id = $1")
        .bind(&b)
        .execute(&app.state.db)
        .await
        .unwrap();
    let a = app.seed_agent("aa@test.com", "password1", "agent").await;
    sqlx::query("UPDATE agents SET display_name = 'Amy' WHERE id = $1")
        .bind(&a)
        .execute(&app.state.db)
        .await
        .unwrap();
    app.add_membership(&b, team, "member", true).await;
    app.add_membership(&a, team, "lead", true).await;

    let (status, body, _) =
        app.request("GET", &format!("/api/teams/{team}/members"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["displayName"], "Amy");
    assert_eq!(items[0]["roleInTeam"], "lead");
    assert_eq!(items[1]["displayName"], "Zed");
}

#[tokio::test]
async fn add_member_creates_primary_first_membership_without_duplicates() {
    let app = spawn_app().await;
    let team = app.seed_team("Crew").await;
    let lead = app.seed_agent("lead@test.com", "password1", "agent").await;
    app.add_membership(&lead, team, "lead", true).await;
    let token = app.login("lead@test.com", "password1").await.0;
    let newbie = app.seed_agent("new@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request("POST", &format!("/api/teams/{team}/members"), Some(&token), Some(json!({"agentId": newbie, "role": "supervisor"})))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // First-ever membership is primary; in-team role is base member despite the
    // `role` input (CRD 1898).
    let (role, primary) = membership(&app, &newbie, team).await.unwrap();
    assert_eq!(role, "member");
    assert_eq!(primary, 1);

    // Repeating does not duplicate.
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/{team}/members"), Some(&token), Some(json!({"agentId": newbie})))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM team_members WHERE agent_id = $1 AND team_id = $2",
    )
    .bind(&newbie)
    .bind(team)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(count, 1);

    // Blank agent id -> 400 (CRD 1900).
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/{team}/members"), Some(&token), Some(json!({"agentId": "  "})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn add_member_requires_lead_rank() {
    let app = spawn_app().await;
    let team = app.seed_team("Crew").await;
    let member = app.seed_agent("base@test.com", "password1", "agent").await;
    app.add_membership(&member, team, "member", true).await;
    let token = app.login("base@test.com", "password1").await.0;
    let other = app.seed_agent("o@test.com", "password1", "agent").await;
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/{team}/members"), Some(&token), Some(json!({"agentId": other})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn batch_add_members_adds_and_skips() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let existing = app.seed_agent("e@test.com", "password1", "agent").await;
    app.add_membership(&existing, team, "member", true).await;
    let fresh = app.seed_agent("f@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/teams/{team}/members/batch"),
            Some(&admin),
            Some(json!({"agentIds": [existing, fresh, "ghost"], "roleInTeam": "lead"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["addedCount"], 1);
    assert_eq!(body["data"]["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["errors"].as_array().unwrap().len(), 1);
    // New batch memberships are not primary (CRD 1905).
    let (role, primary) = membership(&app, &fresh, team).await.unwrap();
    assert_eq!(role, "lead");
    assert_eq!(primary, 0);

    // All skipped -> 200 (CRD 1906).
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/{team}/members/batch"), Some(&admin), Some(json!({"agentIds": [existing]})))
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn batch_add_members_validation_errors() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let path = format!("/api/teams/{team}/members/batch");
    // Empty array -> 400.
    let (status, _, _) = app.request("POST", &path, Some(&admin), Some(json!({"agentIds": []}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Over 50 -> 400.
    let many: Vec<String> = (0..51).map(|i| format!("id-{i}")).collect();
    let (status, _, _) = app.request("POST", &path, Some(&admin), Some(json!({"agentIds": many}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Invalid role -> 400.
    let (status, _, _) = app
        .request("POST", &path, Some(&admin), Some(json!({"agentIds": ["x"], "roleInTeam": "boss"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Nonexistent team -> 404.
    let (status, _, _) = app
        .request("POST", "/api/teams/9999/members/batch", Some(&admin), Some(json!({"agentIds": ["x"]})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_team_member_updates_global_account() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let agent = app.seed_agent("g@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "member", true).await;

    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/teams/{team}/members/{agent}"),
            Some(&admin),
            Some(json!({"role": "admin", "isActive": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["role"], "admin");
    assert_eq!(body["data"]["isActive"], false);
    // The per-team role is untouched (CRD 1913).
    let (role, _) = membership(&app, &agent, team).await.unwrap();
    assert_eq!(role, "member");
    // Blank agent id -> 400.
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/{team}/members/%20"), Some(&admin), Some(json!({"isActive": true})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn remove_team_member_promotes_new_primary() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("A").await;
    let b = app.seed_team("B").await;
    let agent = app.seed_agent("dual@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;
    app.add_membership(&agent, b, "member", false).await;

    let (status, body, _) = app
        .request("DELETE", &format!("/api/teams/{a}/members/{agent}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert!(membership(&app, &agent, a).await.is_none());
    let (_, primary) = membership(&app, &agent, b).await.unwrap();
    assert_eq!(primary, 1);

    // Removing an agent not in the team is a no-success outcome (CRD 1922).
    let (status, body, _) = app
        .request("DELETE", &format!("/api/teams/{a}/members/{agent}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["success"], false);
}

#[tokio::test]
async fn bulk_remove_members_reports_failures() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let inside = app.seed_agent("in@test.com", "password1", "agent").await;
    app.add_membership(&inside, team, "member", true).await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/teams/{team}/members/bulk-remove"),
            Some(&admin),
            Some(json!({"agentIds": [inside, "ghost"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["removedCount"], 1);
    assert_eq!(body["data"]["failed"].as_array().unwrap().len(), 1);

    // Empty -> 400; over 50 -> 400.
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/{team}/members/bulk-remove"), Some(&admin), Some(json!({"agentIds": []})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// -------------------------------------------------------------------- member accounts

#[tokio::test]
async fn list_all_members_is_admin_only_and_enriched() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Home").await;
    let agent = app.seed_agent("enr@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "lead", true).await;

    let (status, body, _) = app.request("GET", "/api/teams/members", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let enriched = items.iter().find(|m| m["id"] == json!(agent)).unwrap();
    assert_eq!(enriched["teamCount"], 1);
    assert_eq!(enriched["primaryTeamId"], team);
    assert_eq!(enriched["primaryTeamName"], "Home");
    assert_eq!(enriched["teams"][0]["roleInTeam"], "lead");

    let token = app.login("enr@test.com", "password1").await.0;
    let (status, _, _) = app.request("GET", "/api/teams/members", Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn check_email_reports_active_and_deleted_accounts() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    // Missing email -> 400.
    let (status, _, _) =
        app.request("GET", "/api/teams/members/check-email", Some(&admin), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unknown email.
    let (status, body, _) = app
        .request("GET", "/api/teams/members/check-email?email=nobody@x.com", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["exists"], false);
    // Active account.
    app.seed_agent("known@test.com", "password1", "agent").await;
    let (_, body, _) = app
        .request("GET", "/api/teams/members/check-email?email=known@test.com", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["exists"], true);
    assert_eq!(body["data"]["isDeleted"], false);
    // Soft-deleted account.
    let gone = app.seed_agent("gone@test.com", "password1", "agent").await;
    sqlx::query("UPDATE agents SET deleted_at = '2026-01-01T00:00:00.000Z' WHERE id = $1")
        .bind(&gone)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (_, body, _) = app
        .request("GET", "/api/teams/members/check-email?email=gone@test.com", Some(&admin), None)
        .await;
    assert_eq!(body["data"]["exists"], true);
    assert_eq!(body["data"]["isDeleted"], true);
}

#[tokio::test]
async fn create_member_with_team_and_reactivation() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Home").await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams/members",
            Some(&admin),
            Some(json!({
                "email": "fresh@test.com", "password": "password1",
                "displayName": "Fresh", "teamId": team,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let member_id = body["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["data"]["role"], "agent");
    let (_, primary) = membership(&app, &member_id, team).await.unwrap();
    assert_eq!(primary, 1);

    // Active duplicate -> 409.
    let (status, _, _) = app
        .request(
            "POST",
            "/api/teams/members",
            Some(&admin),
            Some(json!({"email": "fresh@test.com", "password": "x", "displayName": "Dup"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Soft-deleted account is reactivated with memberships cleared (CRD 1949).
    sqlx::query("UPDATE agents SET deleted_at = '2026-01-01T00:00:00.000Z' WHERE id = $1")
        .bind(&member_id)
        .execute(&app.state.db)
        .await
        .unwrap();
    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams/members",
            Some(&admin),
            Some(json!({"email": "fresh@test.com", "password": "newpw", "displayName": "Reborn"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["id"], member_id);
    assert_eq!(body["data"]["displayName"], "Reborn");
    assert!(membership(&app, &member_id, team).await.is_none());

    // Missing required fields -> 400.
    let (status, _, _) = app
        .request("POST", "/api/teams/members", Some(&admin), Some(json!({"email": "p@x.com"})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn set_member_status_with_guards() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let target = app.seed_agent("tgt@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/teams/members/{target}/status"),
            Some(&admin),
            Some(json!({"isActive": false, "reason": "vacation"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["isActive"], false);

    // isActive omitted -> 400.
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/members/{target}/status"), Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Own account -> 403.
    let admin_id: String = sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@test.com'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/members/{admin_id}/status"), Some(&admin), Some(json!({"isActive": false})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Unknown member -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/teams/members/ghost/status", Some(&admin), Some(json!({"isActive": true})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn set_member_role_with_guards() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let target = app.seed_agent("rl@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request("PUT", &format!("/api/teams/members/{target}/role"), Some(&admin), Some(json!({"role": "admin"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["role"], "admin");

    // role omitted -> 400.
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/members/{target}/role"), Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Own account -> 403.
    let admin_id: String = sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@test.com'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/members/{admin_id}/role"), Some(&admin), Some(json!({"role": "agent"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Unknown member -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/teams/members/ghost/role", Some(&admin), Some(json!({"role": "agent"})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_member_account_applies_subset() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let target = app.seed_agent("upd@test.com", "password1", "agent").await;
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/teams/members/{target}"),
            Some(&admin),
            Some(json!({"displayName": "Renamed", "email": "renamed@test.com"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["displayName"], "Renamed");
    assert_eq!(body["data"]["email"], "renamed@test.com");
    // Unknown member -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/teams/members/ghost", Some(&admin), Some(json!({"displayName": "X"})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_member_account_is_permanent() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let target = app.seed_agent("dead@test.com", "password1", "agent").await;
    app.add_membership(&target, team, "member", true).await;
    sqlx::query(
        "INSERT INTO notifications (id, agent_id, title, created_at) VALUES ('n1', $1, 't', '2026-01-01')",
    )
    .bind(&target)
    .execute(&app.state.db)
    .await
    .unwrap();

    let (status, body, _) = app
        .request("DELETE", &format!("/api/teams/members/{target}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["deletedMemberId"], target);
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE id = $1")
        .bind(&target)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    let notifications: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE agent_id = $1")
        .bind(&target)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(notifications, 0);

    // Own account -> 403; unknown -> 404.
    let admin_id: String = sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@test.com'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) =
        app.request("DELETE", &format!("/api/teams/members/{admin_id}"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) =
        app.request("DELETE", "/api/teams/members/ghost", Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn bulk_delete_members_with_guards() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_agent("bd1@test.com", "password1", "agent").await;
    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams/members/bulk-delete",
            Some(&admin),
            Some(json!({"memberIds": [a, "ghost"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["deletedCount"], 1);
    assert_eq!(body["data"]["failed"].as_array().unwrap().len(), 1);

    // Empty -> 400; including own id -> 403.
    let (status, _, _) = app
        .request("POST", "/api/teams/members/bulk-delete", Some(&admin), Some(json!({"memberIds": []})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let admin_id: String = sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@test.com'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let (status, _, _) = app
        .request("POST", "/api/teams/members/bulk-delete", Some(&admin), Some(json!({"memberIds": [admin_id]})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bulk_update_members_skips_self_and_validates() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let admin_id: String = sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@test.com'")
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    let a = app.seed_agent("bu1@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams/members/bulk-update",
            Some(&admin),
            Some(json!({
                "memberIds": [a, admin_id, "ghost"],
                "updates": {"isActive": false},
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["updatedCount"], 1);
    assert_eq!(body["data"]["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["failed"].as_array().unwrap().len(), 1);
    let active: i64 = sqlx::query_scalar("SELECT is_active FROM agents WHERE id = $1")
        .bind(&a)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(active, 0);

    // No update field -> 400; invalid role -> 400.
    let (status, _, _) = app
        .request("POST", "/api/teams/members/bulk-update", Some(&admin), Some(json!({"memberIds": [a], "updates": {}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("POST", "/api/teams/members/bulk-update", Some(&admin), Some(json!({"memberIds": [a], "updates": {"role": "boss"}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ------------------------------------------------------------------ batch edit & undo

#[tokio::test]
async fn batch_edit_members_and_undo_restore() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team_a = app.seed_team("A").await;
    let team_b = app.seed_team("B").await;
    let target = app.seed_agent("be@test.com", "password1", "agent").await;
    app.add_membership(&target, team_a, "member", true).await;

    let (status, body, _) = app
        .request(
            "POST",
            "/api/teams/members/batch-edit",
            Some(&admin),
            Some(json!({
                "members": [{
                    "memberId": target,
                    "profile": {"displayName": "Edited"},
                    "teamChanges": {"add": [team_b], "remove": [team_a]},
                }],
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["successCount"], 1);
    let result = &body["data"]["results"][0];
    assert_eq!(result["success"], true);
    assert_eq!(result["profileUpdated"], true);
    assert_eq!(result["teamsAdded"][0], team_b);
    assert_eq!(result["teamsRemoved"][0], team_a);
    let undo_token = body["data"]["undoToken"].as_str().unwrap().to_string();
    assert!(body["data"]["undoExpiresAt"].is_string());
    assert!(membership(&app, &target, team_a).await.is_none());
    assert!(membership(&app, &target, team_b).await.is_some());

    // Undo restores prior profile and memberships (CRD 2007).
    let (status, body, _) = app
        .request("POST", "/api/teams/members/batch-edit/undo", Some(&admin), Some(json!({"undoToken": undo_token})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["restoredCount"], 1);
    let name: String = sqlx::query_scalar("SELECT display_name FROM agents WHERE id = $1")
        .bind(&target)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "agent user");
    let (_, primary) = membership(&app, &target, team_a).await.unwrap();
    assert_eq!(primary, 1);
    assert!(membership(&app, &target, team_b).await.is_none());

    // A consumed token is invalid (CRD 2007).
    let (status, _, _) = app
        .request("POST", "/api/teams/members/batch-edit/undo", Some(&admin), Some(json!({"undoToken": body["data"]["results"][0]["memberId"]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn batch_edit_validation_errors() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let path = "/api/teams/members/batch-edit";
    // Empty list -> 400.
    let (status, _, _) = app.request("POST", path, Some(&admin), Some(json!({"members": []}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Missing memberId -> 400.
    let (status, _, _) = app
        .request("POST", path, Some(&admin), Some(json!({"members": [{"profile": {"displayName": "X"}}]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // No actual changes -> 400.
    let (status, _, _) = app
        .request("POST", path, Some(&admin), Some(json!({"members": [{"memberId": "someone"}]})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn undo_is_owner_bound() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    app.seed_agent("admin2@test.com", "password1", "admin").await;
    let admin2 = app.login("admin2@test.com", "password1").await.0;
    let target = app.seed_agent("ob@test.com", "password1", "agent").await;

    let (_, body, _) = app
        .request(
            "POST",
            "/api/teams/members/batch-edit",
            Some(&admin),
            Some(json!({"members": [{"memberId": target, "profile": {"displayName": "Zz"}}]})),
        )
        .await;
    let token = body["data"]["undoToken"].as_str().unwrap().to_string();

    // Missing token -> 400.
    let (status, _, _) = app
        .request("POST", "/api/teams/members/batch-edit/undo", Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Different user -> 403, token survives.
    let (status, _, _) = app
        .request("POST", "/api/teams/members/batch-edit/undo", Some(&admin2), Some(json!({"undoToken": token})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Original user still succeeds.
    let (status, _, _) = app
        .request("POST", "/api/teams/members/batch-edit/undo", Some(&admin), Some(json!({"undoToken": token})))
        .await;
    assert_eq!(status, StatusCode::OK);
}

// -------------------------------------------------------------- agent-team association

#[tokio::test]
async fn agent_teams_visibility_scoping() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Visible").await;
    let agent = app.seed_agent("vis@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "member", true).await;
    let other = app.seed_agent("peek@test.com", "password1", "agent").await;
    let other_token = app.login("peek@test.com", "password1").await.0;
    let own_token = app.login("vis@test.com", "password1").await.0;

    // Admin sees anyone's teams.
    let (status, body, _) =
        app.request("GET", &format!("/api/teams/agent-teams/{agent}"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"][0]["teamName"], "Visible");
    // Self access OK.
    let (status, _, _) = app
        .request("GET", &format!("/api/teams/agent-teams/{agent}"), Some(&own_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    // Another agent -> 403 (CRD 2034).
    let (status, _, _) = app
        .request("GET", &format!("/api/teams/agent-teams/{agent}"), Some(&other_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let _ = other;
}

#[tokio::test]
async fn team_members_detail_includes_multi_team_info() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("A").await;
    let b = app.seed_team("B").await;
    let agent = app.seed_agent("multi@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;
    app.add_membership(&agent, b, "lead", false).await;

    let (status, body, _) = app
        .request("GET", &format!("/api/teams/agent-teams/team/{a}/members"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["teams"].as_array().unwrap().len(), 2);
    assert_eq!(items[0]["primaryTeamId"], a);
}

#[tokio::test]
async fn join_team_creates_membership_with_primary_handling() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("A").await;
    let b = app.seed_team("B").await;
    let agent = app.seed_agent("join@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/teams/agent-teams/{agent}/join"),
            Some(&admin),
            Some(json!({"teamId": b, "roleInTeam": "lead", "isPrimary": true})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // Old primary cleared so exactly one team remains primary (CRD 2045).
    let (_, a_primary) = membership(&app, &agent, a).await.unwrap();
    assert_eq!(a_primary, 0);
    let (role, b_primary) = membership(&app, &agent, b).await.unwrap();
    assert_eq!(role, "lead");
    assert_eq!(b_primary, 1);

    // Already a member -> 409; missing teamId -> 400.
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/agent-teams/{agent}/join"), Some(&admin), Some(json!({"teamId": b})))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/agent-teams/{agent}/join"), Some(&admin), Some(json!({})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn join_multiple_adds_and_skips() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("A").await;
    let b = app.seed_team("B").await;
    let agent = app.seed_agent("jm@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/teams/agent-teams/{agent}/join-multiple"),
            Some(&admin),
            Some(json!({"teamIds": [a, b, 9999]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["added"].as_array().unwrap(), &vec![json!(b)]);
    assert_eq!(body["data"]["skipped"].as_array().unwrap(), &vec![json!(a)]);
    assert_eq!(body["data"]["errors"].as_array().unwrap().len(), 1);
    // New memberships are not primary (CRD 2052).
    let (_, primary) = membership(&app, &agent, b).await.unwrap();
    assert_eq!(primary, 0);

    // Missing/empty teamIds -> 400.
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/agent-teams/{agent}/join-multiple"), Some(&admin), Some(json!({"teamIds": []})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn leave_team_promotes_notifies_and_counts_conversations() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("Main").await;
    let b = app.seed_team("Side").await;
    let agent = app.seed_agent("lv@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;
    app.add_membership(&agent, b, "member", false).await;
    let customer = app.seed_customer("line", "u9", "C", Some(a)).await;
    app.seed_conversation(customer, Some(a), "active").await;

    let (status, body, _) = app
        .request("DELETE", &format!("/api/teams/agent-teams/{agent}/leave/{a}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["teamName"], "Main");
    assert_eq!(body["data"]["affectedConversations"], 1);
    assert!(membership(&app, &agent, a).await.is_none());
    let (_, primary) = membership(&app, &agent, b).await.unwrap();
    assert_eq!(primary, 1);
    // Persisted high-priority personal notification (CRD 2151).
    let notif: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notifications WHERE agent_id = $1 AND type = 'team_removal'",
    )
    .bind(&agent)
    .fetch_one(&app.state.db)
    .await
    .unwrap();
    assert_eq!(notif, 1);
}

#[tokio::test]
async fn update_membership_role_and_primary() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("A").await;
    let b = app.seed_team("B").await;
    let agent = app.seed_agent("mr@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;
    app.add_membership(&agent, b, "member", false).await;

    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/teams/agent-teams/{agent}/role/{b}"),
            Some(&admin),
            Some(json!({"roleInTeam": "supervisor", "isPrimary": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["roleInTeam"], "supervisor");
    assert_eq!(body["data"]["isPrimary"], true);
    let (_, a_primary) = membership(&app, &agent, a).await.unwrap();
    assert_eq!(a_primary, 0);

    // Missing membership surfaces as a server error (CRD 2067).
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/agent-teams/ghost/role/{b}"), Some(&admin), Some(json!({"roleInTeam": "lead"})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn set_primary_team_requires_membership() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_team("A").await;
    let b = app.seed_team("B").await;
    let agent = app.seed_agent("pp@test.com", "password1", "agent").await;
    app.add_membership(&agent, a, "member", true).await;
    app.add_membership(&agent, b, "member", false).await;

    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/agent-teams/{agent}/primary/{b}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, a_primary) = membership(&app, &agent, a).await.unwrap();
    let (_, b_primary) = membership(&app, &agent, b).await.unwrap();
    assert_eq!((a_primary, b_primary), (0, 1));

    // Not a member -> server error (CRD 2074).
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/agent-teams/ghost/primary/{b}"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

// -------------------------------------------------------------------------- QR family

#[tokio::test]
async fn generate_and_list_team_qr_codes() {
    let app = spawn_app().await;
    let team = app.seed_team("QR").await;
    let sup = app.seed_agent("qs@test.com", "password1", "agent").await;
    app.add_membership(&sup, team, "supervisor", true).await;
    let token = app.login("qs@test.com", "password1").await.0;

    let (status, body, _) = app
        .request(
            "POST",
            &format!("/api/teams/{team}/qr-code"),
            Some(&token),
            Some(json!({"campaignName": "Spring", "maxUses": 5})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["campaignName"], "Spring");
    assert_eq!(body["data"]["maxUses"], 5);
    assert_eq!(body["data"]["isActive"], true);

    // Missing body is tolerated (CRD 2079).
    let (status, _) =
        app.request_raw("POST", &format!("/api/teams/{team}/qr-code"), Some(&token), "").await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body, _) =
        app.request("GET", &format!("/api/teams/{team}/qr-codes"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn latest_and_fast_qr_lookup() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    // Team with no QR at all -> 404.
    let bare = app.seed_team("Bare").await;
    let (status, _, _) =
        app.request("GET", &format!("/api/teams/{bare}/qr-code/latest"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) =
        app.request("GET", &format!("/api/teams/{bare}/qr-code/fast"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Generate a QR record (team image not yet cached) -> sourced from records.
    let (_, created, _) = app
        .request("POST", &format!("/api/teams/{bare}/qr-code"), Some(&admin), Some(json!({})))
        .await;
    assert!(created["data"]["imageUrl"].is_string());
    let (status, body, _) =
        app.request("GET", &format!("/api/teams/{bare}/qr-code/latest"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["fromCache"], false);
    assert!(body["data"]["qrCodeImage"].is_string());

    // A team created through the API caches its QR image -> cache hit.
    let (_, team_body, _) = app
        .request("POST", "/api/teams", Some(&admin), Some(json!({"name": "Cached"})))
        .await;
    let cached_id = team_body["data"]["id"].as_i64().unwrap();
    let (status, body, _) = app
        .request("GET", &format!("/api/teams/{cached_id}/qr-code/fast"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["source"], "cache");
}

#[tokio::test]
async fn deactivate_qr_code() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("QR").await;
    let (_, created, _) = app
        .request("POST", &format!("/api/teams/{team}/qr-code"), Some(&admin), Some(json!({})))
        .await;
    let qr_id = created["data"]["id"].as_str().unwrap().to_string();

    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/{team}/qr-codes/{qr_id}/deactivate"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let active: i64 = sqlx::query_scalar("SELECT is_active FROM qr_codes WHERE id = $1")
        .bind(&qr_id)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(active, 0);

    // Blank QR id -> 400 (CRD 2106); unknown -> 404.
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/{team}/qr-codes/%20/deactivate"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("PUT", &format!("/api/teams/{team}/qr-codes/ghost/deactivate"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn liff_qr_lifecycle() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Liff").await;

    // None exists yet -> 404 for read and stats.
    let (status, _, _) =
        app.request("GET", &format!("/api/teams/{team}/qr-code/liff"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = app
        .request("GET", &format!("/api/teams/{team}/qr-code/liff/stats"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Generation is admin-only (CRD 2116).
    app.seed_agent("nl@test.com", "password1", "agent").await;
    let agent_token = app.login("nl@test.com", "password1").await.0;
    let (status, _, _) = app
        .request("POST", &format!("/api/teams/{team}/qr-code/liff"), Some(&agent_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body, _) =
        app.request("POST", &format!("/api/teams/{team}/qr-code/liff"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["url"].as_str().unwrap().contains("liff"));

    let (status, body, _) =
        app.request("GET", &format!("/api/teams/{team}/qr-code/liff"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["isActive"], true);

    let (status, body, _) = app
        .request("GET", &format!("/api/teams/{team}/qr-code/liff/stats"), Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["scanCount"], 0);
    assert_eq!(body["data"]["customerAssignments"], 0);

    // Regenerating a missing team -> 404.
    let (status, _, _) =
        app.request("POST", "/api/teams/9999/qr-code/liff", Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// -------------------------------------------------------------------- auth boundaries

#[tokio::test]
async fn team_routes_require_authentication() {
    let app = spawn_app().await;
    for (method, path) in [
        ("GET", "/api/teams"),
        ("GET", "/api/teams/1"),
        ("GET", "/api/teams/members"),
        ("POST", "/api/teams/transfer"),
        ("GET", "/api/teams/agent-teams/some-agent"),
    ] {
        let (status, _, _) = app.request(method, path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}
