//! Activity Log & Reversible Actions (CRD §3.5, lines 2448-2612).

mod common;

use axum::http::StatusCode;
use serde_json::{json, Value};

fn iso_ago_minutes(mins: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::minutes(mins))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn iso_ago_days(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

struct Ctx {
    app: common::TestApp,
    admin_id: String,
    admin_token: String,
    agent_id: String,
    agent_token: String,
}

async fn setup() -> Ctx {
    let app = common::spawn_app().await;
    let admin_id = app.seed_agent("admin@example.com", "Password1!", "admin").await;
    let agent_id = app.seed_agent("agent@example.com", "Password1!", "agent").await;
    let (admin_token, _, _) = app.login("admin@example.com", "Password1!").await;
    let (agent_token, _, _) = app.login("agent@example.com", "Password1!").await;
    Ctx { app, admin_id, admin_token, agent_id, agent_token }
}

async fn restore_state_of(app: &common::TestApp, id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT restore_state FROM activity_logs WHERE id = $1")
        .bind(id)
        .fetch_one(&app.state.db)
        .await
        .unwrap()
}

// ------------------------------------------------------------------------- listing

#[tokio::test]
async fn list_requires_auth() {
    let ctx = setup().await;
    let (status, _, _) = ctx.app.request("GET", "/api/activities", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_newest_first_with_pagination() {
    let ctx = setup().await;
    for (mins, rid) in [(3, "1"), (2, "2"), (1, "3")] {
        ctx.app
            .seed_activity(
                &ctx.agent_id,
                "order probe",
                "tag",
                Some(rid),
                None,
                Some(&iso_ago_minutes(mins)),
            )
            .await;
    }
    // Trailing-slash spelling of the listing path is also served (CRD 2462).
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/activities/?action=order%20probe", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    let rids: Vec<&str> = items.iter().map(|i| i["resourceId"].as_str().unwrap()).collect();
    assert_eq!(rids, vec!["3", "2", "1"], "newest first");
    assert_eq!(body["data"]["total"], 3);
    assert_eq!(body["data"]["page"], 1);
    assert_eq!(body["data"]["limit"], 50);
    let entry = &items[0];
    for key in ["id", "userId", "userName", "userRole", "action", "createdAt"] {
        assert!(!entry[key].is_null(), "missing {key}");
    }
}

#[tokio::test]
async fn list_rejects_out_of_range_page_and_limit() {
    let ctx = setup().await;
    for (path, field) in [
        ("/api/activities?page=0", "page"),
        ("/api/activities?page=1001", "page"),
        ("/api/activities?page=abc", "page"),
        ("/api/activities?limit=0", "limit"),
        ("/api/activities?limit=1001", "limit"),
    ] {
        let (status, body, _) = ctx.app.request("GET", path, Some(&ctx.admin_token), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {body}");
        assert_eq!(body["data"]["errors"][0]["field"], field, "{path}");
    }
}

#[tokio::test]
async fn list_caps_effective_page_size_at_100() {
    let ctx = setup().await;
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/activities?limit=500", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["limit"], 100);
}

#[tokio::test]
async fn list_rejects_bad_dates() {
    let ctx = setup().await;
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/activities?startDate=not-a-date", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["data"]["errors"][0]["field"], "startDate");

    let (status, body, _) = ctx
        .app
        .request(
            "GET",
            "/api/activities?startDate=2026-06-10&endDate=2026-06-01",
            Some(&ctx.admin_token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn list_scopes_non_admin_to_own_entries() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.admin_id, "scope probe", "tag", Some("1"), None, None).await;
    ctx.app.seed_activity(&ctx.agent_id, "scope probe", "tag", Some("2"), None, None).await;
    // The agent's attempt to view the admin's entries is silently overridden (CRD 2467).
    let path = format!("/api/activities?userId={}", ctx.admin_id);
    let (status, body, _) = ctx.app.request("GET", &path, Some(&ctx.agent_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let items = body["data"]["items"].as_array().unwrap();
    assert!(!items.is_empty());
    for item in items {
        assert_eq!(item["userId"], json!(ctx.agent_id));
    }
}

#[tokio::test]
async fn list_admin_filters_by_actor_action_resource_and_window() {
    let ctx = setup().await;
    ctx.app
        .seed_activity(&ctx.agent_id, "filter probe", "tag", Some("1"), None,
            Some(&iso_ago_minutes(5)))
        .await;
    ctx.app
        .seed_activity(&ctx.agent_id, "filter probe", "team", Some("2"), None,
            Some(&iso_ago_days(10)))
        .await;
    ctx.app
        .seed_activity(&ctx.admin_id, "filter probe", "tag", Some("3"), None,
            Some(&iso_ago_minutes(5)))
        .await;

    let path = format!("/api/activities?userId={}&action=filter%20probe", ctx.agent_id);
    let (_, body, _) = ctx.app.request("GET", &path, Some(&ctx.admin_token), None).await;
    assert_eq!(body["data"]["total"], 2, "{body}");

    let path = format!(
        "/api/activities?userId={}&action=filter%20probe&resourceType=tag",
        ctx.agent_id
    );
    let (_, body, _) = ctx.app.request("GET", &path, Some(&ctx.admin_token), None).await;
    assert_eq!(body["data"]["total"], 1);

    let start = iso_ago_days(1).replace('+', "%2B");
    let path = format!("/api/activities?action=filter%20probe&startDate={start}");
    let (_, body, _) = ctx.app.request("GET", &path, Some(&ctx.admin_token), None).await;
    assert_eq!(body["data"]["total"], 2, "window excludes the 10-day-old entry");
}

// ------------------------------------------------------------------- single entry

#[tokio::test]
async fn get_activity_visible_to_admin_and_actor() {
    let ctx = setup().await;
    let id = ctx
        .app
        .seed_activity(&ctx.agent_id, "view probe", "tag", Some("9"),
            Some(json!({"k": "v"})), None)
        .await;
    for token in [&ctx.admin_token, &ctx.agent_token] {
        let (status, body, _) =
            ctx.app.request("GET", &format!("/api/activities/{id}"), Some(token), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["data"]["id"], id);
        assert_eq!(body["data"]["action"], "view probe");
        assert_eq!(body["data"]["details"]["k"], "v");
    }
}

#[tokio::test]
async fn get_activity_forbidden_for_non_actor() {
    let ctx = setup().await;
    let id = ctx.app.seed_activity(&ctx.admin_id, "secret probe", "tag", None, None, None).await;
    let (status, body, _) = ctx
        .app
        .request("GET", &format!("/api/activities/{id}"), Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Forbidden");
}

#[tokio::test]
async fn get_activity_not_found_and_invalid_id() {
    let ctx = setup().await;
    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/999999", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Activity not found");

    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/abc", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "Invalid activity id");
}

// --------------------------------------------------------------- per-actor stats

#[tokio::test]
async fn user_stats_aggregate_own_actions() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.agent_id, "stat probe a", "tag", None, None, None).await;
    ctx.app.seed_activity(&ctx.agent_id, "stat probe a", "tag", None, None, None).await;
    ctx.app.seed_activity(&ctx.agent_id, "stat probe b", "tag", None, None, None).await;
    let path = format!("/api/activities/user/{}/stats", ctx.agent_id);
    let (status, body, _) = ctx.app.request("GET", &path, Some(&ctx.agent_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"]["totalActions"].as_i64().unwrap() >= 3);
    assert_eq!(body["data"]["actionBreakdown"]["stat probe a"], 2);
    assert_eq!(body["data"]["actionBreakdown"]["stat probe b"], 1);
    assert!(!body["data"]["recentActivities"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn user_stats_forbidden_for_other_actor_unless_admin() {
    let ctx = setup().await;
    let path = format!("/api/activities/user/{}/stats", ctx.admin_id);
    let (status, body, _) = ctx.app.request("GET", &path, Some(&ctx.agent_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Forbidden");

    let path = format!("/api/activities/user/{}/stats", ctx.agent_id);
    let (status, _, _) = ctx.app.request("GET", &path, Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
}

// -------------------------------------------------------------------------- cleanup

#[tokio::test]
async fn cleanup_purges_entries_older_than_retention() {
    let ctx = setup().await;
    let old =
        ctx.app
            .seed_activity(&ctx.agent_id, "ancient probe", "tag", None, None,
                Some(&iso_ago_days(120)))
            .await;
    let fresh =
        ctx.app.seed_activity(&ctx.agent_id, "fresh probe", "tag", None, None, None).await;

    let (status, body, _) = ctx
        .app
        .request("POST", "/api/activities/cleanup?days=90", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["deletedCount"], 1);
    assert!(body["message"].as_str().unwrap().contains("90"));

    assert!(common::TestApp::request(&ctx.app, "GET", &format!("/api/activities/{old}"),
        Some(&ctx.admin_token), None).await.0 == StatusCode::NOT_FOUND);
    assert!(common::TestApp::request(&ctx.app, "GET", &format!("/api/activities/{fresh}"),
        Some(&ctx.admin_token), None).await.0 == StatusCode::OK);
}

#[tokio::test]
async fn cleanup_admin_only_and_retention_bounds() {
    let ctx = setup().await;
    let (status, body, _) =
        ctx.app.request("POST", "/api/activities/cleanup", Some(&ctx.agent_token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "Forbidden");

    for path in [
        "/api/activities/cleanup?days=29",
        "/api/activities/cleanup?days=3651",
        "/api/activities/cleanup?days=-5",
        "/api/activities/cleanup?days=abc",
    ] {
        let (status, body, _) = ctx.app.request("POST", path, Some(&ctx.admin_token), None).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{path}: {body}");
        assert_eq!(body["data"]["errors"][0]["field"], "days");
    }

    // Default retention (90 days) needs no parameters.
    let (status, _, _) =
        ctx.app.request("POST", "/api/activities/cleanup", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK);
}

// ---------------------------------------------------------------- admin statistics

#[tokio::test]
async fn admin_only_statistics_endpoints_reject_agents() {
    let ctx = setup().await;
    for path in [
        "/api/activities/overview",
        "/api/activities/stats/resources",
        "/api/activities/stats/roles",
        "/api/activities/stats/custom?startDate=2026-01-01&endDate=2026-01-31",
        "/api/activities/trends",
        "/api/activities/heatmap",
        "/api/activities/metrics",
    ] {
        let (status, body, _) = ctx.app.request("GET", path, Some(&ctx.agent_token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        assert_eq!(body["error"], "Forbidden");
    }
}

#[tokio::test]
async fn overview_returns_window_aggregates() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.agent_id, "overview probe", "tag", None, None, None).await;
    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/overview", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["totalActivities"].as_i64().unwrap() >= 1);
    assert_eq!(data["actionBreakdown"]["overview probe"], 1);
    assert!(!data["topUsers"].as_array().unwrap().is_empty());
    assert!(!data["dailyActivities"].as_array().unwrap().is_empty());
    assert_eq!(data["period"]["days"], 7);
    assert!(data["period"]["startDate"].is_string() && data["period"]["endDate"].is_string());
}

#[tokio::test]
async fn resource_stats_carry_share_and_label() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.agent_id, "res probe", "tag", None, None, None).await;
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/activities/stats/resources", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["data"]["resources"].as_array().unwrap();
    let tag = items.iter().find(|i| i["resourceType"] == "tag").expect("tag bucket");
    assert!(tag["count"].as_i64().unwrap() >= 1);
    assert!(tag["percentage"].as_f64().unwrap() > 0.0);
    assert_eq!(tag["label"], "标签");
}

#[tokio::test]
async fn role_stats_carry_share_and_label() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.agent_id, "role probe", "tag", None, None, None).await;
    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/stats/roles", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let items = body["data"]["roles"].as_array().unwrap();
    let agent = items.iter().find(|i| i["role"] == "agent").expect("agent bucket");
    assert!(agent["count"].as_i64().unwrap() >= 1);
    assert_eq!(agent["label"], "客服");
}

#[tokio::test]
async fn custom_stats_require_both_dates_and_aggregate_range() {
    let ctx = setup().await;
    let (status, body, _) = ctx
        .app
        .request("GET", "/api/activities/stats/custom", Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"], "Start date and end date are required");

    ctx.app
        .seed_activity(&ctx.agent_id, "custom probe", "tag", None, None,
            Some(&iso_ago_minutes(30)))
        .await;
    ctx.app
        .seed_activity(&ctx.agent_id, "custom probe", "tag", None, None,
            Some(&iso_ago_minutes(45)))
        .await;
    ctx.app
        .seed_activity(&ctx.agent_id, "custom probe", "tag", None, None,
            Some(&iso_ago_days(10)))
        .await;
    let start = iso_ago_days(1).replace('+', "%2B");
    let end = iso_ago_minutes(0).replace('+', "%2B");
    let path = format!("/api/activities/stats/custom?startDate={start}&endDate={end}");
    let (status, body, _) = ctx.app.request("GET", &path, Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["actionBreakdown"]["custom probe"], 2, "{body}");
    assert_eq!(body["data"]["period"]["days"], 1);
}

#[tokio::test]
async fn trends_report_per_day_action_breakdown() {
    let ctx = setup().await;
    ctx.app
        .seed_activity(&ctx.agent_id, "trend probe", "tag", None, None,
            Some(&iso_ago_minutes(10)))
        .await;
    ctx.app
        .seed_activity(&ctx.agent_id, "trend probe", "tag", None, None, Some(&iso_ago_days(2)))
        .await;
    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/trends", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let series = body["data"]["trends"].as_array().unwrap();
    let with_probe: Vec<&Value> =
        series.iter().filter(|d| d["actions"]["trend probe"].is_number()).collect();
    assert_eq!(with_probe.len(), 2, "{body}");
    for day in with_probe {
        assert!(day["total"].as_i64().unwrap() >= 1);
        assert!(day["date"].is_string());
    }
}

#[tokio::test]
async fn heatmap_buckets_classify_intensity() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.agent_id, "heat probe", "tag", None, None, None).await;
    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/heatmap", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let buckets = body["data"]["heatmap"].as_array().unwrap();
    assert!(!buckets.is_empty());
    for b in buckets {
        assert!(b["date"].is_string());
        let hour = b["hour"].as_i64().unwrap();
        assert!((0..24).contains(&hour));
        let count = b["count"].as_i64().unwrap();
        let expected = if count >= 50 { "high" } else if count >= 20 { "medium" } else { "low" };
        assert_eq!(b["intensity"], expected);
    }
}

#[tokio::test]
async fn metrics_summarize_load_and_leaders() {
    let ctx = setup().await;
    ctx.app.seed_activity(&ctx.agent_id, "metric probe", "tag", None, None, None).await;
    let (status, body, _) =
        ctx.app.request("GET", "/api/activities/metrics", Some(&ctx.admin_token), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert!(data["avgActivitiesPerDay"].as_f64().unwrap() > 0.0);
    assert!(data["peakHour"].is_number());
    assert!(data["mostActiveUser"].is_string());
    assert!(data["mostCommonAction"].is_string());
    assert_eq!(data["systemLoad"], "low");
}

// ------------------------------------------------------------------------- restore

async fn seed_reversible_tag_update(ctx: &Ctx, tag_id: i64, actor: &str) -> i64 {
    ctx.app
        .seed_activity(
            actor,
            "tag update",
            "tag",
            Some(&tag_id.to_string()),
            Some(json!({
                "reversible": true,
                "old": { "name": "old-name" },
                "new": { "name": "new-name" },
            })),
            None,
        )
        .await
}

#[tokio::test]
async fn restore_validates_id_and_existence_before_auth() {
    let ctx = setup().await;
    // No credentials supplied: entry validation precedes authentication (CRD 2553-2554).
    let (status, body, _) =
        ctx.app.request("POST", "/api/activities/abc/restore", None, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "Invalid activity id");

    let (status, body, _) =
        ctx.app.request("POST", "/api/activities/999999/restore", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "Activity not found");
}

#[tokio::test]
async fn restore_rejects_non_reversible_entries() {
    let ctx = setup().await;
    let id = ctx
        .app
        .seed_activity(&ctx.agent_id, "tag update", "tag", Some("1"),
            Some(json!({"reversible": false})), None)
        .await;
    let (status, body, _) =
        ctx.app.request("POST", &format!("/api/activities/{id}/restore"), None, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "NOT_REVERSIBLE");
}

#[tokio::test]
async fn restore_requires_authentication() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;
    let (status, body, _) =
        ctx.app.request("POST", &format!("/api/activities/{id}/restore"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "Unauthenticated");
}

#[tokio::test]
async fn restore_reverts_tag_update_and_links_audit_entries() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["restoredActivityId"], id);
    let new_id = body["data"]["restoreActivityId"].as_i64().unwrap();
    assert!(new_id > id);

    // The resource returned to its prior state.
    let name: String = sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(tag)
        .fetch_one(&ctx.app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "old-name");

    // The original entry is permanently marked restored and links the new entry.
    let (_, original, _) = ctx
        .app
        .request("GET", &format!("/api/activities/{id}"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(original["data"]["details"]["restoreState"], "restored");
    assert_eq!(original["data"]["details"]["restoredBy"], new_id);

    // The restore audit entry itself is recorded and not reversible.
    let (_, restore_entry, _) = ctx
        .app
        .request("GET", &format!("/api/activities/{new_id}"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(restore_entry["data"]["action"], "tag restore");
    assert_eq!(restore_entry["data"]["details"]["reversible"], false);
    assert_eq!(restore_entry["data"]["details"]["restoredActivityId"], id);
    assert_eq!(restore_entry["data"]["userId"], json!(ctx.admin_id));
}

#[tokio::test]
async fn restore_original_actor_allowed_others_forbidden() {
    let ctx = setup().await;
    let other_id = ctx.app.seed_agent("other@example.com", "Password1!", "agent").await;
    let (other_token, _, _) = ctx.app.login("other@example.com", "Password1!").await;
    let _ = other_id;

    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&other_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "Forbidden");

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn restore_policy_can_require_admin() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag update",
            "tag",
            Some(&tag.to_string()),
            Some(json!({
                "reversible": true,
                "old": { "name": "old-name" },
                "new": { "name": "new-name" },
                "restorePolicy": { "requiresAdmin": true },
            })),
            None,
        )
        .await;

    // The original actor is rejected when the policy demands an administrator.
    let (status, _, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.agent_token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn restore_twice_reports_already_restored() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let new_id = body["data"]["restoreActivityId"].as_i64().unwrap();

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "ALREADY_RESTORED");
    assert_eq!(body["data"]["restoredBy"], new_id);
}

#[tokio::test]
async fn restore_in_progress_is_rejected_with_retry_hint() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;
    sqlx::query("UPDATE activity_logs SET restore_state = 'in_progress' WHERE id = $1")
        .bind(id)
        .execute(&ctx.app.state.db)
        .await
        .unwrap();

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "RESTORE_IN_PROGRESS");
    assert!(body["data"]["retryAfter"].is_number());
}

#[tokio::test]
async fn restore_window_expiry() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    // Default window: 24 hours from capture (CRD 2577).
    let stale = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag update",
            "tag",
            Some(&tag.to_string()),
            Some(json!({"reversible": true, "old": {"name": "old-name"}, "new": {"name": "new-name"}})),
            Some(&iso_ago_days(3)),
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{stale}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["code"], "RESTORE_EXPIRED");

    // An explicit capture-time expiry overrides the default.
    let expired = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag update",
            "tag",
            Some(&tag.to_string()),
            Some(json!({
                "reversible": true,
                "old": {"name": "old-name"},
                "new": {"name": "new-name"},
                "restorePolicy": {"expiresAt": "2020-01-01T00:00:00Z"},
            })),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{expired}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert_eq!(body["code"], "RESTORE_EXPIRED");
}

#[tokio::test]
async fn restore_missing_resource_id_rejected() {
    let ctx = setup().await;
    let id = ctx
        .app
        .seed_activity(&ctx.agent_id, "tag update", "tag", None,
            Some(json!({"reversible": true, "old": {"name": "a"}, "new": {"name": "b"}})), None)
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "MISSING_RESOURCE_ID");
}

#[tokio::test]
async fn restore_unknown_handler_rejected() {
    let ctx = setup().await;
    let id = ctx
        .app
        .seed_activity(&ctx.agent_id, "message send", "message", Some("m1"),
            Some(json!({"reversible": true})), None)
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "RESTORE_HANDLER_NOT_FOUND");
}

#[tokio::test]
async fn restore_missing_resource_rejected() {
    let ctx = setup().await;
    let id = ctx
        .app
        .seed_activity(&ctx.agent_id, "tag update", "tag", Some("999999"),
            Some(json!({"reversible": true, "old": {"name": "a"}, "new": {"name": "b"}})), None)
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "RESOURCE_NOT_FOUND");
}

#[tokio::test]
async fn restore_unsupported_resource_type_rejected() {
    let ctx = setup().await;
    let id = ctx
        .app
        .seed_activity(&ctx.agent_id, "tag update", "webhook", Some("7"),
            Some(json!({"reversible": true, "old": {"name": "a"}, "new": {"name": "b"}})), None)
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["code"], "UNSUPPORTED_RESOURCE_TYPE");
}

#[tokio::test]
async fn restore_conflict_reported_then_forced() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("drifted", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "RESTORE_CONFLICT");
    let conflicts = body["data"]["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["field"], "name");
    assert_eq!(conflicts[0]["originalValue"], "new-name");
    assert_eq!(conflicts[0]["currentValue"], "drifted");
    assert_eq!(conflicts[0]["restoreValue"], "old-name");

    // The entry remains restorable: force overrides the drift (CRD 2576).
    assert_eq!(restore_state_of(&ctx.app, id).await, None);
    let (status, body, _) = ctx
        .app
        .request(
            "POST",
            &format!("/api/activities/{id}/restore"),
            Some(&ctx.admin_token),
            Some(json!({"force": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let name: String = sqlx::query_scalar("SELECT name FROM tags WHERE id = $1")
        .bind(tag)
        .fetch_one(&ctx.app.state.db)
        .await
        .unwrap();
    assert_eq!(name, "old-name");
}

#[tokio::test]
async fn restore_conversation_transfer_returns_prior_team() {
    let ctx = setup().await;
    let team_a = ctx.app.seed_team("Team A").await;
    let team_b = ctx.app.seed_team("Team B").await;
    let customer = ctx.app.seed_customer("line", "u1", "Customer", Some(team_a)).await;
    let conv = ctx.app.seed_conversation(customer, Some(team_b), "active").await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "conversation transfer",
            "conversation",
            Some(&conv),
            Some(json!({
                "reversible": true,
                "old": { "teamId": team_a },
                "new": { "teamId": team_b },
            })),
            None,
        )
        .await;

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let current: Option<i64> = sqlx::query_scalar("SELECT team_id FROM conversations WHERE id = $1")
        .bind(&conv)
        .fetch_one(&ctx.app.state.db)
        .await
        .unwrap();
    assert_eq!(current, Some(team_a));
}

#[tokio::test]
async fn restore_conversation_transfer_detects_drift() {
    let ctx = setup().await;
    let team_a = ctx.app.seed_team("Team A").await;
    let team_b = ctx.app.seed_team("Team B").await;
    let team_c = ctx.app.seed_team("Team C").await;
    let customer = ctx.app.seed_customer("line", "u2", "Customer", Some(team_a)).await;
    // The conversation drifted to a third team after the recorded transfer.
    let conv = ctx.app.seed_conversation(customer, Some(team_c), "active").await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "conversation transfer",
            "conversation",
            Some(&conv),
            Some(json!({
                "reversible": true,
                "old": { "teamId": team_a },
                "new": { "teamId": team_b },
            })),
            None,
        )
        .await;

    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "RESTORE_CONFLICT");
    assert_eq!(body["data"]["conflicts"][0]["field"], "teamId");
    assert_eq!(body["data"]["conflicts"][0]["currentValue"], team_c);
}

#[tokio::test]
async fn restore_tag_create_soft_deletes_record() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("created-tag", &ctx.agent_id).await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag create",
            "tag",
            Some(&tag.to_string()),
            Some(json!({"reversible": true, "old": null, "new": {"name": "created-tag"}})),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (deleted_at, active): (Option<String>, i64) =
        sqlx::query_as("SELECT deleted_at, is_active FROM tags WHERE id = $1")
            .bind(tag)
            .fetch_one(&ctx.app.state.db)
            .await
            .unwrap();
    assert!(deleted_at.is_some());
    assert_eq!(active, 0);
}

#[tokio::test]
async fn restore_tag_delete_undeletes_record() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag_full("gone-tag", &ctx.agent_id, None, false).await;
    let deleted_ts = iso_ago_minutes(5);
    sqlx::query("UPDATE tags SET deleted_at = $1 WHERE id = $2")
        .bind(&deleted_ts)
        .bind(tag)
        .execute(&ctx.app.state.db)
        .await
        .unwrap();
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag delete",
            "tag",
            Some(&tag.to_string()),
            Some(json!({
                "reversible": true,
                "old": { "isActive": true, "deletedAt": null },
                "new": { "isActive": false, "deletedAt": deleted_ts },
            })),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (deleted_at, active): (Option<String>, i64) =
        sqlx::query_as("SELECT deleted_at, is_active FROM tags WHERE id = $1")
            .bind(tag)
            .fetch_one(&ctx.app.state.db)
            .await
            .unwrap();
    assert_eq!(deleted_at, None);
    assert_eq!(active, 1);
}

#[tokio::test]
async fn restore_tag_assign_removes_association() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "u3", "Customer", None).await;
    let tag = ctx.app.seed_tag("assoc-tag", &ctx.agent_id).await;
    ctx.app.add_customer_tag(customer, tag, &ctx.agent_id).await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag assign",
            "customer_tag",
            Some(&tag.to_string()),
            Some(json!({"reversible": true, "customerId": customer, "tagId": tag})),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customer_tags WHERE customer_id = $1 AND tag_id = $2",
    )
    .bind(customer)
    .bind(tag)
    .fetch_one(&ctx.app.state.db)
    .await
    .unwrap();
    assert_eq!(count, 0);

    // Restoring again: the association no longer exists -> RESOURCE_NOT_FOUND would
    // apply for a fresh assign entry whose target vanished.
    let id2 = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag assign",
            "customer_tag",
            Some(&tag.to_string()),
            Some(json!({"reversible": true, "customerId": customer, "tagId": tag})),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id2}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "RESOURCE_NOT_FOUND");
}

