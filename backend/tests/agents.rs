//! Behavioral tests for the Agents/Operators domain (CRD §3.3, lines 2154-2321).

mod common;

use axum::http::StatusCode;
use common::{spawn_app, TestApp};
use serde_json::{json, Value};

async fn admin_token(app: &TestApp) -> String {
    app.seed_agent("admin@test.com", "password1", "admin").await;
    app.login("admin@test.com", "password1").await.0
}

/// Team-leader capability is granted to the distinct "team" role value (CRD 2303).
async fn leader_token(app: &TestApp) -> String {
    app.seed_agent("leader@test.com", "password1", "team").await;
    app.login("leader@test.com", "password1").await.0
}

async fn skill_payload() -> Value {
    json!({"name": "Negotiation", "category": "communication", "level": "advanced"})
}

// -------------------------------------------------------------------- list operators

#[tokio::test]
async fn list_agents_requires_privilege_and_paginates() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let leader = leader_token(&app).await;
    app.seed_agent("op1@test.com", "password1", "agent").await;

    let (status, body, _) = app.request("GET", "/api/agents", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 3);
    assert_eq!(body["pagination"]["total"], 3);
    // Password material is always blank (CRD 2168).
    assert_eq!(body["data"][0]["password"], "");

    // Team leaders are privileged too.
    let (status, _, _) = app.request("GET", "/api/agents", Some(&leader), None).await;
    assert_eq!(status, StatusCode::OK);

    // Ordinary operators are forbidden.
    let op = app.login("op1@test.com", "password1").await.0;
    let (status, _, _) = app.request("GET", "/api/agents", Some(&op), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_agents_rejects_invalid_pagination() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    for query in ["page=0", "page=1001", "page=abc", "limit=0", "limit=101", "limit=x"] {
        let (status, _, _) =
            app.request("GET", &format!("/api/agents?{query}"), Some(&admin), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
}

#[tokio::test]
async fn list_agents_filters() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Squad").await;
    let member = app.seed_agent("findme@test.com", "password1", "agent").await;
    app.add_membership(&member, team, "member", true).await;
    let inactive = app.seed_agent("sleepy@test.com", "password1", "agent").await;
    sqlx::query("UPDATE agents SET is_active = 0 WHERE id = ?")
        .bind(&inactive)
        .execute(&app.state.db)
        .await
        .unwrap();

    // Inactive excluded unless includeInactive=true (CRD 2165).
    let (_, body, _) = app.request("GET", "/api/agents", Some(&admin), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    let (_, body, _) =
        app.request("GET", "/api/agents?includeInactive=true", Some(&admin), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 3);

    // Search by email substring.
    let (_, body, _) = app.request("GET", "/api/agents?search=findme", Some(&admin), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["teamId"], team);
    assert_eq!(body["data"][0]["teamName"], "Squad");

    // Filter by team and role.
    let (_, body, _) =
        app.request("GET", &format!("/api/agents?teamId={team}"), Some(&admin), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    let (_, body, _) = app.request("GET", "/api/agents?role=admin", Some(&admin), None).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    // Unknown status values are ignored, not rejected (CRD 2165).
    let (status, _, _) =
        app.request("GET", "/api/agents?status=zzz", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
}

// ----------------------------------------------------------------------- bulk update

#[tokio::test]
async fn batch_update_applies_same_changes_best_effort() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let a = app.seed_agent("ba1@test.com", "password1", "agent").await;
    let b = app.seed_agent("ba2@test.com", "password1", "agent").await;
    let ghost = "ghost-0000-0000-0000-000000000000";

    let (status, body, _) = app
        .request(
            "PUT",
            "/api/agents/batch",
            Some(&admin),
            Some(json!({"agentIds": [a, b, ghost], "updates": {"isActive": false}})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Failing operators are silently omitted (CRD 2178).
    assert_eq!(body["data"].as_array().unwrap().len(), 2);
    assert!(body["message"].as_str().unwrap().contains("2"));
    let active: i64 = sqlx::query_scalar("SELECT is_active FROM agents WHERE id = ?")
        .bind(&a)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(active, 0);
}

#[tokio::test]
async fn batch_update_validation_and_authorization() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    // Empty array, over 50, out-of-range identifier -> 400 (CRD 2177).
    let (status, _, _) = app
        .request("PUT", "/api/agents/batch", Some(&admin), Some(json!({"agentIds": [], "updates": {"isActive": true}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let many: Vec<String> = (0..51).map(|i| format!("agent-id-{i:04}")).collect();
    let (status, _, _) = app
        .request("PUT", "/api/agents/batch", Some(&admin), Some(json!({"agentIds": many, "updates": {"isActive": true}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = app
        .request("PUT", "/api/agents/batch", Some(&admin), Some(json!({"agentIds": ["short"], "updates": {"isActive": true}})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Non-administrator -> 403 (team leaders not allowed here).
    let leader = leader_token(&app).await;
    let (status, _, _) = app
        .request("PUT", "/api/agents/batch", Some(&leader), Some(json!({"agentIds": ["agent-id-x-1"], "updates": {"isActive": true}})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// --------------------------------------------------------------------- bulk transfer

#[tokio::test]
async fn batch_transfer_replaces_memberships_with_primary() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let old_a = app.seed_team("OldA").await;
    let old_b = app.seed_team("OldB").await;
    let target = app.seed_team("Target").await;
    let agent = app.seed_agent("bt@test.com", "password1", "agent").await;
    app.add_membership(&agent, old_a, "lead", true).await;
    app.add_membership(&agent, old_b, "member", false).await;
    let ghost = "ghost-0000-0000-0000-000000000000";

    let (status, body, _) = app
        .request(
            "PUT",
            "/api/agents/batch/transfer",
            Some(&admin),
            Some(json!({"agentIds": [agent, ghost], "toTeamId": target})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Top-level success mirrors the error list (CRD 2185).
    assert_eq!(body["success"], false);
    assert_eq!(body["data"]["errors"].as_array().unwrap().len(), 1);

    let memberships: Vec<(i64, i64)> =
        sqlx::query_as("SELECT team_id, is_primary FROM team_members WHERE agent_id = ?")
            .bind(&agent)
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert_eq!(memberships, vec![(target, 1)]);
}

#[tokio::test]
async fn batch_transfer_errors() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = app.seed_agent("bt2@test.com", "password1", "agent").await;
    // Missing target team surfaces as a server error (CRD 2186).
    let (status, body, _) = app
        .request("PUT", "/api/agents/batch/transfer", Some(&admin), Some(json!({"agentIds": [agent], "toTeamId": 9999})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body["error"].as_str().unwrap().to_lowercase().contains("team not found"));
    // Validation failures -> 400.
    let (status, _, _) = app
        .request("PUT", "/api/agents/batch/transfer", Some(&admin), Some(json!({"agentIds": [], "toTeamId": 1})))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------- search

#[tokio::test]
async fn search_agents_filters_and_orders() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Searchers").await;
    let hit = app.seed_agent("needle@test.com", "password1", "agent").await;
    app.add_membership(&hit, team, "member", true).await;
    app.seed_agent("hay@test.com", "password1", "agent").await;

    let (status, body, _) = app
        .request("POST", "/api/agents/search", Some(&admin), Some(json!({"keyword": "needle"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["id"], hit);

    let (_, body, _) = app
        .request("POST", "/api/agents/search", Some(&admin), Some(json!({"teamIds": [team], "isActive": true})))
        .await;
    assert_eq!(body["data"].as_array().unwrap().len(), 1);

    // Ordinary operators forbidden.
    let op = app.login("hay@test.com", "password1").await.0;
    let (status, _, _) =
        app.request("POST", "/api/agents/search", Some(&op), Some(json!({"keyword": "x"}))).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------- status statistics

#[tokio::test]
async fn status_statistics_counts_presence_states() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let online = app.seed_agent("on@test.com", "password1", "agent").await;
    app.seed_agent("off@test.com", "password1", "agent").await;
    let token = app.login("on@test.com", "password1").await.0;
    let (status, _, _) = app
        .request("PUT", &format!("/api/agents/{online}/status"), Some(&token), Some(json!({"status": "online"})))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body, _) =
        app.request("GET", "/api/agents/status/statistics", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["online"], 1);
    // Operators without presence count as offline (CRD 2201): admin + off@.
    assert_eq!(body["data"]["offline"], 2);
    assert_eq!(body["data"]["busy"], 0);
    assert_eq!(body["data"]["meeting"], 0);

    // Ordinary operators forbidden.
    let (status, _, _) =
        app.request("GET", "/api/agents/status/statistics", Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ----------------------------------------------------------------------------- skills

#[tokio::test]
async fn skills_crud_with_scoping() {
    let app = spawn_app().await;
    let agent = app.seed_agent("sk@test.com", "password1", "agent").await;
    let token = app.login("sk@test.com", "password1").await.0;

    // Empty inventory.
    let (status, body, _) =
        app.request("GET", &format!("/api/agents/{agent}/skills"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 0);

    // Add own skill.
    let (status, body, _) = app
        .request("POST", &format!("/api/agents/{agent}/skills"), Some(&token), Some(skill_payload().await))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["name"], "Negotiation");
    assert_eq!(body["data"]["certified"], false);
    assert!(body["data"]["certifiedAt"].is_null());

    // Identifier length out of range -> 400.
    let (status, _, _) = app.request("GET", "/api/agents/short/skills", Some(&token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Ordinary operator targeting another operator -> 403.
    let other = app.seed_agent("sk2@test.com", "password1", "agent").await;
    let (status, _, _) =
        app.request("GET", &format!("/api/agents/{other}/skills"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Team leaders may target anyone (CRD 2209).
    let leader = leader_token(&app).await;
    let (status, _, _) =
        app.request("GET", &format!("/api/agents/{other}/skills"), Some(&leader), None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn add_skill_validation_errors() {
    let app = spawn_app().await;
    let agent = app.seed_agent("sv@test.com", "password1", "agent").await;
    let token = app.login("sv@test.com", "password1").await.0;
    let path = format!("/api/agents/{agent}/skills");

    for bad in [
        json!({"category": "technical", "level": "expert"}),                       // missing name
        json!({"name": "x", "category": "technical", "level": "expert"}),          // name too short
        json!({"name": "Valid", "category": "cooking", "level": "expert"}),        // bad category
        json!({"name": "Valid", "category": "technical", "level": "guru"}),        // bad level
        json!({"name": "Valid", "category": "technical", "level": "expert",
               "description": "d".repeat(501)}),                                   // long description
        json!({"name": "Valid", "category": "technical", "level": "expert",
               "certified": "yes"}),                                               // non-boolean
    ] {
        let (status, _, _) = app.request("POST", &path, Some(&token), Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // Duplicate name surfaces as a server error (CRD 2220).
    let (status, _, _) =
        app.request("POST", &path, Some(&token), Some(skill_payload().await)).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) =
        app.request("POST", &path, Some(&token), Some(skill_payload().await)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn update_skill_merges_and_handles_certification() {
    let app = spawn_app().await;
    let agent = app.seed_agent("us@test.com", "password1", "agent").await;
    let token = app.login("us@test.com", "password1").await.0;
    let (_, created, _) = app
        .request("POST", &format!("/api/agents/{agent}/skills"), Some(&token), Some(skill_payload().await))
        .await;
    let skill_id = created["data"]["id"].as_str().unwrap().to_string();

    // Turning certification on stamps the timestamp (CRD 2227).
    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/agents/{agent}/skills/{skill_id}"),
            Some(&token),
            Some(json!({"level": "expert", "certified": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["level"], "expert");
    assert_eq!(body["data"]["certified"], true);
    assert!(body["data"]["certifiedAt"].is_string());

    // Absent `certified` preserves the prior timestamp.
    let stamp = body["data"]["certifiedAt"].clone();
    let (_, body, _) = app
        .request("PUT", &format!("/api/agents/{agent}/skills/{skill_id}"), Some(&token), Some(json!({"description": "kept"})))
        .await;
    assert_eq!(body["data"]["certifiedAt"], stamp);

    // Turning it off clears the timestamp.
    let (_, body, _) = app
        .request("PUT", &format!("/api/agents/{agent}/skills/{skill_id}"), Some(&token), Some(json!({"certified": false})))
        .await;
    assert_eq!(body["data"]["certified"], false);
    assert!(body["data"]["certifiedAt"].is_null());

    // Unknown skill surfaces as a server error (CRD 2229).
    let (status, _, _) = app
        .request("PUT", &format!("/api/agents/{agent}/skills/ghost"), Some(&token), Some(json!({"level": "expert"})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_skill_and_not_found() {
    let app = spawn_app().await;
    let agent = app.seed_agent("ds@test.com", "password1", "agent").await;
    let token = app.login("ds@test.com", "password1").await.0;
    let (_, created, _) = app
        .request("POST", &format!("/api/agents/{agent}/skills"), Some(&token), Some(skill_payload().await))
        .await;
    let skill_id = created["data"]["id"].as_str().unwrap().to_string();

    let (status, _, _) = app
        .request("DELETE", &format!("/api/agents/{agent}/skills/{skill_id}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    // Already gone -> 404 (CRD 2237).
    let (status, _, _) = app
        .request("DELETE", &format!("/api/agents/{agent}/skills/{skill_id}"), Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn skill_statistics_summary() {
    let app = spawn_app().await;
    let agent = app.seed_agent("ss@test.com", "password1", "agent").await;
    let token = app.login("ss@test.com", "password1").await.0;
    let path = format!("/api/agents/{agent}/skills");

    // Zero rate with no skills (CRD 2244).
    let (status, body, _) =
        app.request("GET", &format!("{path}/statistics"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["total"], 0);
    assert_eq!(body["data"]["certificationRate"], 0.0);

    app.request("POST", &path, Some(&token), Some(json!({
        "name": "One", "category": "technical", "level": "expert", "certified": true,
    })))
    .await;
    app.request("POST", &path, Some(&token), Some(json!({
        "name": "Two", "category": "technical", "level": "beginner",
    })))
    .await;
    app.request("POST", &path, Some(&token), Some(json!({
        "name": "Three", "category": "language", "level": "beginner",
    })))
    .await;

    let (_, body, _) = app.request("GET", &format!("{path}/statistics"), Some(&token), None).await;
    assert_eq!(body["data"]["total"], 3);
    assert_eq!(body["data"]["byCategory"]["technical"], 2);
    assert_eq!(body["data"]["byCategory"]["language"], 1);
    assert_eq!(body["data"]["byLevel"]["beginner"], 2);
    assert_eq!(body["data"]["certifiedCount"], 1);
    assert_eq!(body["data"]["certificationRate"], 33.33);
}

// --------------------------------------------------------------------------- presence

#[tokio::test]
async fn get_status_defaults_to_offline_and_auto_expires() {
    let app = spawn_app().await;
    let agent = app.seed_agent("ps@test.com", "password1", "agent").await;
    let token = app.login("ps@test.com", "password1").await.0;

    // Never set -> default offline record (CRD 2251).
    let (status, body, _) =
        app.request("GET", &format!("/api/agents/{agent}/status"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["status"], "offline");

    // A stored record with a past expiry auto-transitions to offline on read.
    sqlx::query(
        "INSERT INTO agent_status (agent_id, status, since, available_until)
         VALUES (?, 'busy', '2026-01-01T00:00:00.000Z', '2026-01-02T00:00:00.000Z')",
    )
    .bind(&agent)
    .execute(&app.state.db)
    .await
    .unwrap();
    let (_, body, _) =
        app.request("GET", &format!("/api/agents/{agent}/status"), Some(&token), None).await;
    assert_eq!(body["data"]["status"], "offline");
    assert_eq!(body["data"]["note"], "auto-expired");
    // The auto-expiry transition is recorded in history (CRD 2254).
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_status_history WHERE agent_id = ?")
            .bind(&agent)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(history, 1);
}

#[tokio::test]
async fn update_status_validates_and_records_history() {
    let app = spawn_app().await;
    let agent = app.seed_agent("us2@test.com", "password1", "agent").await;
    let token = app.login("us2@test.com", "password1").await.0;
    let path = format!("/api/agents/{agent}/status");

    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let (status, body, _) = app
        .request("PUT", &path, Some(&token), Some(json!({"status": "busy", "availableUntil": future, "note": "deep work"})))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["status"], "busy");
    assert_eq!(body["data"]["note"], "deep work");
    assert!(body["data"]["availableUntil"].is_string());

    // Error conditions (CRD 2262).
    for bad in [
        json!({}),                                                       // missing status
        json!({"status": "vacation"}),                                   // invalid status
        json!({"status": "online", "availableUntil": "not-a-date"}),     // unparseable
        json!({"status": "online", "availableUntil": "2020-01-01T00:00:00Z"}), // not future
        json!({"status": "online", "note": "n".repeat(201)}),            // long note
    ] {
        let (status, _, _) = app.request("PUT", &path, Some(&token), Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // History entry recorded.
    let history: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_status_history WHERE agent_id = ?")
            .bind(&agent)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(history, 1);
}

#[tokio::test]
async fn status_history_newest_first_with_limit() {
    let app = spawn_app().await;
    let agent = app.seed_agent("hist@test.com", "password1", "agent").await;
    let token = app.login("hist@test.com", "password1").await.0;
    let path = format!("/api/agents/{agent}/status");
    for s in ["online", "busy", "away"] {
        app.request("PUT", &path, Some(&token), Some(json!({"status": s}))).await;
    }

    let (status, body, _) =
        app.request("GET", &format!("{path}/history?limit=2"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["status"], "away");
    assert_eq!(items[1]["status"], "busy");
    assert!(items[0]["recordedAt"].is_string());
}

// ----------------------------------------------------------------- operator details

#[tokio::test]
async fn get_agent_combines_profile_skills_and_status() {
    let app = spawn_app().await;
    let agent = app.seed_agent("full@test.com", "password1", "agent").await;
    let token = app.login("full@test.com", "password1").await.0;
    app.request("POST", &format!("/api/agents/{agent}/skills"), Some(&token), Some(skill_payload().await))
        .await;
    app.request("PUT", &format!("/api/agents/{agent}/status"), Some(&token), Some(json!({"status": "online"})))
        .await;

    let (status, body, _) =
        app.request("GET", &format!("/api/agents/{agent}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["email"], "full@test.com");
    assert_eq!(body["data"]["password"], "");
    assert_eq!(body["data"]["skills"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"]["currentStatus"]["status"], "online");
}

#[tokio::test]
async fn get_agent_error_conditions() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    // Unknown operator (valid-length id) -> 404.
    let (status, _, _) = app
        .request("GET", "/api/agents/agent-0000-0000-0000-000000000000", Some(&admin), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Identifier length out of range -> 400.
    let (status, _, _) = app.request("GET", "/api/agents/tiny-id", Some(&admin), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Ordinary operator targeting another -> 403.
    let a = app.seed_agent("d1@test.com", "password1", "agent").await;
    app.seed_agent("d2@test.com", "password1", "agent").await;
    let token = app.login("d2@test.com", "password1").await.0;
    let (status, _, _) = app.request("GET", &format!("/api/agents/{a}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------------- update operator

#[tokio::test]
async fn update_agent_profile_fields_and_team() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let old_team = app.seed_team("Old").await;
    let new_team = app.seed_team("New").await;
    let agent = app.seed_agent("pu@test.com", "password1", "agent").await;
    app.add_membership(&agent, old_team, "lead", true).await;

    let (status, body, _) = app
        .request(
            "PUT",
            &format!("/api/agents/{agent}"),
            Some(&admin),
            Some(json!({"displayName": "Promoted", "email": "promoted@test.com", "teamId": new_team})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["displayName"], "Promoted");
    assert_eq!(body["data"]["email"], "promoted@test.com");
    assert_eq!(body["data"]["teamId"], new_team);
    // Prior memberships replaced with one primary membership (CRD 2285).
    let memberships: Vec<(i64, i64)> =
        sqlx::query_as("SELECT team_id, is_primary FROM team_members WHERE agent_id = ?")
            .bind(&agent)
            .fetch_all(&app.state.db)
            .await
            .unwrap();
    assert_eq!(memberships, vec![(new_team, 1)]);
}

#[tokio::test]
async fn update_agent_validation_errors() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = app.seed_agent("uv@test.com", "password1", "agent").await;
    let path = format!("/api/agents/{agent}");

    for bad in [
        json!({}),                              // empty body
        json!({"email": "not-an-email"}),       // invalid email
        json!({"displayName": "x"}),            // too short
        json!({"role": "supervisor"}),          // invalid role
        json!({"teamId": "abc"}),               // invalid team id format
        json!({"isActive": "yes"}),             // non-boolean
    ] {
        let (status, _, _) = app.request("PUT", &path, Some(&admin), Some(bad.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // Unknown operator -> 404.
    let (status, _, _) = app
        .request("PUT", "/api/agents/agent-0000-0000-0000-000000000000", Some(&admin), Some(json!({"displayName": "Ghost"})))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_agent_privilege_guards() {
    let app = spawn_app().await;
    let team = app.seed_team("Self").await;
    let agent = app.seed_agent("guard@test.com", "password1", "agent").await;
    let token = app.login("guard@test.com", "password1").await.0;

    // Role elevation above caller's own level -> 403 (CRD 2284).
    let (status, body, _) = app
        .request("PUT", &format!("/api/agents/{agent}"), Some(&token), Some(json!({"role": "admin"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["error"].as_str().unwrap().to_lowercase().contains("role"));

    // Non-admin self team change -> 403 (CRD 2284).
    let (status, _, _) = app
        .request("PUT", &format!("/api/agents/{agent}"), Some(&token), Some(json!({"teamId": team})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Self displayName change is allowed.
    let (status, _, _) = app
        .request("PUT", &format!("/api/agents/{agent}"), Some(&token), Some(json!({"displayName": "My New Name"})))
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn update_agent_collision_and_team_errors_surface_as_500() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let agent = app.seed_agent("c1@test.com", "password1", "agent").await;
    app.seed_agent("taken@test.com", "password1", "agent").await;

    // Duplicate email -> internal error (CRD 2287).
    let (status, _, _) = app
        .request("PUT", &format!("/api/agents/{agent}"), Some(&admin), Some(json!({"email": "taken@test.com"})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    // Nonexistent target team -> internal error (CRD 2287).
    let (status, _, _) = app
        .request("PUT", &format!("/api/agents/{agent}"), Some(&admin), Some(json!({"teamId": 9999})))
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

// ------------------------------------------------------------------- delete operator

#[tokio::test]
async fn delete_agent_is_admin_only_with_cleanup() {
    let app = spawn_app().await;
    let admin = admin_token(&app).await;
    let team = app.seed_team("Crew").await;
    let agent = app.seed_agent("del@test.com", "password1", "agent").await;
    app.add_membership(&agent, team, "member", true).await;

    // Non-administrator -> 403.
    let token = app.login("del@test.com", "password1").await.0;
    let (status, _, _) =
        app.request("DELETE", &format!("/api/agents/{agent}"), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) =
        app.request("DELETE", &format!("/api/agents/{agent}"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agents WHERE id = ?")
        .bind(&agent)
        .fetch_one(&app.state.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0);
    let memberships: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM team_members WHERE agent_id = ?")
            .bind(&agent)
            .fetch_one(&app.state.db)
            .await
            .unwrap();
    assert_eq!(memberships, 0);

    // Unknown operator -> 404.
    let (status, _, _) =
        app.request("DELETE", &format!("/api/agents/{agent}"), Some(&admin), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// -------------------------------------------------------------------- auth boundaries

#[tokio::test]
async fn agent_routes_require_authentication() {
    let app = spawn_app().await;
    for (method, path) in [
        ("GET", "/api/agents"),
        ("PUT", "/api/agents/batch"),
        ("POST", "/api/agents/search"),
        ("GET", "/api/agents/status/statistics"),
        ("GET", "/api/agents/agent-0000-0000-0000-000000000000"),
    ] {
        let (status, _, _) = app.request(method, path, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}