#[tokio::test]
async fn restore_tag_unassign_re_adds_association() {
    let ctx = setup().await;
    let customer = ctx.app.seed_customer("line", "u4", "Customer", None).await;
    let tag = ctx.app.seed_tag("unassoc-tag", &ctx.agent_id).await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag unassign",
            "customer_tag",
            Some(&tag.to_string()),
            Some(json!({
                "reversible": true,
                "customerId": customer,
                "tagId": tag,
                "assignedBy": ctx.agent_id,
                "assignedAt": iso_ago_days(0),
            })),
            None,
        )
        .await;
    // Add-back tolerates the association being absent (CRD 2557 step 4).
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customer_tags WHERE customer_id = $1 AND tag_id = $2",
    )
    .bind(customer)
    .bind(tag)
    .fetch_one(&ctx.app.state.db)
    .await
    .unwrap();
    assert_eq!(count, 1);

    // A second unassign entry now sees the re-added association as drift.
    let id2 = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag unassign",
            "customer_tag",
            Some(&tag.to_string()),
            Some(json!({"reversible": true, "customerId": customer, "tagId": tag})),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id2}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "RESTORE_CONFLICT");

    // Force succeeds and leaves the association in place.
    let (status, _, _) = ctx
        .app
        .request(
            "POST",
            &format!("/api/activities/{id2}/restore"),
            Some(&ctx.admin_token),
            Some(json!({"force": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn restore_reinstates_removed_team_membership() {
    let ctx = setup().await;
    let team = ctx.app.seed_team("Restore Team").await;
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "team remove member",
            "team_member",
            Some(&ctx.agent_id),
            Some(json!({
                "reversible": true,
                "old": {
                    "agentId": ctx.agent_id,
                    "teamId": team,
                    "roleInTeam": "lead",
                    "isPrimary": true,
                    "joinedAt": iso_ago_days(30),
                },
                "new": null,
            })),
            None,
        )
        .await;
    // Membership-style restore tolerates the membership being absent (CRD 2557 step 4).
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT role, is_primary FROM team_members WHERE agent_id = $1 AND team_id = $2",
    )
    .bind(&ctx.agent_id)
    .bind(team)
    .fetch_optional(&ctx.app.state.db)
    .await
    .unwrap();
    assert_eq!(row, Some(("lead".to_string(), 1)));
}

#[tokio::test]
async fn restore_batch_failure_releases_claim_for_retry() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("any-name", &ctx.agent_id).await;
    // A prior-state field outside the allowlist makes the reversal batch fail (CRD 2593).
    let id = ctx
        .app
        .seed_activity(
            &ctx.agent_id,
            "tag update",
            "tag",
            Some(&tag.to_string()),
            Some(json!({"reversible": true, "old": {"bogusField": "x"}, "new": {}})),
            None,
        )
        .await;
    let (status, body, _) = ctx
        .app
        .request("POST", &format!("/api/activities/{id}/restore"), Some(&ctx.admin_token), None)
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["code"], "BATCH_FAILED");
    // The claim is not left dangling: a later retry is possible (CRD 2573).
    assert_eq!(restore_state_of(&ctx.app, id).await, None);
}

#[tokio::test]
async fn restore_accepts_test_only_caller_header() {
    let ctx = setup().await;
    let tag = ctx.app.seed_tag("new-name", &ctx.agent_id).await;
    let id = seed_reversible_tag_update(&ctx, tag, &ctx.agent_id).await;
    let header = json!({"id": ctx.admin_id, "role": "admin"}).to_string();
    let (status, body, _) = ctx
        .app
        .request_with_headers(
            "POST",
            &format!("/api/activities/{id}/restore"),
            None,
            None,
            &[("x-test-user", header.as_str())],
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["restoredActivityId"], id);
}
